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

//! Send uProtocol RPC requests using the selected Zenoh endpoint API and wire.
use example_streamer_uses::common;

use clap::{Parser, ValueEnum};
use common::cli;
use common::payloads::{
    arrow_payload_bytes, native_payload_alignment, native_payload_bytes, omgidl_payload_bytes,
    xcdrv2_payload_bytes, NativeContext,
};
use common::{protobuf_payload, ServiceResponseListener};
use configurable_streamer_wire_support::prepared::{
    zenoh_core, PreparedAddress, PreparedEndpoint, PreparedSession,
};
use hello_world_protos::hello_world_service::HelloRequest;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};
use up_rust::ProtobufWire;
use up_rust::UWithNativePrefixWire;
use up_rust::{
    PayloadCodecIdentity, PayloadEncoding, UCode, UFrameMetadata, UMessageBuilder, UOwnedTransport,
    UStatus, UTransport, UUri,
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
const DEFAULT_UENTITY: &str = "0x1236";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x0";
const DEFAULT_TARGET_AUTHORITY: &str = "authority-a";
const DEFAULT_TARGET_UENTITY: &str = "0x4321";
const DEFAULT_TARGET_UVERSION: &str = "0x1";
const DEFAULT_TARGET_RESOURCE: &str = "0x0421";

const REQUEST_TTL: u32 = 1000;
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"ZCLI");
const RESPONSE_LISTENER_SETTLE_MS: u64 = 100;

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
    /// Authority for the local client identity
    #[arg(long, default_value = DEFAULT_UAUTHORITY)]
    uauthority: String,
    /// UEntity ID for local client identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UENTITY)]
    uentity: String,
    /// UEntity major version for local client identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UVERSION)]
    uversion: String,
    /// Resource ID for local client identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_RESOURCE)]
    resource: String,
    /// Authority for the target service URI
    #[arg(long, default_value = DEFAULT_TARGET_AUTHORITY)]
    target_authority: String,
    /// UEntity ID for target service URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_TARGET_UENTITY)]
    target_uentity: String,
    /// UEntity major version for target service URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_TARGET_UVERSION)]
    target_uversion: String,
    /// Resource ID for target service URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_TARGET_RESOURCE)]
    target_resource: String,
    /// Number of requests to send before exiting (0 means run forever)
    #[arg(long, default_value_t = 0)]
    send_count: u64,
    /// Milliseconds to wait between request sends
    #[arg(long, default_value_t = 1000)]
    send_interval_ms: u64,
    /// Optional selected-wire route family. Omit this flag to use the classic UTransport example path.
    #[arg(long, value_enum)]
    route_family: Option<RouteFamily>,
    /// Payload encoding used when --route-family selects a selected-wire path.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Text payload used by selected-wire examples.
    #[arg(long, default_value = "zenoh-client")]
    payload: String,
    /// Timeout for selected-wire response receive.
    #[arg(long, default_value_t = 5000)]
    timeout_ms: u64,
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
    let target_uentity = cli::parse_u32_status("--target-uentity", &args.target_uentity)?;
    let target_uversion = cli::parse_u8_status("--target-uversion", &args.target_uversion)?;
    let target_resource = cli::parse_u16_status("--target-resource", &args.target_resource)?;

    if let Some(route_family) = args.route_family {
        return run_selected_wire_client(
            &args,
            route_family,
            uentity,
            uversion,
            target_uentity,
            target_uversion,
            target_resource,
        )
        .await;
    }

    info!("Started zenoh_client");

    let zenoh_config = zenoh_config_from_args(&args)?;

    let client_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let client: Arc<dyn UTransport> = Arc::new(
        common::backend::zenoh(zenoh_config, client_uuri.to_string())
            .await
            .unwrap(),
    );
    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;
    let sink = cli::build_uuri(
        &args.target_authority,
        target_uentity,
        target_uversion,
        target_resource,
    )?;

    let service_response_listener = Arc::new(ServiceResponseListener::default());
    client
        .register_listener(
            &sink,
            Some(&source),
            common::native::payload_listener(&args.native, service_response_listener.clone()),
        )
        .await?;

    let sent_count = common::classic::send(
        client.as_ref(),
        (args.send_count > 0).then_some(args.send_count),
        Duration::from_millis(args.send_interval_ms),
        |sent_count| {
            let hello_request = HelloRequest {
                name: format!("ue_client@i={sent_count}"),
                ..Default::default()
            };

            let mut builder = UMessageBuilder::request(sink.clone(), source.clone(), REQUEST_TTL);
            let request_msg = if args.encoding == Encoding::Protobuf {
                builder
                    .build_with_payload(protobuf_payload(&hello_request), PayloadEncoding::PROTOBUF)
                    .unwrap()
            } else {
                builder
                    .build_with_payload(
                        selected_payload_bytes(
                            &args,
                            sent_count.saturating_add(1).min(u64::from(u32::MAX)) as u32,
                        )?,
                        selected_payload_encoding(&args)?,
                    )
                    .map_err(|error| {
                        invalid_config(format!("failed to build request message: {error:?}"))
                    })?
            };
            debug!("Invoking URI {} with response URI {}", &sink, &source);
            info!("Sending Request message:\n{:?}", &request_msg);

            Ok(request_msg)
        },
    )
    .await?;

    if args.send_count > 0 {
        info!("Completed bounded send run: sent_count={sent_count}");
        service_response_listener
            .wait_for_response(args.timeout_ms)
            .await?;
    }

    Ok(())
}

