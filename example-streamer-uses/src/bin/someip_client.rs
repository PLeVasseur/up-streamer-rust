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

//! Send classic uProtocol RPC requests using a configured native SOME/IP application.
use example_streamer_uses::common;

use clap::Parser;
use common::cli;
use common::encoding::Encoding;
use common::{protobuf_payload, ServiceResponseListener};
use hello_world_protos::hello_world_service::HelloRequest;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, trace, warn};
use up_rust::{PayloadEncoding, UCode, UMessageBuilder, UStatus, UTransport};

const DEFAULT_UAUTHORITY: &str = "authority-a";
const DEFAULT_UENTITY: &str = "0x15678";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x0";

const DEFAULT_TARGET_AUTHORITY: &str = "authority-b";
const DEFAULT_TARGET_UENTITY: &str = "0x11236";
const DEFAULT_TARGET_UVERSION: &str = "0x1";
const DEFAULT_TARGET_RESOURCE: &str = "0x0421";

const DEFAULT_REMOTE_AUTHORITY: &str = "authority-b";
const DEFAULT_VSOMEIP_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vsomeip-configs/someip_client.json"
);
const DEFAULT_UENTITY_NUM: u32 = 0x0001_5678;

const REQUEST_TTL: u32 = 1000;
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"SCLI");

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(skip)]
    native: common::native::NativeContext,
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
    /// Remote authority used by the SOME/IP transport bridge
    #[arg(long, default_value = DEFAULT_REMOTE_AUTHORITY)]
    remote_authority: String,
    /// Path to the vsomeip JSON configuration file
    #[arg(long, default_value = DEFAULT_VSOMEIP_CONFIG)]
    vsomeip_config: String,
    /// Number of requests to send before exiting (0 means run forever)
    #[arg(long, default_value_t = 0)]
    send_count: u64,
    /// Milliseconds to wait between request sends
    #[arg(long, default_value_t = 1000)]
    send_interval_ms: u64,
    /// Milliseconds to wait for an observed response after bounded sends.
    #[arg(long, default_value_t = 5000)]
    timeout_ms: u64,
    /// Payload encoding fixed by the SOME/IP topic convention.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Text payload used by non-Protobuf example messages.
    #[arg(long, default_value = "someip-client")]
    payload: String,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    let mut args = Args::parse();
    args.native = common::native::NativeContext::load()?;
    if args.encoding == Encoding::Native {
        args.native.identity()?;
    }

    info!("Started someip_client");

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;
    let target_uentity = cli::parse_u32_status("--target-uentity", &args.target_uentity)?;
    let target_uversion = cli::parse_u8_status("--target-uversion", &args.target_uversion)?;
    let target_resource = cli::parse_u16_status("--target-resource", &args.target_resource)?;

    let vsomeip_config = cli::canonicalize_cli_path("--vsomeip-config", &args.vsomeip_config)?;
    trace!("vsomeip_config: {vsomeip_config:?}");

    if uentity != DEFAULT_UENTITY_NUM {
        warn!(
            "--uentity override ({uentity:#X}) may conflict with application/service IDs in '{}' ; update the SOME/IP config accordingly",
            vsomeip_config.display()
        );
    }

    let client_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let payload_encoding = payload_encoding(&args)?;

    // There will be a single vsomeip_transport, as there is a connection into device and a streamer
    // TODO: Add error handling if we fail to create a UPTransportVsomeip
    let client: Arc<dyn UTransport> = Arc::new(
        common::backend::someip(
            client_uuri,
            &args.remote_authority,
            &vsomeip_config,
            payload_encoding,
        )
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
            common::native::listener(&args.native, service_response_listener.clone()),
        )
        .await?;

    if args.send_count > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let sent_count = common::classic::send(
        client.as_ref(),
        (args.send_count > 0).then_some(args.send_count),
        Duration::from_millis(args.send_interval_ms),
        |sent_count| {
            let hello_request = HelloRequest {
                name: format!("me_client@i={sent_count}"),
                ..Default::default()
            };

            let mut builder = UMessageBuilder::request(sink.clone(), source.clone(), REQUEST_TTL);
            let request_msg = if args.encoding == Encoding::Protobuf {
                builder
                    .build_with_payload(protobuf_payload(&hello_request), PayloadEncoding::PROTOBUF)
                    .unwrap()
            } else {
                let (payload, encoding) = selected_payload_parts(
                    &args,
                    sent_count.saturating_add(1).min(u64::from(u32::MAX)) as u32,
                )?;
                builder
                    .build_with_payload(payload, encoding)
                    .map_err(|error| {
                        invalid_config(format!("failed to build request message: {error:?}"))
                    })?
            };
            info!("Sending Request message:\n{request_msg:?}");

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

fn payload_encoding(args: &Args) -> Result<PayloadEncoding, UStatus> {
    args.encoding.identity(&args.native)
}

fn selected_payload_parts(
    args: &Args,
    sequence: u32,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    args.encoding.payload_parts(
        &args.native,
        NATIVE_PAYLOAD_MAGIC,
        sequence,
        &args.uauthority,
        &args.payload,
    )
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
