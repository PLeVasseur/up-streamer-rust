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

#[cfg(feature = "mqtt-transport")]
use crate::config::MqttMode;
use crate::config::{Config, EndpointConfig, TransportKind};
use clap::Parser;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;
use up_rust::usubscription::USubscription;
use up_rust::{UCode, UStatus};
use up_streamer::{OwnedFrameEndpoint, UStreamer};
use up_transport_iceoryx2_rust::{transport::UTransportIceoryx2, MessagingPattern};
#[cfg(feature = "mqtt-transport")]
use up_transport_mqtt5::{
    Mqtt5Transport, Mqtt5TransportOptions, TransportMode as MqttTransportMode,
};
#[cfg(feature = "vsomeip-transport")]
use up_transport_vsomeip::UPTransportVsomeip;
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};
use usubscription_static_file::USubscriptionStaticFile;

#[derive(Parser)]
#[command()]
struct StreamerArgs {
    #[arg(short, long, value_name = "FILE")]
    config: String,
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::INVALID_ARGUMENT, message.into())
}

#[cfg(feature = "vsomeip-transport")]
fn required_field<'a>(
    endpoint: &EndpointConfig,
    field: &'a Option<String>,
    name: &str,
) -> Result<&'a str, UStatus> {
    field
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            invalid_config(format!(
                "OwnedFrameEndpoint '{}' requires non-empty field '{}'",
                endpoint.name, name
            ))
        })
}

#[cfg(feature = "mqtt-transport")]
fn normalize_mqtt_uri(uri: &str) -> String {
    if uri.contains("://") {
        uri.to_string()
    } else {
        format!("mqtt://{uri}")
    }
}

fn resolve_config_path(base_dir: &Path, raw_path: &str) -> PathBuf {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

#[cfg(feature = "mqtt-transport")]
impl From<MqttMode> for MqttTransportMode {
    fn from(value: MqttMode) -> Self {
        match value {
            MqttMode::InVehicle => Self::InVehicle,
            MqttMode::OffVehicle => Self::OffVehicle,
        }
    }
}

async fn endpoint_from_config(
    endpoint: &EndpointConfig,
    base_dir: &Path,
) -> Result<OwnedFrameEndpoint, UStatus> {
    match endpoint.transport {
        TransportKind::ZenohOwned => {
            let zenoh_config = match endpoint.zenoh_config_file.as_ref() {
                Some(path) if !path.is_empty() => {
                    let path = resolve_config_path(base_dir, path);
                    ZenohConfig::from_file(&path).map_err(|error| {
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
            Ok(OwnedFrameEndpoint::from_owned(
                &endpoint.name,
                &endpoint.authority,
                transport,
            ))
        }
        TransportKind::Iceoryx2ZeroCopy => {
            let transport = UTransportIceoryx2::build(MessagingPattern::PublishSubscribe)?;
            Ok(OwnedFrameEndpoint::from_zero_copy(
                &endpoint.name,
                &endpoint.authority,
                transport,
            ))
        }
        TransportKind::Mqtt5Owned => {
            #[cfg(feature = "mqtt-transport")]
            {
                let mut options = Mqtt5TransportOptions::default();
                if let Some(broker_uri) = endpoint.mqtt_broker_uri.as_deref() {
                    options.mqtt_client_options.broker_uri = normalize_mqtt_uri(broker_uri);
                }
                options.mqtt_client_options.client_id = endpoint.mqtt_client_id.clone();
                options.mode = endpoint.mqtt_mode.into();

                let transport =
                    Arc::new(Mqtt5Transport::new(options, endpoint.authority.clone()).await?);
                transport.connect().await?;
                Ok(OwnedFrameEndpoint::from_owned(
                    &endpoint.name,
                    &endpoint.authority,
                    transport,
                ))
            }
            #[cfg(not(feature = "mqtt-transport"))]
            {
                Err(invalid_config(format!(
                    "OwnedFrameEndpoint '{}' uses mqtt5_owned but configurable-streamer was built without feature 'mqtt-transport'",
                    endpoint.name
                )))
            }
        }
        TransportKind::VsomeipOwned => {
            #[cfg(feature = "vsomeip-transport")]
            {
                let config_file = required_field(
                    endpoint,
                    &endpoint.vsomeip_config_file,
                    "vsomeip_config_file",
                )?;
                let config_file = resolve_config_path(base_dir, config_file);
                let remote_authority =
                    required_field(endpoint, &endpoint.remote_authority, "remote_authority")?;
                let local_uentity = endpoint.local_uentity.ok_or_else(|| {
                    invalid_config(format!(
                        "OwnedFrameEndpoint '{}' requires field 'local_uentity'",
                        endpoint.name
                    ))
                })?;
                let local_uversion = endpoint.local_uversion.unwrap_or(1);
                let local_resource = endpoint.local_resource.unwrap_or(0);
                let local_uri = up_rust::UUri::try_from_parts(
                    &endpoint.authority,
                    local_uentity,
                    local_uversion,
                    local_resource,
                )
                .map_err(|error| invalid_config(format!("invalid vSOME/IP local URI: {error}")))?;
                let transport = Arc::new(UPTransportVsomeip::new_with_config(
                    local_uri,
                    &remote_authority.to_string(),
                    &config_file,
                    None,
                )?);
                Ok(OwnedFrameEndpoint::from_owned(
                    &endpoint.name,
                    &endpoint.authority,
                    transport,
                ))
            }
            #[cfg(not(feature = "vsomeip-transport"))]
            {
                Err(invalid_config(format!(
                    "OwnedFrameEndpoint '{}' uses vsomeip_owned but configurable-streamer was built without feature 'vsomeip-transport'",
                    endpoint.name
                )))
            }
        }
    }
}

async fn wire_forwarding_rules(
    streamer: &mut UStreamer,
    endpoints: &HashMap<String, OwnedFrameEndpoint>,
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
    let config_path = PathBuf::from(&args.config);
    let config_base_dir = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut file = File::open(&config_path)
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

    let usubscription_path =
        resolve_config_path(&config_base_dir, &config.usubscription_config.file_path);
    let usubscription: Arc<dyn USubscription> = Arc::new(USubscriptionStaticFile::new(
        usubscription_path.display().to_string(),
    ));
    let mut streamer = UStreamer::new(
        "configurable-streamer",
        config.up_streamer_config.message_queue_size,
        usubscription,
    )
    .await?;

    let mut endpoints = HashMap::new();
    for endpoint_config in &config.endpoints {
        let endpoint = endpoint_from_config(endpoint_config, &config_base_dir).await?;
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
