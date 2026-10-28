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

//! Publish uProtocol messages using the selected Zenoh endpoint API and wire.
use example_streamer_uses::common;

use clap::{Parser, ValueEnum};
use common::cli;
use common::payloads::{
    arrow_payload_bytes, native_payload_alignment, native_payload_bytes, omgidl_payload_bytes,
    xcdrv2_payload_bytes, NativeContext,
};
use configurable_streamer_wire_support::prepared::{
    zenoh_core, PreparedAddress, PreparedEndpoint, PreparedSession,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use up_rust::ProtobufWire;
use up_rust::{
    PayloadCodecIdentity, PayloadEncoding, UFrameMetadata, UMessageBuilder, UOwnedTransport,
    UStatus, UTransport, UWithNativePrefixWire,
};
use up_transport_zenoh::{
    zenoh_config::{Config, EndPoint},
    ZenohPreparedConfig,
};
use up_wire_arrow::ArrowWire;
use up_wire_omgidl::OmgIdlWire;
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
    Arrow,
    Omgidl,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(skip)]
    native: NativeContext,
    /// The endpoint for Zenoh client to connect to
    #[arg(short, long, default_value = DEFAULT_ENDPOINT)]
    endpoint: String,
    /// Optional Zenoh JSON5 configuration file. When set, this overrides --endpoint.
    #[arg(long)]
    zenoh_config: Option<String>,
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

    let mut args = Args::parse();
    args.native = example_streamer_uses::common::payloads::load_native_context()?;
    if args.encoding == Encoding::Native {
        args.native.identity()?;
    }

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;

    if let Some(route_family) = args.route_family {
        return run_selected_wire_publisher(&args, route_family, uentity, uversion, resource).await;
    }

    println!("uE_publisher");

    let zenoh_config = zenoh_config_from_args(&args)?;

    let publisher_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let publisher: Arc<dyn UTransport> = Arc::new(
        common::backend::zenoh(zenoh_config, publisher_uuri.to_string())
            .await
            .unwrap(),
    );

    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;

    let sent_count = common::classic::send(
        publisher.as_ref(),
        (args.send_count > 0).then_some(args.send_count),
        Duration::from_millis(args.send_interval_ms),
        |_| {
            let mut builder = UMessageBuilder::publish(source.clone());
            let publish_msg = if args.encoding == Encoding::Protobuf {
                builder
                    .build_with_payload(common::classic::timer_payload(), PayloadEncoding::PROTOBUF)
                    .unwrap()
            } else {
                builder
                    .build_with_payload(
                        selected_payload_bytes(&args)?,
                        selected_payload_encoding(&args)?,
                    )
                    .map_err(|error| {
                        invalid_config(format!("failed to build publish message: {error:?}"))
                    })?
            };
            println!("Sending Publish message:\n{publish_msg:?}");

            Ok(publish_msg)
        },
    )
    .await?;

    if args.send_count > 0 {
        println!("Completed bounded send run: sent_count={sent_count}");
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
        .with_payload_encoding(selected_payload_encoding(args)?)
        .build()
        .map_err(|error| invalid_config(format!("failed to build frame metadata: {error:?}")))?;
    let metadata = if args.encoding == Encoding::Native {
        args.native.stamp(metadata)?
    } else {
        metadata
    };
    let payload = selected_payload_bytes(args)?;
    let zenoh_config = zenoh_config_from_args(args)?;

    if route_family == RouteFamily::CopyMinimized {
        let alignment = if args.encoding == Encoding::Native {
            native_payload_alignment()
        } else {
            args.payload_alignment
        };
        let catalog = [common::endpoint_config::prepared_address(
            metadata.source(),
            None,
            payload.len(),
            alignment,
            true,
        )];
        let core = zenoh_core(
            zenoh_config,
            &local_uri.to_string(),
            &catalog,
            ZenohPreparedConfig::default(),
        )
        .await?;
        macro_rules! send {
            ($transport:expr) => {
                send_zero_copy_repeated(
                    Arc::new($transport),
                    &catalog,
                    metadata,
                    &payload,
                    alignment,
                    args,
                )
                .await?
            };
        }
        match args.encoding {
            Encoding::Native => {
                send!(core.into_stable_container_transport(args.native.agreement()?.clone()))
            }
            Encoding::Protobuf => send!(core.into_protobuf_transport()),
            Encoding::Xcdrv2 => send!(core.into_native_prefix_wire_transport(XcdrV2Wire)),
            Encoding::Arrow => send!(core.into_native_prefix_wire_transport(ArrowWire)),
            Encoding::Omgidl => send!(core.into_native_prefix_wire_transport(OmgIdlWire)),
        }
        println!("FLOW sent_payload_bytes={} role=publisher", payload.len());
        return Ok(());
    }

    {
        let transport = common::backend::zenoh_owned_selected(
            zenoh_config,
            local_uri.to_string(),
            args.encoding.into(),
            &args.native,
        )
        .await?;
        send_owned_repeated(&transport, metadata, &payload, args).await?;
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

fn zenoh_config_from_args(args: &Args) -> Result<Config, UStatus> {
    if let Some(path) = &args.zenoh_config {
        return Config::from_file(path).map_err(|error| {
            invalid_config(format!("failed to load Zenoh config {path}: {error:?}"))
        });
    }
    Ok(zenoh_config_from_endpoint(&args.endpoint))
}

async fn send_owned_repeated(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
    args: &Args,
) -> Result<(), UStatus> {
    common::startup::wait_for_start().await?;
    common::flow::send_owned_repeated(
        transport,
        metadata,
        payload,
        common::flow::SendOptions {
            count: args.selected_send_count,
            interval: Some(Duration::from_millis(args.send_interval_ms)),
        },
    )
    .await?;
    common::startup::wait_for_completion().await
}

async fn send_zero_copy_repeated<T>(
    transport: Arc<T>,
    catalog: &[PreparedAddress],
    metadata: UFrameMetadata,
    payload: &[u8],
    alignment: usize,
    args: &Args,
) -> Result<(), UStatus>
where
    T: PreparedEndpoint,
{
    let session = Arc::new(PreparedSession::new(transport, catalog)?);
    common::startup::wait_for_start().await?;
    common::flow::send_native_repeated(
        &session,
        metadata,
        payload,
        alignment,
        common::flow::SendOptions {
            count: args.selected_send_count,
            interval: Some(Duration::from_millis(args.send_interval_ms)),
        },
    )
    .await?;
    common::startup::wait_for_completion().await
}

fn selected_payload_encoding(args: &Args) -> Result<PayloadEncoding, UStatus> {
    Ok(match args.encoding {
        Encoding::Native => args.native.encoding()?,
        Encoding::Protobuf => ProtobufWire::encoding(),
        Encoding::Xcdrv2 => XcdrV2Wire::encoding(),
        Encoding::Arrow => ArrowWire::encoding(),
        Encoding::Omgidl => OmgIdlWire::encoding(),
    })
}

fn selected_payload_bytes(args: &Args) -> Result<Vec<u8>, UStatus> {
    match args.encoding {
        Encoding::Native => native_payload_bytes(NATIVE_PAYLOAD_MAGIC, 1, &args.payload),
        Encoding::Protobuf => Ok(args.payload.as_bytes().to_vec()),
        Encoding::Xcdrv2 => xcdrv2_payload_bytes(1, args.uauthority.clone(), &args.payload),
        Encoding::Arrow => arrow_payload_bytes(1, &args.payload),
        Encoding::Omgidl => omgidl_payload_bytes(1, &args.payload),
    }
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(up_rust::UCode::InvalidArgument, message.into())
}

impl From<Encoding> for common::encoding::Encoding {
    fn from(value: Encoding) -> Self {
        match value {
            Encoding::Native => Self::Native,
            Encoding::Protobuf => Self::Protobuf,
            Encoding::Xcdrv2 => Self::Xcdrv2,
            Encoding::Arrow => Self::Arrow,
            Encoding::Omgidl => Self::Omgidl,
        }
    }
}
