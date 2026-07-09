/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
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

use chrono::{Local, Timelike};
use clap::{Parser, ValueEnum};
use common::cli;
use common::{native_message_payload_parts, protobuf_payload, xcdrv2_message_payload_parts};
use hello_world_protos::hello_world_topics::Timer;
use hello_world_protos::timeofday::TimeOfDay;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, trace, warn};
use up_rust::{PayloadEncoding, UCode, UMessageBuilder, UPayloadFormat, UStatus, UTransport};
use up_transport_vsomeip::{TransportConfig, UPTransportVsomeip};

const DEFAULT_UAUTHORITY: &str = "authority-a";
const DEFAULT_UENTITY: &str = "0x5BA0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x8000";
const DEFAULT_SINK_AUTHORITY: &str = "authority-b";
const DEFAULT_SINK_UENTITY: &str = "0x5BB0";
const DEFAULT_REMOTE_AUTHORITY: &str = "authority-b";
const DEFAULT_VSOMEIP_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vsomeip-configs/someip_notifier.json"
);
const DEFAULT_UENTITY_NUM: u32 = 0x5BA0;
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"SNTF");

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Encoding {
    Native,
    Protobuf,
    Xcdrv2,
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = DEFAULT_UAUTHORITY)]
    uauthority: String,
    #[arg(long, default_value = DEFAULT_UENTITY)]
    uentity: String,
    #[arg(long, default_value = DEFAULT_UVERSION)]
    uversion: String,
    #[arg(long, default_value = DEFAULT_RESOURCE)]
    resource: String,
    #[arg(long, default_value = DEFAULT_SINK_AUTHORITY)]
    sink_authority: String,
    #[arg(long, default_value = DEFAULT_SINK_UENTITY)]
    sink_uentity: String,
    #[arg(long, default_value = DEFAULT_REMOTE_AUTHORITY)]
    remote_authority: String,
    #[arg(long, default_value = DEFAULT_VSOMEIP_CONFIG)]
    vsomeip_config: String,
    #[arg(long, default_value_t = 0)]
    send_count: u64,
    #[arg(long, default_value_t = 1000)]
    send_interval_ms: u64,
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    #[arg(long, default_value = "someip-notifier")]
    payload: String,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = Args::parse();
    info!("Started someip_notifier");

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;
    let sink_uentity = cli::parse_u32_status("--sink-uentity", &args.sink_uentity)?;

    let vsomeip_config = cli::canonicalize_cli_path("--vsomeip-config", &args.vsomeip_config)?;
    trace!("vsomeip_config: {vsomeip_config:?}");

    if uentity != DEFAULT_UENTITY_NUM {
        warn!(
            "--uentity override ({uentity:#X}) must match the vSomeIP application id in '{}' for Notification source reconstruction",
            vsomeip_config.display()
        );
    }

    let local_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, 0)?;
    let assumed_payload_encoding = payload_encoding(&args)?;
    let notifier: Arc<dyn UTransport> = Arc::new(
        UPTransportVsomeip::new_with_config_and_transport_config(
            local_uuri,
            &args.remote_authority,
            &vsomeip_config,
            None,
            TransportConfig::new(assumed_payload_encoding),
        )
        .unwrap(),
    );

    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;
    let sink = cli::build_uuri(&args.sink_authority, sink_uentity, uversion, 0)?;

    if args.send_count > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let mut sent_count = 0_u64;
    loop {
        if args.send_count > 0 && sent_count >= args.send_count {
            info!("Completed bounded notification run: sent_count={sent_count}");
            break;
        }

        tokio::time::sleep(Duration::from_millis(args.send_interval_ms)).await;
        let now = Local::now();
        let notification = if args.encoding == Encoding::Protobuf {
            let timer_message = Timer {
                time: Some(TimeOfDay {
                    hours: now.hour() as i32,
                    minutes: now.minute() as i32,
                    seconds: now.second() as i32,
                    nanos: now.nanosecond() as i32,
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            };
            UMessageBuilder::notification(source.clone(), sink.clone())
                .build_with_payload(protobuf_payload(&timer_message), UPayloadFormat::Protobuf)
                .unwrap()
        } else {
            let (payload, encoding) = selected_payload_parts(&args, sent_count as u32 + 1)?;
            UMessageBuilder::notification(source.clone(), sink.clone())
                .build_with_payload_encoding(payload, encoding)
                .map_err(|error| {
                    invalid_config(format!("failed to build notification message: {error:?}"))
                })?
        };
        info!("Sending Notification message:\n{notification:?}");
        notifier.send(notification).await?;
        sent_count += 1;
    }

    if args.send_count > 0 {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}

fn payload_encoding(args: &Args) -> Result<PayloadEncoding, UStatus> {
    match args.encoding {
        Encoding::Native => {
            native_message_payload_parts(NATIVE_PAYLOAD_MAGIC, 0, "").map(|(_, encoding)| encoding)
        }
        Encoding::Protobuf => Ok(PayloadEncoding::PROTOBUF),
        Encoding::Xcdrv2 => xcdrv2_message_payload_parts(0, args.uauthority.clone(), "")
            .map(|(_, encoding)| encoding),
    }
}

fn selected_payload_parts(
    args: &Args,
    sequence: u32,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    match args.encoding {
        Encoding::Native => {
            native_message_payload_parts(NATIVE_PAYLOAD_MAGIC, sequence, &args.payload)
        }
        Encoding::Protobuf => unreachable!("protobuf is handled by the classic protobuf path"),
        Encoding::Xcdrv2 => {
            xcdrv2_message_payload_parts(sequence, args.uauthority.clone(), &args.payload)
        }
    }
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
