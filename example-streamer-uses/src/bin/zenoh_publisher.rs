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

use chrono::Local;
use chrono::Timelike;
use clap::{Parser, ValueEnum};
use common::cli;
use common::payloads::{
    native_payload_alignment, native_payload_bytes, xcdrv2_payload_bytes, SelectedWireNativePayload,
};
use common::protobuf_payload;
use hello_world_protos::hello_world_topics::Timer;
use hello_world_protos::timeofday::TimeOfDay;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use up_rust::selected_wire_user_api::{ProtobufWire, StableContainerWireFormat};
use up_rust::{
    PayloadEncoding, PayloadFormat, StableContainerPayload, UFrameMetadata, UMessageBuilder,
    UOwnedFrame, UOwnedTransport, UPayloadFormat, UStatus, UTransport, UTxBuffer, UTxLoanSpec,
    UZeroCopyTransport,
};
use up_transport_zenoh::{
    zenoh_config::{Config, EndPoint},
    UPTransportZenoh, ZenohOwnedCore, ZenohZeroCopyCore,
};
use up_wire_xcdrv2::XcdrV2Wire;

const DEFAULT_ENDPOINT: &str = "tcp/127.0.0.1:7447";
const DEFAULT_UAUTHORITY: &str = "authority-b";
const DEFAULT_UENTITY: &str = "0x3039";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x8001";
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"ZPUB");

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
    /// Authority for the local publisher identity and publish source URI
    #[arg(long, default_value = DEFAULT_UAUTHORITY)]
    uauthority: String,
    /// UEntity ID for publish source URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UENTITY)]
    uentity: String,
    /// UEntity major version for publish source URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UVERSION)]
    uversion: String,
    /// Resource ID for publish source URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_RESOURCE)]
    resource: String,
    /// Number of publish messages to send before exiting (0 means run forever)
    #[arg(long, default_value_t = 0)]
    send_count: u64,
    /// Milliseconds to wait between publish sends
    #[arg(long, default_value_t = 1000)]
    send_interval_ms: u64,
    /// Optional selected-wire route family. Omit this flag to use the classic UTransport example path.
    #[arg(long, value_enum)]
    route_family: Option<RouteFamily>,
    /// Payload encoding used when --route-family selects a selected-wire path.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Text payload used by selected-wire examples.
    #[arg(long, default_value = "zenoh-publisher")]
    payload: String,
    /// Number of selected-wire payloads to send before exiting.
    #[arg(long, default_value_t = 1)]
    selected_send_count: usize,
    /// Payload alignment for non-native copy-minimized sends.
    #[arg(long, default_value_t = 1)]
    payload_alignment: usize,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    let args = Args::parse();

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;

    if let Some(route_family) = args.route_family {
        return run_selected_wire_publisher(&args, route_family, uentity, uversion, resource).await;
    }

    println!("uE_publisher");

    let mut zenoh_config = Config::default();

    if !args.endpoint.is_empty() {
        // Specify the address to listen on using IPv4
        let ipv4_endpoint =
            EndPoint::from_str(args.endpoint.as_str()).expect("Unable to set endpoint");

        // Add the IPv4 endpoint to the Zenoh configuration
        zenoh_config
            .connect
            .endpoints
            .set(vec![ipv4_endpoint])
            .expect("Unable to set Zenoh Config");
    }

    let publisher_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let publisher: Arc<dyn UTransport> = Arc::new(
        UPTransportZenoh::new(zenoh_config, publisher_uuri.to_string())
            .await
            .unwrap(),
    );

    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;

    let mut sent_count: u64 = 0;
    loop {
        if args.send_count > 0 && sent_count >= args.send_count {
            println!("Completed bounded send run: sent_count={sent_count}");
            break;
        }

        tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;

        let now = Local::now();

        let time_of_day = TimeOfDay {
            hours: now.hour() as i32,
            minutes: now.minute() as i32,
            seconds: now.second() as i32,
            nanos: now.nanosecond() as i32,
            ..Default::default()
        };

        let timer_message = Timer {
            time: Some(time_of_day).into(),
            ..Default::default()
        };

        let publish_msg = UMessageBuilder::publish(source.clone())
            .build_with_payload(protobuf_payload(&timer_message), UPayloadFormat::Protobuf)
            .unwrap();
        println!("Sending Publish message:\n{publish_msg:?}");

        publisher.send(publish_msg).await?;
        sent_count += 1;
    }

    if args.send_count > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}