async fn run_selected_wire_client(
    args: &Args,
    route_family: RouteFamily,
    uentity: u32,
    uversion: u8,
    target_uentity: u32,
    target_uversion: u8,
    target_resource: u16,
) -> Result<(), UStatus> {
    let local_uri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let source = cli::build_uuri(&args.uauthority, uentity, uversion, args.resource_id()?)?;
    let sink = cli::build_uuri(
        &args.target_authority,
        target_uentity,
        target_uversion,
        target_resource,
    )?;
    let response_source_filter = sink.clone();
    let response_sink_filter = source.clone();
    let zenoh_config = zenoh_config_from_args(args)?;

    if route_family == RouteFamily::CopyMinimized {
        let alignment = if args.encoding == Encoding::Native {
            native_payload_alignment()
        } else {
            args.payload_alignment
        };
        let catalog = [
            common::endpoint_config::prepared_address(
                &source,
                Some(&sink),
                selected_payload_bytes(args, 1)?.len(),
                alignment,
                true,
            ),
            common::endpoint_config::prepared_address(
                &response_source_filter,
                Some(&response_sink_filter),
                0,
                alignment,
                false,
            ),
        ];
        let core = zenoh_core(
            zenoh_config,
            &local_uri.to_string(),
            &catalog,
            ZenohPreparedConfig::default(),
        )
        .await?;
        macro_rules! request {
            ($transport:expr) => {
                run_zero_copy_selected_client(
                    Arc::new($transport),
                    &catalog,
                    args,
                    source,
                    sink,
                    response_source_filter,
                    response_sink_filter,
                )
                .await
            };
        }
        return match args.encoding {
            Encoding::Native => {
                request!(core.into_stable_container_transport(args.native.agreement()?.clone()))
            }
            Encoding::Protobuf => request!(core.into_protobuf_transport()),
            Encoding::Xcdrv2 => request!(core.into_native_prefix_wire_transport(XcdrV2Wire)),
            Encoding::Arrow => request!(core.into_native_prefix_wire_transport(ArrowWire)),
            Encoding::Omgidl => request!(core.into_native_prefix_wire_transport(OmgIdlWire)),
        };
    }

    {
        let transport = common::backend::zenoh_owned_selected(
            zenoh_config,
            local_uri.to_string(),
            args.encoding.into(),
            &args.native,
        )
        .await?;
        run_owned_selected_client(
            &transport,
            args,
            source,
            sink,
            response_source_filter,
            response_sink_filter,
        )
        .await
    }
}

