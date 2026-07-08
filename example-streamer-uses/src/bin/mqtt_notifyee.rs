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

use clap::Parser;
use common::cli;
use common::PublishReceiver;
use std::sync::Arc;
use std::thread;
use tracing::info;
use up_rust::{UListener, UStatus, UTransport};
use up_transport_mqtt5::{Mqtt5Transport, Mqtt5TransportOptions, MqttClientOptions};

const DEFAULT_UAUTHORITY: &str = "authority-b";
const DEFAULT_UENTITY: &str = "0x5BA0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x0";
const DEFAULT_SOURCE_AUTHORITY: &str = "authority-a";
const DEFAULT_SOURCE_UENTITY: &str = "0x5BA0";
const DEFAULT_SOURCE_UVERSION: &str = "0x1";
const DEFAULT_SOURCE_RESOURCE: &str = "0x8001";
const DEFAULT_BROKER_URI: &str = "localhost:1883";

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
    #[arg(long, default_value = DEFAULT_BROKER_URI)]
    broker_uri: String,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();
    let args = Args::parse();
    info!("Started mqtt_notifyee.");

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;
    let source_uentity = cli::parse_u32_status("--source-uentity", &args.source_uentity)?;
    let source_uversion = cli::parse_u8_status("--source-uversion", &args.source_uversion)?;
    let source_resource = cli::parse_u16_status("--source-resource", &args.source_resource)?;

    let local_uuri = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;
    let source_filter = cli::build_uuri(
        &args.source_authority,
        source_uentity,
        source_uversion,
        source_resource,
    )?;

    let mqtt_client_options = MqttClientOptions {
        broker_uri: args.broker_uri,
        ..Default::default()
    };
    let mqtt_transport_options = Mqtt5TransportOptions {
        mqtt_client_options,
        ..Default::default()
    };
    let mqtt5_transport =
        Mqtt5Transport::new(mqtt_transport_options, local_uuri.authority_name()).await?;
    mqtt5_transport.connect().await?;
    let notifyee: Arc<dyn UTransport> = Arc::new(mqtt5_transport);

    let listener: Arc<dyn UListener> = Arc::new(PublishReceiver);
    notifyee
        .register_listener(&source_filter, Some(&local_uuri), listener)
        .await?;
    println!("READY listener_registered");
    thread::park();
    Ok(())
}