async fn run_selected_wire_publisher(
    args: &Args,
    route_family: RouteFamily,
    uentity: u32,
    uversion: u8,
    resource: u16,
) -> Result<(), UStatus> {
    let local_uri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;
    let metadata = UFrameMetadata::publish(source)
        .with_payload_encoding(selected_payload_encoding(args.encoding))
        .build()
        .map_err(|error| invalid_config(format!("failed to build frame metadata: {error:?}")))?;
    let payload = selected_payload_bytes(args)?;
    let zenoh_config = zenoh_config_from_endpoint(&args.endpoint);

    match (route_family, args.encoding) {
        (RouteFamily::OwnedFrame, Encoding::Native) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(StableContainerWireFormat),
            ) as Arc<dyn UOwnedTransport>;
            send_owned_repeated(&transport, metadata, &payload, args.selected_send_count).await?;
        }
        (RouteFamily::OwnedFrame, Encoding::Protobuf) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(ProtobufWire),
            ) as Arc<dyn UOwnedTransport>;
            send_owned_repeated(&transport, metadata, &payload, args.selected_send_count).await?;
        }
        (RouteFamily::OwnedFrame, Encoding::Xcdrv2) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(XcdrV2Wire),
            ) as Arc<dyn UOwnedTransport>;
            send_owned_repeated(&transport, metadata, &payload, args.selected_send_count).await?;
        }
        (RouteFamily::CopyMinimized, Encoding::Native) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(StableContainerWireFormat),
            );
            send_zero_copy_repeated(
                &transport,
                metadata,
                &payload,
                native_payload_alignment(),
                args,
            )
            .await?;
        }
        (RouteFamily::CopyMinimized, Encoding::Protobuf) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(ProtobufWire),
            );
            send_zero_copy_repeated(&transport, metadata, &payload, args.payload_alignment, args)
                .await?;
        }
        (RouteFamily::CopyMinimized, Encoding::Xcdrv2) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(XcdrV2Wire),
            );
            send_zero_copy_repeated(&transport, metadata, &payload, args.payload_alignment, args)
                .await?;
        }
    }

    println!("FLOW sent_payload_bytes={} role=publisher", payload.len());
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

async fn send_owned_repeated(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
    count: usize,
) -> Result<(), UStatus> {
    for _ in 0..count.max(1) {
        transport
            .send_owned(
                UOwnedFrame::with_payload(metadata.clone(), payload.to_vec()).map_err(|error| {
                    invalid_config(format!("failed to build owned frame: {error:?}"))
                })?,
            )
            .await?;
    }
    Ok(())
}

async fn send_zero_copy_repeated<T>(
    transport: &Arc<T>,
    metadata: UFrameMetadata,
    payload: &[u8],
    alignment: usize,
    args: &Args,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
{
    for attempt in 0..args.selected_send_count.max(1) {
        let mut tx = transport
            .loan_tx(UTxLoanSpec::payload(
                metadata.clone(),
                payload.len(),
                alignment,
            )?)
            .await?;
        tx.payload_mut().copy_from_slice(payload);
        transport.send_zero_copy(tx).await?;
        if attempt + 1 < args.selected_send_count.max(1) {
            tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;
        }
    }
    Ok(())
}

fn selected_payload_encoding(encoding: Encoding) -> PayloadEncoding {
    match encoding {
        Encoding::Native => StableContainerPayload::<SelectedWireNativePayload>::encoding(),
        Encoding::Protobuf => ProtobufWire::encoding(),
        Encoding::Xcdrv2 => XcdrV2Wire::encoding(),
    }
}

fn selected_payload_bytes(args: &Args) -> Result<Vec<u8>, UStatus> {
    match args.encoding {
        Encoding::Native => native_payload_bytes(NATIVE_PAYLOAD_MAGIC, 1, &args.payload),
        Encoding::Protobuf => Ok(args.payload.as_bytes().to_vec()),
        Encoding::Xcdrv2 => xcdrv2_payload_bytes(1, args.uauthority.clone(), &args.payload),
    }
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(up_rust::UCode::InvalidArgument, message.into())
}
