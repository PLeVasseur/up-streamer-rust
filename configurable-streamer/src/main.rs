/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

mod config;

use crate::config::{Config, EndpointConfig, TransportKind};
use clap::Parser;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use tracing::info;
use up_rust::{UCode, UStatus, USubscription};
use up_streamer::{Endpoint, UStreamer};
use up_transport_iceoryx2_rust::{transport::UTransportIceoryx2, MessagingPattern};
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};
use usubscription_static_file::USubscriptionStaticFile;

#[derive(Parser)]
#[command()]
struct StreamerArgs {
    #[arg(short, long, value_name = "FILE")]
    config: String,
}

async fn endpoint_from_config(endpoint: &EndpointConfig) -> Result<Endpoint, UStatus> {
    match endpoint.transport {
        TransportKind::ZenohOwned => {
            let zenoh_config = match endpoint.zenoh_config_file.as_ref() {
                Some(path) if !path.is_empty() => {
                    ZenohConfig::from_file(path).map_err(|error| {
                        UStatus::fail_with_code(
                            UCode::INVALID_ARGUMENT,
                            format!("Unable to load Zenoh config file: {error}"),
                        )
                    })?
                }
                _ => ZenohConfig::default(),
            };
            let transport = Arc::new(
                UPTransportZenoh::builder(endpoint.authority.clone())?
                    .with_config(zenoh_config)
                    .build()
                    .await?,
            );
            Ok(Endpoint::from_owned(
                &endpoint.name,
                &endpoint.authority,
                transport,
            ))
        }
        TransportKind::Iceoryx2ZeroCopy => {
            let transport =
                UTransportIceoryx2::build_zero_copy(MessagingPattern::PublishSubscribe)?;
            Ok(Endpoint::from_zero_copy(
                &endpoint.name,
                &endpoint.authority,
                transport,
            ))
        }
    }
}

async fn wire_forwarding_rules(
    streamer: &mut UStreamer,
    endpoints: &HashMap<String, Endpoint>,
    endpoint_configs: &[EndpointConfig],
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        let left_endpoint = endpoints.get(&endpoint_config.name).ok_or_else(|| {
            UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                format!(
                    "Unknown endpoint in forwarding rules: {}",
                    endpoint_config.name
                ),
            )
        })?;

        for forwarding_target in &endpoint_config.forwarding {
            let right_endpoint = endpoints.get(forwarding_target).ok_or_else(|| {
                UStatus::fail_with_code(
                    UCode::INVALID_ARGUMENT,
                    format!("Unknown forwarding target endpoint: {forwarding_target}"),
                )
            })?;
            streamer
                .add_route_ref(left_endpoint, right_endpoint)
                .await?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = StreamerArgs::parse();
    let mut file = File::open(args.config)
        .map_err(|e| UStatus::fail_with_code(UCode::NOT_FOUND, format!("File not found: {e}")))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|e| {
        UStatus::fail_with_code(UCode::INTERNAL, format!("Unable to read config file: {e}"))
    })?;
    let config: Config = json5::from_str(&contents).map_err(|e| {
        UStatus::fail_with_code(
            UCode::INVALID_ARGUMENT,
            format!("Unable to parse config: {e}"),
        )
    })?;

    let usubscription: Arc<dyn USubscription> = Arc::new(USubscriptionStaticFile::new(
        config.usubscription_config.file_path.clone(),
    ));
    let mut streamer = UStreamer::new(
        "configurable-streamer",
        config.up_streamer_config.message_queue_size,
        usubscription,
    )
    .await?;

    let mut endpoints = HashMap::new();
    for endpoint_config in &config.endpoints {
        let endpoint = endpoint_from_config(endpoint_config).await?;
        if endpoints
            .insert(endpoint_config.name.clone(), endpoint)
            .is_some()
        {
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                format!("Duplicate endpoint name found: {}", endpoint_config.name),
            ));
        }
    }

    wire_forwarding_rules(&mut streamer, &endpoints, &config.endpoints).await?;

    println!("READY streamer_initialized");
    info!("Streamer initialized; waiting for shutdown signal");
    tokio::signal::ctrl_c().await.map_err(|error| {
        UStatus::fail_with_code(
            UCode::INTERNAL,
            format!("Unable to wait for shutdown signal: {error}"),
        )
    })
}
