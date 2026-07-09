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

mod common;

use clap::{Parser, ValueEnum};
use common::cli;
use common::PublishReceiver;
use std::str::FromStr;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::info;
use up_rust::selected_wire_user_api::{ProtobufWire, StableContainerWireFormat};
use up_rust::{
    UCode, UFrameView, UListener, UOwnedFrame, UOwnedTransport, UStatus, UTransport, UUri,
    UZeroCopyRxLease, UZeroCopyTransport,
};
use up_transport_zenoh::{
    zenoh_config::{Config, EndPoint},
    UPTransportZenoh, ZenohOwnedCore, ZenohZeroCopyCore,
};
use up_wire_xcdrv2::XcdrV2Wire;

const DEFAULT_ENDPOINT: &str = "tcp/127.0.0.1:7447";
const DEFAULT_UAUTHORITY: &str = "authority-b";
const DEFAULT_UENTITY: &str = "0x5BB0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x0";
const DEFAULT_SOURCE_AUTHORITY: &str = "authority-a";
const DEFAULT_SOURCE_UENTITY: &str = "0x5BA0";
const DEFAULT_SOURCE_UVERSION: &str = "0x1";
const DEFAULT_SOURCE_RESOURCE: &str = "0x8001";

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RouteFamily {
    OwnedFrame,
    CopyMinimized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Encoding {
    Native,
    Protobuf,
    Xcdrv2,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The endpoint for Zenoh client to connect to
    #[arg(short, long, default_value = DEFAULT_ENDPOINT)]
    endpoint: String,
    /// Optional Zenoh JSON5 configuration file. When set, this overrides --endpoint.
    #[arg(long)]
    zenoh_config: Option<String>,
    /// Authority for the local subscriber identity
    #[arg(long, default_value = DEFAULT_UAUTHORITY)]
    uauthority: String,
    /// UEntity ID for local subscriber identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UENTITY)]
    uentity: String,
    /// UEntity major version for local subscriber identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UVERSION)]
    uversion: String,
    /// Resource ID for local subscriber identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_RESOURCE)]
    resource: String,
    /// Source authority filter for publish subscription
    #[arg(long, default_value = DEFAULT_SOURCE_AUTHORITY)]
    source_authority: String,
    /// Source UEntity ID filter for publish subscription (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_SOURCE_UENTITY)]
    source_uentity: String,
    /// Source UEntity major version filter for publish subscription (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_SOURCE_UVERSION)]
    source_uversion: String,
    /// Source resource ID filter for publish subscription (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_SOURCE_RESOURCE)]
    source_resource: String,
    /// Optional selected-wire route family. Omit this flag to use the classic UTransport example path.
    #[arg(long, value_enum)]
    route_family: Option<RouteFamily>,
    /// Payload encoding used when --route-family selects a selected-wire path.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Timeout for selected-wire receive examples.
    #[arg(long, default_value_t = 5000)]
    timeout_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    let args = Args::parse();

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;
    let source_uentity = cli::parse_u32_status("--source-uentity", &args.source_uentity)?;
    let source_uversion = cli::parse_u8_status("--source-uversion", &args.source_uversion)?;
    let source_resource = cli::parse_u16_status("--source-resource", &args.source_resource)?;

    if let Some(route_family) = args.route_family {
        return run_selected_wire_subscriber(
            &args,
            route_family,
            uentity,
            uversion,
            source_uentity,
            source_uversion,
            source_resource,
        )
        .await;
    }

    info!("Started zenoh_subscriber");

    let zenoh_config = zenoh_config_from_args(&args)?;

    let subscriber_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let subscriber: Arc<dyn UTransport> = Arc::new(
        UPTransportZenoh::new(zenoh_config, subscriber_uuri.to_string())
            .await
            .unwrap(),
    );

    let _subscriber_sink = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;

    let source_filter = cli::build_uuri(
        &args.source_authority,
        source_uentity,
        source_uversion,
        source_resource,
    )?;

    let publish_receiver: Arc<dyn UListener> = Arc::new(PublishReceiver);
    subscriber
        .register_listener(&source_filter, None, publish_receiver.clone())
        .await?;

    println!("READY listener_registered");

    loop {
        thread::park();
    }
}

async fn run_selected_wire_subscriber(
    args: &Args,
    route_family: RouteFamily,
    uentity: u32,
    uversion: u8,
    source_uentity: u32,
    source_uversion: u8,
    source_resource: u16,
) -> Result<(), UStatus> {
    let local_uri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let source_filter = cli::build_uuri(
        &args.source_authority,
        source_uentity,
        source_uversion,
        source_resource,
    )?;
    let zenoh_config = zenoh_config_from_args(args)?;

    let payload = match (route_family, args.encoding) {
        (RouteFamily::OwnedFrame, Encoding::Native) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(StableContainerWireFormat),
            ) as Arc<dyn UOwnedTransport>;
            println!("READY listener_registered");
            receive_owned_payload(&transport, &source_filter, args.timeout_ms).await?
        }
        (RouteFamily::OwnedFrame, Encoding::Protobuf) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(ProtobufWire),
            ) as Arc<dyn UOwnedTransport>;
            println!("READY listener_registered");
            receive_owned_payload(&transport, &source_filter, args.timeout_ms).await?
        }
        (RouteFamily::OwnedFrame, Encoding::Xcdrv2) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(XcdrV2Wire),
            ) as Arc<dyn UOwnedTransport>;
            println!("READY listener_registered");
            receive_owned_payload(&transport, &source_filter, args.timeout_ms).await?
        }
        (RouteFamily::CopyMinimized, Encoding::Native) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(StableContainerWireFormat),
            );
            println!("READY listener_registered");
            receive_zero_copy_payload(&transport, &source_filter, args.timeout_ms).await?
        }
        (RouteFamily::CopyMinimized, Encoding::Protobuf) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(ProtobufWire),
            );
            println!("READY listener_registered");
            receive_zero_copy_payload(&transport, &source_filter, args.timeout_ms).await?
        }
        (RouteFamily::CopyMinimized, Encoding::Xcdrv2) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(XcdrV2Wire),
            );
            println!("READY listener_registered");
            receive_zero_copy_payload(&transport, &source_filter, args.timeout_ms).await?
        }
    };

    println!(
        "FLOW observed_payload_bytes={} role=subscriber",
        payload.len()
    );
    Ok(())
}

