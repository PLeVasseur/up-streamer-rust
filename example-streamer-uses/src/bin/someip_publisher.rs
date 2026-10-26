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

//! Publish classic uProtocol messages using a configured native SOME/IP application.
use example_streamer_uses::common;

use clap::Parser;
use common::cli;
use common::encoding::Encoding;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, trace, warn};
use up_rust::{PayloadEncoding, UCode, UMessageBuilder, UStatus, UTransport};

const DEFAULT_UAUTHORITY: &str = "authority-a";
const DEFAULT_UENTITY: &str = "0x5BA0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x8001";
const DEFAULT_REMOTE_AUTHORITY: &str = "authority-b";
const DEFAULT_VSOMEIP_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vsomeip-configs/someip_publisher.json"
);
const DEFAULT_UENTITY_NUM: u32 = 0x5BA0;
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"SPUB");

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(skip)]
    native: common::native::NativeContext,
    /// Authority for the local publisher identity and publish source URI
    #[arg(long, default_value = DEFAULT_UAUTHORITY)]
    uauthority: String,
    /// UEntity ID for local publisher identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UENTITY)]
    uentity: String,
    /// UEntity major version for local publisher identity (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_UVERSION)]
    uversion: String,
    /// Resource ID for publish source URI (decimal or 0x-prefixed hex)
    #[arg(long, default_value = DEFAULT_RESOURCE)]
    resource: String,
    /// Remote authority used by the SOME/IP transport bridge
    #[arg(long, default_value = DEFAULT_REMOTE_AUTHORITY)]
    remote_authority: String,
    /// Path to the vsomeip JSON configuration file
    #[arg(long, default_value = DEFAULT_VSOMEIP_CONFIG)]
    vsomeip_config: String,
    /// Number of publish messages to send before exiting (0 means run forever)
    #[arg(long, default_value_t = 0)]
    send_count: u64,
    /// Milliseconds to wait between publish sends
    #[arg(long, default_value_t = 1000)]
    send_interval_ms: u64,
    /// Payload encoding fixed by the SOME/IP topic convention.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Text payload used by non-Protobuf example messages.
    #[arg(long, default_value = "someip-publisher")]
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

    info!("Started someip_publisher");

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;

    let vsomeip_config = cli::canonicalize_cli_path("--vsomeip-config", &args.vsomeip_config)?;
    trace!("vsomeip_config: {vsomeip_config:?}");

    if uentity != DEFAULT_UENTITY_NUM {
        warn!(
            "--uentity override ({uentity:#X}) may conflict with application/service IDs in '{}' ; update the SOME/IP config accordingly",
            vsomeip_config.display()
        );
    }

    let publisher_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let payload_encoding = payload_encoding(&args)?;

    // There will be a single vsomeip_transport, as there is a connection into device and a streamer
    let publisher: Arc<dyn UTransport> = Arc::new(
        common::backend::someip(
            publisher_uuri,
            &args.remote_authority,
            &vsomeip_config,
            payload_encoding,
        )
        .unwrap(),
    );

    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;

    if args.send_count > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let sent_count = common::classic::send(
        publisher.as_ref(),
        (args.send_count > 0).then_some(args.send_count),
        Duration::from_millis(args.send_interval_ms),
        |sent_count| {
            let mut builder = UMessageBuilder::publish(source.clone());
            let publish_msg = if args.encoding == Encoding::Protobuf {
                builder
                    .build_with_payload(common::classic::timer_payload(), PayloadEncoding::PROTOBUF)
                    .unwrap()
            } else {
                let (payload, encoding) = selected_payload_parts(
                    &args,
                    sent_count.saturating_add(1).min(u64::from(u32::MAX)) as u32,
                )?;
                builder
                    .build_with_payload(payload, encoding)
                    .map_err(|error| {
                        invalid_config(format!("failed to build publish message: {error:?}"))
                    })?
            };
            info!("Sending Publish message:\n{publish_msg:?}");

            Ok(publish_msg)
        },
    )
    .await?;

    if args.send_count > 0 {
        info!("Completed bounded send run: sent_count={sent_count}");
        tokio::time::sleep(Duration::from_secs(1)).await;
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