async fn run_owned_selected_client(
    transport: &Arc<dyn UOwnedTransport>,
    args: &Args,
    source: UUri,
    sink: UUri,
    response_source_filter: UUri,
    response_sink_filter: UUri,
) -> Result<(), UStatus> {
    let options = client_options(args, response_source_filter, response_sink_filter);
    common::rpc::owned(transport, &options, |sequence| {
        Ok((
            request_metadata(args, sink.clone(), source.clone())?,
            selected_payload_bytes(args, sequence)?,
        ))
    })
    .await
}

async fn run_zero_copy_selected_client<T>(
    transport: Arc<T>,
    catalog: &[PreparedAddress],
    args: &Args,
    source: UUri,
    sink: UUri,
    response_source_filter: UUri,
    response_sink_filter: UUri,
) -> Result<(), UStatus>
where
    T: PreparedEndpoint,
{
    let session = Arc::new(PreparedSession::new(transport, catalog)?);
    let options = client_options(args, response_source_filter, response_sink_filter);
    common::rpc::native(&session, &options, |sequence| {
        Ok((
            request_metadata(args, sink.clone(), source.clone())?,
            selected_payload_bytes(args, sequence)?,
        ))
    })
    .await
}

fn client_options(
    args: &Args,
    response_source: UUri,
    response_sink: UUri,
) -> common::rpc::ClientOptions {
    common::rpc::ClientOptions {
        response_source,
        response_sink,
        timeout_ms: args.timeout_ms,
        send_count: args.send_count,
        interval: Duration::from_millis(args.send_interval_ms),
        receive_settle: Duration::from_millis(RESPONSE_LISTENER_SETTLE_MS),
        alignment: if args.encoding == Encoding::Native {
            native_payload_alignment()
        } else {
            args.payload_alignment
        },
        native: (args.encoding == Encoding::Native).then(|| args.native.clone()),
    }
}

fn zenoh_config_from_args(args: &Args) -> Result<Config, UStatus> {
    if let Some(path) = &args.zenoh_config {
        return Config::from_file(path).map_err(|error| {
            invalid_config(format!("failed to load Zenoh config {path}: {error:?}"))
        });
    }
    Ok(zenoh_config_from_endpoint(&args.endpoint))
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

fn request_metadata(args: &Args, sink: UUri, source: UUri) -> Result<UFrameMetadata, UStatus> {
    UFrameMetadata::request(
        sink,
        source,
        std::time::Duration::from_millis(args.timeout_ms.min(u32::MAX as u64)),
    )
    .with_payload_encoding(selected_payload_encoding(args)?)
    .build()
    .map_err(|error| invalid_config(format!("failed to build frame metadata: {error:?}")))
    .and_then(|metadata| {
        if args.encoding == Encoding::Native {
            args.native.stamp(metadata)
        } else {
            Ok(metadata)
        }
    })
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

fn selected_payload_bytes(args: &Args, sequence: u32) -> Result<Vec<u8>, UStatus> {
    match args.encoding {
        Encoding::Native => native_payload_bytes(NATIVE_PAYLOAD_MAGIC, sequence, &args.payload),
        Encoding::Protobuf => Ok(args.payload.as_bytes().to_vec()),
        Encoding::Xcdrv2 => xcdrv2_payload_bytes(sequence, args.uauthority.clone(), &args.payload),
        Encoding::Arrow => arrow_payload_bytes(sequence, &args.payload),
        Encoding::Omgidl => omgidl_payload_bytes(sequence, &args.payload),
    }
}

impl Args {
    fn resource_id(&self) -> Result<u16, UStatus> {
        cli::parse_u16_status("--resource", &self.resource)
    }
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
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
