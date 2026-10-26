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

//! Publish classic uProtocol messages through a configured MQTT broker connection.
mod common;

use clap::Parser;
use common::cli;
use common::encoding::Encoding;
use common::native::NativeContext;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use up_rust::{PayloadEncoding, UCode, UMessageBuilder, UStatus, UTransport};
use up_transport_mqtt5::{Mqtt5Transport, Mqtt5TransportOptions, MqttClientOptions};

const DEFAULT_UAUTHORITY: &str = "authority-a";
const DEFAULT_UENTITY: &str = "0x5BA0";
const DEFAULT_UVERSION: &str = "0x1";
const DEFAULT_RESOURCE: &str = "0x8001";
const DEFAULT_BROKER_URI: &str = "localhost:1883";
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"MPUB");

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(skip)]
    native: NativeContext,
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
    /// MQTT broker URI in host:port format
    #[arg(long, default_value = DEFAULT_BROKER_URI)]
    broker_uri: String,
    /// Number of publish messages to send before exiting (0 means run forever)
    #[arg(long, default_value_t = 0)]
    send_count: u64,
    /// Milliseconds to wait between publish sends
    #[arg(long, default_value_t = 1000)]
    send_interval_ms: u64,
    /// Payload encoding for the classic UMessage payload.
    #[arg(long, value_enum, default_value = "protobuf")]
    encoding: Encoding,
    /// Text payload used by non-Protobuf example messages.
    #[arg(long, default_value = "mqtt-publisher")]
    payload: String,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    let mut args = Args::parse();
    args.native = NativeContext::load()?;
    if args.encoding == Encoding::Native {
        args.native.identity()?;
    }

    info!("Started mqtt_publisher.");

    let uentity = cli::parse_u32_status("--uentity", &args.uentity)?;
    let uversion = cli::parse_u8_status("--uversion", &args.uversion)?;
    let resource = cli::parse_u16_status("--resource", &args.resource)?;

    // This is the URI of the publisher entity
    let source = cli::build_uuri(&args.uauthority, uentity, uversion, resource)?;

    let mqtt_client_options = MqttClientOptions {
        broker_uri: args.broker_uri.clone(),
        ..Default::default()
    };
    let mqtt_transport_options = Mqtt5TransportOptions {
        mqtt_client_options,
        ..Default::default()
    };
    let mqtt5_transport =
        Mqtt5Transport::new(mqtt_transport_options, args.uauthority.to_string()).await?;
    mqtt5_transport.connect().await?;

    let publisher: Arc<dyn UTransport> = Arc::new(mqtt5_transport);

    let sent_count = common::classic::send(
        publisher.as_ref(),
        (args.send_count > 0).then_some(args.send_count),
        Duration::from_millis(args.send_interval_ms),
        |sent_count| {
            // Publish messages signed with the source URI
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

fn selected_payload_parts(
    args: &Args,
    sequence: u32,
) -> Result<(Vec<u8>, up_rust::PayloadEncoding), UStatus> {
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
