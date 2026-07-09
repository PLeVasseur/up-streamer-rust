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
use common::{native_message_payload_parts, xcdrv2_message_payload_parts, PublishReceiver};
use std::sync::Arc;
use std::thread;
use tracing::{trace, warn};
use up_rust::{PayloadEncoding, UListener, UStatus, UTransport};
use up_transport_vsomeip::{TransportConfig, UPTransportVsomeip};

const DEFAULT_UAUTHORITY: &str = "authority-b";
const DEFAULT_UENTITY: &str = "0x5BB0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x0";
const DEFAULT_SOURCE_AUTHORITY: &str = "authority-b";
const DEFAULT_SOURCE_UENTITY: &str = "0x3039";
const DEFAULT_SOURCE_UVERSION: &str = "0x1";
const DEFAULT_SOURCE_RESOURCE: &str = "0x8001";
const DEFAULT_REMOTE_AUTHORITY: &str = "authority-b";
const DEFAULT_VSOMEIP_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vsomeip-configs/someip_subscriber.json"
);
const DEFAULT_UENTITY_NUM: u32 = 0x5BB0;
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"SSUB");

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Encoding {
    Native,
    Protobuf,
    Xcdrv2,
}

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
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
    /// Remote authority used by the SOME/IP transport bridge
    #[arg(long, default_value = DEFAULT_REMOTE_AUTHORITY)]
    remote_authority: String,
    /// Path to the vsomeip JSON configuration file
    #[arg(long, default_value = DEFAULT_VSOMEIP_CONFIG)]
    vsomeip_config: String,
    /// Payload encoding fixed by the SOME/IP topic convention.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    let args = Args::parse();

    println!("mE_subscriber");

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

    let subscriber_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;
    let assumed_payload_encoding = payload_encoding(&args)?;

    // There will be a single vsomeip_transport, as there is a connection into device and a streamer
    // TODO: Add error handling if we fail to create a UPTransportVsomeip
    let subscriber: Arc<dyn UTransport> = Arc::new(
        UPTransportVsomeip::new_with_config_and_transport_config(
            subscriber_uuri,
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

    let publish_receiver: Arc<dyn UListener> = Arc::new(PublishReceiver);
    // TODO: Need to revisit how the vsomeip config file is used in non point-to-point cases
    subscriber
        .register_listener(&source_filter, None, publish_receiver.clone())
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