fn zenoh_config_from_endpoint(endpoint: &str) -> Config {
    let mut zenoh_config = Config::default();
    if !endpoint.is_empty() {
        let ipv4_endpoint = EndPoint::from_str(endpoint).expect("Unable to set endpoint");
        zenoh_config
            .connect
            .endpoints
            .set(vec![ipv4_endpoint])
            .expect("Unable to set Zenoh Config");
    }
    zenoh_config
}

fn zenoh_config_from_args(args: &Args) -> Result<Config, UStatus> {
    if let Some(path) = &args.zenoh_config {
        return Config::from_file(path).map_err(|error| {
            invalid_config(format!("failed to load Zenoh config {path}: {error:?}"))
        });
    }
    Ok(zenoh_config_from_endpoint(&args.endpoint))
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

async fn receive_owned_payload(
    transport: &Arc<dyn UOwnedTransport>,
    source_filter: &UUri,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus> {
    receive_owned_frame(transport, source_filter, timeout_ms)
        .await
        .map(|frame| frame.payload_bytes().to_vec())
}

async fn receive_owned_frame(
    transport: &Arc<dyn UOwnedTransport>,
    source_filter: &UUri,
    timeout_ms: u64,
) -> Result<UOwnedFrame, UStatus> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for owned frame",
            ));
        }
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(100)),
            transport.receive_owned(source_filter, None),
        )
        .await
        {
            Ok(Ok(frame)) => return Ok(frame),
            Ok(Err(error)) if error.get_code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
}

async fn receive_zero_copy_payload<T>(
    transport: &Arc<T>,
    source_filter: &UUri,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Rx: UZeroCopyRxLease,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for zero-copy frame",
            ));
        }
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(100)),
            transport.receive_zero_copy(source_filter, None),
        )
        .await
        {
            Ok(Ok(frame)) => return Ok(frame.try_contiguous_payload().unwrap_or(&[]).to_vec()),
            Ok(Err(error)) if error.get_code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
}
