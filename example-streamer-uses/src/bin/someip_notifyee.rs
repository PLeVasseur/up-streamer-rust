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

use clap::{Parser, ValueEnum};
use common::cli;
use common::{native_message_payload_parts, xcdrv2_message_payload_parts, PublishReceiver};
use std::sync::Arc;
use std::thread;
use tracing::{info, trace, warn};
use up_rust::{PayloadEncoding, UListener, UStatus, UTransport};
use up_transport_vsomeip::{TransportConfig, UPTransportVsomeip};

const DEFAULT_UAUTHORITY: &str = "authority-b";
const DEFAULT_UENTITY: &str = "0x5BB0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x0";
const DEFAULT_SOURCE_AUTHORITY: &str = "authority-b";
const DEFAULT_SOURCE_UENTITY: &str = "0x5BA0";
const DEFAULT_SOURCE_UVERSION: &str = "0x1";
const DEFAULT_SOURCE_RESOURCE: &str = "0x8000";
const DEFAULT_REMOTE_AUTHORITY: &str = "authority-b";
const DEFAULT_VSOMEIP_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vsomeip-configs/someip_notifyee.json"
);
const DEFAULT_UENTITY_NUM: u32 = 0x5BB0;
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
    #[arg(long, default_value = DEFAULT_SOURCE_AUTHORITY)]
    source_authority: String,
    #[arg(long, default_value = DEFAULT_SOURCE_UENTITY)]
    source_uentity: String,
    #[arg(long, default_value = DEFAULT_SOURCE_UVERSION)]
    source_uversion: String,
    #[arg(long, default_value = DEFAULT_SOURCE_RESOURCE)]
    source_resource: String,
    #[arg(long, default_value = DEFAULT_REMOTE_AUTHORITY)]
    remote_authority: String,
    #[arg(long, default_value = DEFAULT_VSOMEIP_CONFIG)]
    vsomeip_config: String,
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = Args::parse();
    info!("Started someip_notifyee");

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;
    let source_uentity = cli::parse_u32_status("--source-uentity", &args.source_uentity)?;
    let source_uversion = cli::parse_u8_status("--source-uversion", &args.source_uversion)?;
    let source_resource = cli::parse_u16_status("--source-resource", &args.source_resource)?;

    let vsomeip_config = cli::canonicalize_cli_path("--vsomeip-config", &args.vsomeip_config)?;
    trace!("vsomeip_config: {vsomeip_config:?}");

    if uentity != DEFAULT_UENTITY_NUM {
        warn!(
            "--uentity override ({uentity:#X}) may conflict with application/service IDs in '{}' ; update the SOME/IP config accordingly",
            vsomeip_config.display()
        );
    }

    let local_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;
    let assumed_payload_encoding = payload_encoding(&args)?;
    let notifyee: Arc<dyn UTransport> = Arc::new(
        UPTransportVsomeip::new_with_config_and_transport_config(
            local_uuri.clone(),
            &args.remote_authority,
            &vsomeip_config,
            None,
            TransportConfig::new(assumed_payload_encoding),
        )
        .unwrap(),
    );

    let source_filter = cli::build_uuri(
        &args.source_authority,
        source_uentity,
        source_uversion,
        source_resource,
    )?;
    let listener: Arc<dyn UListener> = Arc::new(PublishReceiver);
    notifyee
        .register_listener(&source_filter, Some(&local_uuri), listener)
        .await?;

    println!("READY listener_registered");
    loop {
        thread::park();
    }
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
