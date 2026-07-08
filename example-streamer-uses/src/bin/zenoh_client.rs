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
use common::payloads::{
    native_payload_alignment, native_payload_bytes, xcdrv2_payload_bytes, SelectedWireNativePayload,
};
use common::{protobuf_payload, ServiceResponseListener};
use hello_world_protos::hello_world_service::HelloRequest;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};
use up_rust::selected_wire_user_api::{ProtobufWire, StableContainerWireFormat};
use up_rust::{
    PayloadEncoding, PayloadFormat, StableContainerPayload, UCode, UFrameMetadata, UFrameView,
    UListener, UMessageBuilder, UOwnedFrame, UOwnedTransport, UPayloadFormat, UStatus, UTransport,
    UTxBuffer, UTxLoanSpec, UUri, UZeroCopyRxLease, UZeroCopyTransport,
};
use up_transport_zenoh::{
    zenoh_config::{Config, EndPoint},
    UPTransportZenoh, ZenohOwnedCore, ZenohZeroCopyCore,
};
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

    let args = Args::parse();

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
        UPTransportZenoh::new(zenoh_config, client_uuri.to_string())
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

    let service_response_listener: Arc<dyn UListener> = Arc::new(ServiceResponseListener);
    client
        .register_listener(&sink, Some(&source), service_response_listener)
        .await?;

    let mut i: u64 = 0;
    let mut sent_count: u64 = 0;
    loop {
        if args.send_count > 0 && sent_count >= args.send_count {
            info!("Completed bounded send run: sent_count={sent_count}");
            break;
        }

        tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;

        let hello_request = HelloRequest {
            name: format!("ue_client@i={}", i),
            ..Default::default()
        };
        i += 1;

        let mut builder = UMessageBuilder::request(sink.clone(), source.clone(), REQUEST_TTL);
        let request_msg = if args.encoding == Encoding::Protobuf {
            builder
                .build_with_payload(protobuf_payload(&hello_request), UPayloadFormat::Protobuf)
                .unwrap()
        } else {
            builder
                .build_with_payload_encoding(
                    selected_payload_bytes(&args, sent_count as u32 + 1)?,
                    selected_payload_encoding(args.encoding),
                )
                .map_err(|error| {
                    invalid_config(format!("failed to build request message: {error:?}"))
                })?
        };
        debug!("Invoking URI {} with response URI {}", &sink, &source);
        info!("Sending Request message:\n{:?}", &request_msg);

        client.send(request_msg).await?;
        sent_count += 1;
    }

    if args.send_count > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
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

    match (route_family, args.encoding) {
        (RouteFamily::OwnedFrame, Encoding::Native) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(StableContainerWireFormat),
            ) as Arc<dyn UOwnedTransport>;
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
        (RouteFamily::OwnedFrame, Encoding::Protobuf) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(ProtobufWire),
            ) as Arc<dyn UOwnedTransport>;
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
        (RouteFamily::OwnedFrame, Encoding::Xcdrv2) => {
            let transport = Arc::new(
                ZenohOwnedCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(XcdrV2Wire),
            ) as Arc<dyn UOwnedTransport>;
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
        (RouteFamily::CopyMinimized, Encoding::Native) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(StableContainerWireFormat),
            );
            run_zero_copy_selected_client(
                &transport,
                args,
                source,
                sink,
                response_source_filter,
                response_sink_filter,
            )
            .await
        }
        (RouteFamily::CopyMinimized, Encoding::Protobuf) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(ProtobufWire),
            );
            run_zero_copy_selected_client(
                &transport,
                args,
                source,
                sink,
                response_source_filter,
                response_sink_filter,
            )
            .await
        }
        (RouteFamily::CopyMinimized, Encoding::Xcdrv2) => {
            let transport = Arc::new(
                ZenohZeroCopyCore::new(zenoh_config, local_uri.to_string())
                    .await?
                    .with_selected_wire(XcdrV2Wire),
            );
            run_zero_copy_selected_client(
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
}

async fn run_owned_selected_client(
    transport: &Arc<dyn UOwnedTransport>,
    args: &Args,
    source: UUri,
    sink: UUri,
    response_source_filter: UUri,
    response_sink_filter: UUri,
) -> Result<(), UStatus> {
    let run_forever = args.send_count == 0;
    let attempt_count = args.send_count.max(1);
    let mut exchanges = 0_u64;

    loop {
        let sequence = exchanges.saturating_add(1).min(u32::MAX as u64) as u32;
        let payload = selected_payload_bytes(args, sequence)?;
        let metadata = request_metadata(args, sink.clone(), source.clone())?;
        let receive_task = tokio::spawn(receive_owned_payload(
            transport.clone(),
            response_source_filter.clone(),
            response_sink_filter.clone(),
            args.timeout_ms,
        ));
        tokio::time::sleep(Duration::from_millis(RESPONSE_LISTENER_SETTLE_MS)).await;
        for attempt in 0..attempt_count {
            if let Err(error) = transport
                .send_owned(
                    UOwnedFrame::with_payload(metadata.clone(), payload.clone()).map_err(
                        |error| {
                            invalid_config(format!(
                                "failed to build owned request frame: {error:?}"
                            ))
                        },
                    )?,
                )
                .await
            {
                receive_task.abort();
                let _ = receive_task.await;
                return Err(error);
            }
            if attempt + 1 < attempt_count {
                tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;
            }
        }
        let response = receive_task.await.map_err(receive_task_failed)??;
        ensure_payload_matches(&payload, &response)?;
        println!(
            "FLOW observed_payload_bytes={} role=rpc_client",
            response.len()
        );
        exchanges += 1;
        if run_forever {
            tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;
        } else {
            break;
        }
    }
    Ok(())
}

async fn run_zero_copy_selected_client<T>(
    transport: &Arc<T>,
    args: &Args,
    source: UUri,
    sink: UUri,
    response_source_filter: UUri,
    response_sink_filter: UUri,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
    T::Rx: UZeroCopyRxLease,
{
    let run_forever = args.send_count == 0;
    let attempt_count = args.send_count.max(1);
    let mut exchanges = 0_u64;

    loop {
        let sequence = exchanges.saturating_add(1).min(u32::MAX as u64) as u32;
        let payload = selected_payload_bytes(args, sequence)?;
        let metadata = request_metadata(args, sink.clone(), source.clone())?;
        let alignment = match args.encoding {
            Encoding::Native => native_payload_alignment(),
            _ => args.payload_alignment,
        };
        let receive_task = tokio::spawn(receive_zero_copy_payload(
            transport.clone(),
            response_source_filter.clone(),
            response_sink_filter.clone(),
            args.timeout_ms,
        ));
        tokio::time::sleep(Duration::from_millis(RESPONSE_LISTENER_SETTLE_MS)).await;
        for attempt in 0..attempt_count {
            let mut tx = match transport
                .loan_tx(UTxLoanSpec::payload(
                    metadata.clone(),
                    payload.len(),
                    alignment,
                )?)
                .await
            {
                Ok(tx) => tx,
                Err(error) => {
                    receive_task.abort();
                    let _ = receive_task.await;
                    return Err(error);
                }
            };
            tx.payload_mut().copy_from_slice(&payload);
            if let Err(error) = transport.send_zero_copy(tx).await {
                receive_task.abort();
                let _ = receive_task.await;
                return Err(error);
            }
            if attempt + 1 < attempt_count {
                tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;
            }
        }
        let response = receive_task.await.map_err(receive_task_failed)??;
        ensure_payload_matches(&payload, &response)?;
        println!(
            "FLOW observed_payload_bytes={} role=rpc_client",
            response.len()
        );
        exchanges += 1;
        if run_forever {
            tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;
        } else {
            break;
        }
    }
    Ok(())
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
    .with_payload_encoding(selected_payload_encoding(args.encoding))
    .build()
    .map_err(|error| invalid_config(format!("failed to build frame metadata: {error:?}")))
}

fn selected_payload_encoding(encoding: Encoding) -> PayloadEncoding {
    match encoding {
        Encoding::Native => StableContainerPayload::<SelectedWireNativePayload>::encoding(),
        Encoding::Protobuf => ProtobufWire::encoding(),
        Encoding::Xcdrv2 => XcdrV2Wire::encoding(),
    }
}

fn selected_payload_bytes(args: &Args, sequence: u32) -> Result<Vec<u8>, UStatus> {
    match args.encoding {
        Encoding::Native => native_payload_bytes(NATIVE_PAYLOAD_MAGIC, sequence, &args.payload),
        Encoding::Protobuf => Ok(args.payload.as_bytes().to_vec()),
        Encoding::Xcdrv2 => xcdrv2_payload_bytes(sequence, args.uauthority.clone(), &args.payload),
    }
}

async fn receive_owned_payload(
    transport: Arc<dyn UOwnedTransport>,
    source_filter: UUri,
    sink_filter: UUri,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for owned response frame",
            ));
        }
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(100)),
            transport.receive_owned(&source_filter, Some(&sink_filter)),
        )
        .await
        {
            Ok(Ok(frame)) => return Ok(frame.payload_bytes().to_vec()),
            Ok(Err(error)) if error.get_code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
}

async fn receive_zero_copy_payload<T>(
    transport: Arc<T>,
    source_filter: UUri,
    sink_filter: UUri,
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
                "timed out waiting for zero-copy response frame",
            ));
        }
        match tokio::time::timeout(
            remaining.min(Duration::from_millis(100)),
            transport.receive_zero_copy(&source_filter, Some(&sink_filter)),
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

fn receive_task_failed(error: tokio::task::JoinError) -> UStatus {
    UStatus::fail_with_code(
        UCode::Internal,
        format!("response receive task failed: {error}"),
    )
}

fn ensure_payload_matches(expected: &[u8], actual: &[u8]) -> Result<(), UStatus> {
    if expected == actual {
        Ok(())
    } else {
        Err(UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!(
                "response payload mismatch: expected {} bytes, observed {} bytes",
                expected.len(),
                actual.len()
            ),
        ))
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
