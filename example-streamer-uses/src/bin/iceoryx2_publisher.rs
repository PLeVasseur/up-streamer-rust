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
use common::payloads::{
    arrow_payload_bytes, native_payload_alignment, native_payload_bytes, omgidl_payload_bytes,
    xcdrv2_payload_bytes, SelectedWireNativePayload,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use up_rust::selected_wire_user_api::{ProtobufWire, StableContainerWireFormat};
use up_rust::{
    PayloadEncoding, PayloadFormat, StableContainerPayload, UCode, UFrameMetadata, UFrameView,
    UMessage, UMessageBuilder, UOwnedFrame, UOwnedTransport, UStatus, UTxBuffer, UTxLoanSpec, UUri,
    UZeroCopyRxLease, UZeroCopyTransport,
};
#[cfg(feature = "iceoryx2-owned-frame")]
use up_transport_iceoryx2_rust::BenchmarkOwnedIceoryx2Core;
#[cfg(any(feature = "iceoryx2-zero-copy", feature = "iceoryx2-owned-frame"))]
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
#[cfg(feature = "lola-owned-frame")]
use up_transport_lola_rust::LolaOwnedCore;
#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
use up_transport_lola_rust::{LolaDefaultRxChannel, LolaTransportConfig, UTransportLola};
#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
use up_transport_zenoh::zenoh_config::Config as ZenohConfig;
#[cfg(feature = "zenoh-owned-frame")]
use up_transport_zenoh::ZenohOwnedCore;
#[cfg(feature = "zenoh-zero-copy")]
use up_transport_zenoh::ZenohZeroCopyCore;
use up_wire_arrow::ArrowWire;
use up_wire_omgidl::OmgIdlWire;
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use up_wire_xcdrv2::XcdrV2Wire;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FlowMode {
    CopyMinimized,
    OwnedFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FlowTransport {
    Zenoh,
    Iceoryx2,
    Lola,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FlowWireFormat {
    Native,
    Protobuf,
    Xcdrv2,
    Arrow,
    Omgidl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FlowRole {
    PubSender,
    PubReceiver,
    NotifySender,
    NotifyReceiver,
    RpcClient,
    RpcServer,
}

const NATIVE_FLOW_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"UPNF");

#[derive(Parser)]
#[command()]
struct Cli {
    #[arg(long = "route-family", value_enum, default_value = "owned-frame")]
    mode: FlowMode,
    #[arg(long, value_enum, default_value = "iceoryx2", hide = true)]
    transport: FlowTransport,
    #[arg(long = "encoding", value_enum, default_value = "protobuf")]
    wire_format: FlowWireFormat,
    #[arg(long, value_enum, default_value = "pub-sender", hide = true)]
    role: FlowRole,
    #[arg(long)]
    local_authority: String,
    #[arg(long)]
    peer_authority: String,
    #[arg(long, default_value_t = 0x5BA0)]
    ue_id: u32,
    #[arg(long, default_value_t = 1)]
    ue_version_major: u8,
    #[arg(long, default_value_t = 0x8001)]
    topic_resource_id: u16,
    #[arg(long, default_value_t = 0x1000)]
    method_resource_id: u16,
    #[arg(long, default_value = "iceoryx2-publisher")]
    payload: String,
    #[arg(long, default_value_t = 1)]
    payload_alignment: usize,
    #[arg(long, default_value_t = 1)]
    send_count: usize,
    #[arg(long, default_value_t = 200)]
    send_interval_ms: u64,
    #[arg(long, default_value_t = 5_000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 250)]
    rpc_response_delay_ms: u64,
    #[arg(long, default_value = "ZENOH_CONFIG.json5")]
    zenoh_config: String,
    #[arg(long)]
    lola_mw_com_config_file: Option<String>,
    #[arg(long)]
    lola_rpc_response_mw_com_config_file: Option<String>,
    #[arg(long, default_value = "/payload_flow")]
    lola_instance_specifier: String,
    #[arg(long)]
    lola_rpc_response_instance_specifier: Option<String>,
    #[arg(long, default_value = "payload_flow")]
    lola_service_type: String,
    #[arg(long)]
    lola_rpc_response_service_type: Option<String>,
    #[arg(long, default_value = "payload")]
    lola_event_name: String,
    #[arg(long)]
    lola_rpc_response_event_name: Option<String>,
    #[arg(long, default_value = "primary")]
    lola_default_rx_channel: String,
    #[arg(long, default_value_t = 4096)]
    lola_sample_size: usize,
    #[arg(long, default_value_t = 8)]
    lola_sample_alignment: usize,
    #[arg(long, default_value_t = 16)]
    lola_max_samples: usize,
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let cli = Cli::parse();
    match cli.mode {
        FlowMode::OwnedFrame => run_owned(&cli).await,
        FlowMode::CopyMinimized => run_zero_copy(&cli).await,
    }
}

async fn run_owned(cli: &Cli) -> Result<(), UStatus> {
    let transport = owned_transport(cli).await?;
    match cli.role {
        FlowRole::PubSender => {
            let payload = payload_bytes(cli)?;
            let metadata = publish_metadata(cli, cli.local_authority.as_str())?;
            send_owned_frame_repeated(&transport, metadata, &payload, cli).await?;
            println!("FLOW sent_payload_bytes={} role=pub_sender", payload.len());
        }
        FlowRole::PubReceiver => {
            println!("READY listener_registered");
            let payload = receive_owned_payload(
                &transport,
                &topic_uri(cli.peer_authority.as_str(), cli.topic_resource_id)?,
                None,
                cli.timeout_ms,
            )
            .await?;
            println!(
                "FLOW observed_payload_bytes={} role=pub_receiver",
                payload.len()
            );
        }
        FlowRole::NotifySender => {
            let payload = payload_bytes(cli)?;
            let metadata = notification_metadata(
                cli,
                cli.local_authority.as_str(),
                cli.peer_authority.as_str(),
            )?;
            send_owned_frame_repeated(&transport, metadata, &payload, cli).await?;
            println!(
                "FLOW sent_payload_bytes={} role=notify_sender",
                payload.len()
            );
        }
        FlowRole::NotifyReceiver => {
            println!("READY listener_registered");
            let source = topic_uri(cli.peer_authority.as_str(), cli.topic_resource_id)?;
            let sink = endpoint_uri(cli.local_authority.as_str())?;
            let payload =
                receive_owned_payload(&transport, &source, Some(&sink), cli.timeout_ms).await?;
            println!(
                "FLOW observed_payload_bytes={} role=notify_receiver",
                payload.len()
            );
        }
        FlowRole::RpcClient => {
            let payload = payload_bytes(cli)?;
            let metadata = request_metadata(
                cli,
                cli.local_authority.as_str(),
                cli.peer_authority.as_str(),
            )?;
            let source = method_uri(cli.peer_authority.as_str(), cli.method_resource_id)?;
            let sink = endpoint_uri(cli.local_authority.as_str())?;
            let response = if cli.send_count <= 1 {
                send_owned_frame(&transport, metadata, &payload).await?;
                receive_owned_payload(&transport, &source, Some(&sink), cli.timeout_ms).await?
            } else {
                let send_transport = transport.clone();
                let send_payload = payload.clone();
                let send_cli = cli_send_options(cli);
                let send_task = tokio::spawn(async move {
                    send_owned_frame_repeated_with_options(
                        &send_transport,
                        metadata,
                        &send_payload,
                        send_cli,
                    )
                    .await
                });
                let response =
                    receive_owned_payload(&transport, &source, Some(&sink), cli.timeout_ms).await;
                send_task.abort();
                let _ = send_task.await;
                response?
            };
            ensure_payload_matches(&payload, &response)?;
            println!(
                "FLOW observed_payload_bytes={} role=rpc_client",
                response.len()
            );
        }
        FlowRole::RpcServer => {
            println!("READY listener_registered");
            let source = endpoint_wildcard(cli.peer_authority.as_str())?;
            let sink = method_uri(cli.local_authority.as_str(), cli.method_resource_id)?;
            let request =
                receive_owned_frame(&transport, &source, Some(&sink), cli.timeout_ms).await?;
            let payload = request.payload_bytes().to_vec();
            let response_metadata = response_metadata(cli, request.metadata())?;
            tokio::time::sleep(Duration::from_millis(cli.rpc_response_delay_ms)).await;
            send_owned_frame_repeated(&transport, response_metadata, &payload, cli).await?;
            println!(
                "FLOW observed_payload_bytes={} role=rpc_server",
                payload.len()
            );
        }
    }
    Ok(())
}

async fn run_zero_copy(cli: &Cli) -> Result<(), UStatus> {
    match (cli.transport, cli.wire_format) {
        #[cfg(feature = "zenoh-zero-copy")]
        (FlowTransport::Zenoh, FlowWireFormat::Native) => {
            run_zero_copy_transport(
                Arc::new(
                    zenoh_zero_copy_core(cli)
                        .await?
                        .with_selected_wire(StableContainerWireFormat),
                ),
                cli,
            )
            .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        (FlowTransport::Zenoh, FlowWireFormat::Protobuf) => {
            run_zero_copy_transport(
                Arc::new(
                    zenoh_zero_copy_core(cli)
                        .await?
                        .with_selected_wire(ProtobufWire),
                ),
                cli,
            )
            .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        (FlowTransport::Zenoh, FlowWireFormat::Xcdrv2) => {
            run_zero_copy_transport(
                Arc::new(
                    zenoh_zero_copy_core(cli)
                        .await?
                        .with_selected_wire(XcdrV2Wire),
                ),
                cli,
            )
            .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        (FlowTransport::Zenoh, FlowWireFormat::Arrow) => {
            run_zero_copy_transport(Arc::new(zenoh_zero_copy_core(cli).await?.with_selected_wire(ArrowWire)), cli).await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        (FlowTransport::Zenoh, FlowWireFormat::Omgidl) => {
            run_zero_copy_transport(Arc::new(zenoh_zero_copy_core(cli).await?.with_selected_wire(OmgIdlWire)), cli).await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Native) => {
            run_zero_copy_transport(
                Arc::new(Iceoryx2PubSub::new().with_selected_wire(StableContainerWireFormat)),
                cli,
            )
            .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Protobuf) => {
            run_zero_copy_transport(
                Arc::new(Iceoryx2PubSub::new().with_selected_wire(ProtobufWire)),
                cli,
            )
            .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Xcdrv2) => {
            run_zero_copy_transport(
                Arc::new(Iceoryx2PubSub::new().with_selected_wire(XcdrV2Wire)),
                cli,
            )
            .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Arrow) => run_zero_copy_transport(
            Arc::new(Iceoryx2PubSub::new().with_selected_wire(ArrowWire)),
            cli,
        )
        .await,
        #[cfg(feature = "iceoryx2-zero-copy")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Omgidl) => run_zero_copy_transport(
            Arc::new(Iceoryx2PubSub::new().with_selected_wire(OmgIdlWire)),
            cli,
        )
        .await,
        #[cfg(feature = "lola-transport")]
        (FlowTransport::Lola, FlowWireFormat::Native) => {
            let core = lola_transport(cli)?.zero_copy_core();
            run_zero_copy_transport(
                Arc::new(core.with_selected_wire(StableContainerWireFormat)),
                cli,
            )
            .await
        }
        #[cfg(feature = "lola-transport")]
        (FlowTransport::Lola, FlowWireFormat::Protobuf) => {
            let core = lola_transport(cli)?.zero_copy_core();
            run_zero_copy_transport(Arc::new(core.with_selected_wire(ProtobufWire)), cli).await
        }
        #[cfg(feature = "lola-transport")]
        (FlowTransport::Lola, FlowWireFormat::Xcdrv2) => {
            let core = lola_transport(cli)?.zero_copy_core();
            run_zero_copy_transport(Arc::new(core.with_selected_wire(XcdrV2Wire)), cli).await
        }
        #[cfg(feature = "lola-transport")]
        (FlowTransport::Lola, FlowWireFormat::Arrow) => {
            let core = lola_transport(cli)?.zero_copy_core();
            run_zero_copy_transport(Arc::new(core.with_selected_wire(ArrowWire)), cli).await
        }
        #[cfg(feature = "lola-transport")]
        (FlowTransport::Lola, FlowWireFormat::Omgidl) => {
            let core = lola_transport(cli)?.zero_copy_core();
            run_zero_copy_transport(Arc::new(core.with_selected_wire(OmgIdlWire)), cli).await
        }
        #[cfg(not(all(
            feature = "zenoh-zero-copy",
            feature = "iceoryx2-zero-copy",
            feature = "lola-transport"
        )))]
        _ => Err(invalid_config(format!(
            "copy_minimized iceoryx2-publisher role for {:?}/{:?} is not available with this feature set",
            cli.transport, cli.wire_format
        ))),
    }
}

#[cfg_attr(
    not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )),
    allow(dead_code)
)]
async fn run_zero_copy_transport<T>(transport: Arc<T>, cli: &Cli) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
    T::Rx: UZeroCopyRxLease,
{
    match cli.role {
        FlowRole::PubSender => {
            let payload = payload_bytes(cli)?;
            let metadata = publish_metadata(cli, cli.local_authority.as_str())?;
            send_zero_copy_frame_repeated(&transport, metadata, &payload, cli).await?;
            println!("FLOW sent_payload_bytes={} role=pub_sender", payload.len());
        }
        FlowRole::PubReceiver => {
            println!("READY listener_registered");
            let payload = receive_zero_copy_payload(
                &transport,
                &topic_uri(cli.peer_authority.as_str(), cli.topic_resource_id)?,
                None,
                cli.timeout_ms,
            )
            .await?;
            println!(
                "FLOW observed_payload_bytes={} role=pub_receiver",
                payload.len()
            );
        }
        FlowRole::NotifySender => {
            let payload = payload_bytes(cli)?;
            let metadata = notification_metadata(
                cli,
                cli.local_authority.as_str(),
                cli.peer_authority.as_str(),
            )?;
            send_zero_copy_frame_repeated(&transport, metadata, &payload, cli).await?;
            println!(
                "FLOW sent_payload_bytes={} role=notify_sender",
                payload.len()
            );
        }
        FlowRole::NotifyReceiver => {
            println!("READY listener_registered");
            let source = topic_uri(cli.peer_authority.as_str(), cli.topic_resource_id)?;
            let sink = endpoint_uri(cli.local_authority.as_str())?;
            let payload =
                receive_zero_copy_payload(&transport, &source, Some(&sink), cli.timeout_ms).await?;
            println!(
                "FLOW observed_payload_bytes={} role=notify_receiver",
                payload.len()
            );
        }
        FlowRole::RpcClient => {
            let payload = payload_bytes(cli)?;
            let metadata = request_metadata(
                cli,
                cli.local_authority.as_str(),
                cli.peer_authority.as_str(),
            )?;
            let source = method_uri(cli.peer_authority.as_str(), cli.method_resource_id)?;
            let sink = endpoint_uri(cli.local_authority.as_str())?;
            let response = if cli.send_count <= 1 {
                send_zero_copy_frame(&transport, metadata, &payload, payload_alignment(cli))
                    .await?;
                receive_zero_copy_payload(&transport, &source, Some(&sink), cli.timeout_ms).await?
            } else {
                let send_transport = transport.clone();
                let send_payload = payload.clone();
                let send_cli = cli_send_options(cli);
                let payload_alignment = payload_alignment(cli);
                let send_task = tokio::spawn(async move {
                    send_zero_copy_frame_repeated_with_options(
                        &send_transport,
                        metadata,
                        &send_payload,
                        payload_alignment,
                        send_cli,
                    )
                    .await
                });
                let response =
                    receive_zero_copy_payload(&transport, &source, Some(&sink), cli.timeout_ms)
                        .await;
                send_task.abort();
                let _ = send_task.await;
                response?
            };
            ensure_payload_matches(&payload, &response)?;
            println!(
                "FLOW observed_payload_bytes={} role=rpc_client",
                response.len()
            );
        }
        FlowRole::RpcServer => {
            println!("READY listener_registered");
            let source = endpoint_wildcard(cli.peer_authority.as_str())?;
            let sink = method_uri(cli.local_authority.as_str(), cli.method_resource_id)?;
            let request =
                receive_zero_copy_frame(&transport, &source, Some(&sink), cli.timeout_ms).await?;
            let payload = request.try_contiguous_payload().unwrap_or(&[]).to_vec();
            let response_metadata = response_metadata(cli, request.metadata())?;
            tokio::time::sleep(Duration::from_millis(cli.rpc_response_delay_ms)).await;
            send_zero_copy_frame_repeated(&transport, response_metadata, &payload, cli).await?;
            println!(
                "FLOW observed_payload_bytes={} role=rpc_server",
                payload.len()
            );
        }
    }
    Ok(())
}

async fn owned_transport(cli: &Cli) -> Result<Arc<dyn UOwnedTransport>, UStatus> {
    match (cli.transport, cli.wire_format) {
        #[cfg(feature = "zenoh-owned-frame")]
        (FlowTransport::Zenoh, FlowWireFormat::Native) => Ok(Arc::new(
            zenoh_owned_core(cli)
                .await?
                .with_selected_wire(StableContainerWireFormat),
        )),
        #[cfg(feature = "zenoh-owned-frame")]
        (FlowTransport::Zenoh, FlowWireFormat::Protobuf) => Ok(Arc::new(
            zenoh_owned_core(cli)
                .await?
                .with_selected_wire(ProtobufWire),
        )),
        #[cfg(feature = "zenoh-owned-frame")]
        (FlowTransport::Zenoh, FlowWireFormat::Xcdrv2) => Ok(Arc::new(
            zenoh_owned_core(cli).await?.with_selected_wire(XcdrV2Wire),
        )),
        #[cfg(feature = "zenoh-owned-frame")]
        (FlowTransport::Zenoh, FlowWireFormat::Arrow) => Ok(Arc::new(
            zenoh_owned_core(cli).await?.with_selected_wire(ArrowWire),
        )),
        #[cfg(feature = "zenoh-owned-frame")]
        (FlowTransport::Zenoh, FlowWireFormat::Omgidl) => Ok(Arc::new(
            zenoh_owned_core(cli).await?.with_selected_wire(OmgIdlWire),
        )),
        #[cfg(feature = "iceoryx2-owned-frame")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Native) => Ok(Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::new())
                .with_selected_wire(StableContainerWireFormat),
        )),
        #[cfg(feature = "iceoryx2-owned-frame")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Protobuf) => Ok(Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::new()).with_selected_wire(ProtobufWire),
        )),
        #[cfg(feature = "iceoryx2-owned-frame")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Xcdrv2) => Ok(Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::new()).with_selected_wire(XcdrV2Wire),
        )),
        #[cfg(feature = "iceoryx2-owned-frame")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Arrow) => Ok(Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::new()).with_selected_wire(ArrowWire),
        )),
        #[cfg(feature = "iceoryx2-owned-frame")]
        (FlowTransport::Iceoryx2, FlowWireFormat::Omgidl) => Ok(Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::new()).with_selected_wire(OmgIdlWire),
        )),
        #[cfg(feature = "lola-owned-frame")]
        (FlowTransport::Lola, _) => lola_owned_transport(cli, lola_transport(cli)?),
        #[cfg(not(all(
            feature = "zenoh-owned-frame",
            feature = "iceoryx2-owned-frame",
            feature = "lola-owned-frame"
        )))]
        _ => Err(invalid_config(format!(
            "owned_frame iceoryx2-publisher role for {:?}/{:?} is not available with this feature set",
            cli.transport, cli.wire_format
        ))),
    }
}

#[cfg(feature = "lola-owned-frame")]
fn lola_owned_transport(
    cli: &Cli,
    transport: Arc<UTransportLola>,
) -> Result<Arc<dyn UOwnedTransport>, UStatus> {
    match cli.wire_format {
        FlowWireFormat::Native => Ok(Arc::new(
            LolaOwnedCore::new(transport.zero_copy_core())
                .with_selected_wire(StableContainerWireFormat),
        )),
        FlowWireFormat::Protobuf => Ok(Arc::new(
            LolaOwnedCore::new(transport.zero_copy_core()).with_selected_wire(ProtobufWire),
        )),
        FlowWireFormat::Xcdrv2 => Ok(Arc::new(
            LolaOwnedCore::new(transport.zero_copy_core()).with_selected_wire(XcdrV2Wire),
        )),
        FlowWireFormat::Arrow => Ok(Arc::new(
            LolaOwnedCore::new(transport.zero_copy_core()).with_selected_wire(ArrowWire),
        )),
        FlowWireFormat::Omgidl => Ok(Arc::new(
            LolaOwnedCore::new(transport.zero_copy_core()).with_selected_wire(OmgIdlWire),
        )),
    }
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn lola_response_transport_config(
    cli: &Cli,
    base: &LolaTransportConfig,
) -> Result<Option<LolaTransportConfig>, UStatus> {
    let Some(instance_specifier) = cli.lola_rpc_response_instance_specifier.clone() else {
        return Ok(None);
    };

    let service_type = cli.lola_rpc_response_service_type.clone().ok_or_else(|| {
        invalid_config("--lola-rpc-response-service-type is required with --lola-rpc-response-instance-specifier")
    })?;
    let event_name = cli.lola_rpc_response_event_name.clone().ok_or_else(|| {
        invalid_config("--lola-rpc-response-event-name is required with --lola-rpc-response-instance-specifier")
    })?;
    let response_mw_com_config_path = lola_response_mw_com_config_path(cli, base)?;

    Ok(Some(LolaTransportConfig {
        local_authority: cli.local_authority.clone(),
        instance_specifier,
        service_type,
        event_name,
        sample_size: base.sample_size,
        sample_alignment: base.sample_alignment,
        max_samples: base.max_samples,
        pull_mismatch_queue_capacity: base.pull_mismatch_queue_capacity,
        pull_mismatch_queue_full_policy: base.pull_mismatch_queue_full_policy,
        mw_com_config_path: Some(response_mw_com_config_path),
    }))
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn lola_response_mw_com_config_path(
    cli: &Cli,
    base: &LolaTransportConfig,
) -> Result<String, UStatus> {
    let response_path = cli
        .lola_rpc_response_mw_com_config_file
        .as_deref()
        .map(canonical_lola_mw_com_config_path)
        .transpose()?
        .unwrap_or_else(|| {
            base.mw_com_config_path
                .clone()
                .expect("base LoLa MW COM config path is required")
        });
    let base_path = base
        .mw_com_config_path
        .as_deref()
        .expect("base LoLa MW COM config path is required");
    if response_path != base_path {
        return Err(invalid_config(format!(
            "LoLa RPC response MW COM config {response_path} differs from primary config {base_path}; use one complete manifest per process"
        )));
    }
    Ok(response_path)
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn required_lola_mw_com_config_path(cli: &Cli) -> Result<String, UStatus> {
    let path = cli.lola_mw_com_config_file.as_deref().ok_or_else(|| {
        invalid_config("--lola-mw-com-config-file is required for LoLa iceoryx2-publisher roles")
    })?;
    canonical_lola_mw_com_config_path(path)
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn canonical_lola_mw_com_config_path(path: &str) -> Result<String, UStatus> {
    std::fs::canonicalize(path)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            invalid_config(format!(
                "LoLa MW COM config {path} could not be loaded: {error}"
            ))
        })
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn lola_transport_from_config(
    default_rx_channel: LolaDefaultRxChannel,
    config: LolaTransportConfig,
    response_config: Option<LolaTransportConfig>,
) -> Result<Arc<UTransportLola>, UStatus> {
    UTransportLola::build_with_response_channel_and_default_rx(
        config,
        response_config,
        default_rx_channel,
    )
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn lola_default_rx_channel(cli: &Cli) -> Result<LolaDefaultRxChannel, UStatus> {
    match cli.lola_default_rx_channel.as_str() {
        "primary" => Ok(LolaDefaultRxChannel::Primary),
        "response" => Ok(LolaDefaultRxChannel::Response),
        "both" => Ok(LolaDefaultRxChannel::Both),
        other => Err(invalid_config(format!(
            "unsupported --lola-default-rx-channel {other}"
        ))),
    }
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
fn lola_transport(cli: &Cli) -> Result<Arc<UTransportLola>, UStatus> {
    let config = LolaTransportConfig {
        local_authority: cli.local_authority.clone(),
        instance_specifier: cli.lola_instance_specifier.clone(),
        service_type: cli.lola_service_type.clone(),
        event_name: cli.lola_event_name.clone(),
        sample_size: cli.lola_sample_size,
        sample_alignment: cli.lola_sample_alignment,
        max_samples: cli.lola_max_samples,
        pull_mismatch_queue_capacity: LolaTransportConfig::DEFAULT_PULL_MISMATCH_QUEUE_CAPACITY,
        pull_mismatch_queue_full_policy:
            LolaTransportConfig::DEFAULT_PULL_MISMATCH_QUEUE_FULL_POLICY,
        mw_com_config_path: Some(required_lola_mw_com_config_path(cli)?),
    };
    let response_config = lola_response_transport_config(cli, &config)?;
    lola_transport_from_config(lola_default_rx_channel(cli)?, config, response_config)
}

#[cfg(feature = "zenoh-zero-copy")]
async fn zenoh_zero_copy_core(cli: &Cli) -> Result<ZenohZeroCopyCore, UStatus> {
    let config = ZenohConfig::from_file(&cli.zenoh_config).map_err(|error| {
        invalid_config(format!(
            "failed to load Zenoh config {}: {error:?}",
            cli.zenoh_config
        ))
    })?;
    ZenohZeroCopyCore::new(config, local_uri(cli)?.to_string()).await
}

#[cfg(feature = "zenoh-owned-frame")]
async fn zenoh_owned_core(cli: &Cli) -> Result<ZenohOwnedCore, UStatus> {
    let config = ZenohConfig::from_file(&cli.zenoh_config).map_err(|error| {
        invalid_config(format!(
            "failed to load Zenoh config {}: {error:?}",
            cli.zenoh_config
        ))
    })?;
    ZenohOwnedCore::new(config, local_uri(cli)?.to_string()).await
}

async fn send_owned_frame(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
) -> Result<(), UStatus> {
    transport
        .send_owned(
            UOwnedFrame::with_payload(metadata, payload.to_vec()).map_err(|error| {
                invalid_config(format!("failed to build owned frame: {error:?}"))
            })?,
        )
        .await
}

#[derive(Clone, Copy)]
struct SendOptions {
    count: usize,
    interval_ms: u64,
}

fn cli_send_options(cli: &Cli) -> SendOptions {
    SendOptions {
        count: cli.send_count.max(1),
        interval_ms: cli.send_interval_ms,
    }
}

async fn send_owned_frame_repeated(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
    cli: &Cli,
) -> Result<(), UStatus> {
    send_owned_frame_repeated_with_options(transport, metadata, payload, cli_send_options(cli))
        .await
}

async fn send_owned_frame_repeated_with_options(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
    options: SendOptions,
) -> Result<(), UStatus> {
    for attempt in 0..options.count {
        send_owned_frame(transport, metadata.clone(), payload).await?;
        if attempt + 1 < options.count {
            tokio::time::sleep(Duration::from_millis(options.interval_ms)).await;
        }
    }
    Ok(())
}

async fn receive_owned_payload(
    transport: &Arc<dyn UOwnedTransport>,
    source_filter: &UUri,
    sink_filter: Option<&UUri>,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus> {
    receive_owned_frame(transport, source_filter, sink_filter, timeout_ms)
        .await
        .map(|frame| frame.payload_bytes().to_vec())
}

async fn receive_owned_frame(
    transport: &Arc<dyn UOwnedTransport>,
    source_filter: &UUri,
    sink_filter: Option<&UUri>,
    timeout_ms: u64,
) -> Result<UOwnedFrame, UStatus> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for owned frame",
            ));
        }
        match tokio::time::timeout(
            remaining,
            transport.receive_owned(source_filter, sink_filter),
        )
        .await
        {
            Ok(Ok(frame)) => return Ok(frame),
            Ok(Err(error)) if error.get_code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
}

#[cfg_attr(
    not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )),
    allow(dead_code)
)]
async fn send_zero_copy_frame<T>(
    transport: &Arc<T>,
    metadata: UFrameMetadata,
    payload: &[u8],
    payload_alignment: usize,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
{
    let mut tx = transport
        .loan_tx(UTxLoanSpec::payload(
            metadata,
            payload.len(),
            payload_alignment,
        )?)
        .await?;
    tx.payload_mut().copy_from_slice(payload);
    transport.send_zero_copy(tx).await
}

#[cfg_attr(
    not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )),
    allow(dead_code)
)]
async fn send_zero_copy_frame_repeated<T>(
    transport: &Arc<T>,
    metadata: UFrameMetadata,
    payload: &[u8],
    cli: &Cli,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
{
    send_zero_copy_frame_repeated_with_options(
        transport,
        metadata,
        payload,
        payload_alignment(cli),
        cli_send_options(cli),
    )
    .await
}

#[cfg_attr(
    not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )),
    allow(dead_code)
)]
async fn send_zero_copy_frame_repeated_with_options<T>(
    transport: &Arc<T>,
    metadata: UFrameMetadata,
    payload: &[u8],
    payload_alignment: usize,
    options: SendOptions,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
{
    for attempt in 0..options.count {
        send_zero_copy_frame(transport, metadata.clone(), payload, payload_alignment).await?;
        if attempt + 1 < options.count {
            tokio::time::sleep(Duration::from_millis(options.interval_ms)).await;
        }
    }
    Ok(())
}

#[cfg_attr(
    not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )),
    allow(dead_code)
)]
async fn receive_zero_copy_payload<T>(
    transport: &Arc<T>,
    source_filter: &UUri,
    sink_filter: Option<&UUri>,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Rx: UZeroCopyRxLease,
{
    receive_zero_copy_frame(transport, source_filter, sink_filter, timeout_ms)
        .await
        .map(|frame| frame.try_contiguous_payload().unwrap_or(&[]).to_vec())
}

#[cfg_attr(
    not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )),
    allow(dead_code)
)]
async fn receive_zero_copy_frame<T>(
    transport: &Arc<T>,
    source_filter: &UUri,
    sink_filter: Option<&UUri>,
    timeout_ms: u64,
) -> Result<T::Rx, UStatus>
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
                "timed out waiting for zero-copy frame",
            ));
        }
        match tokio::time::timeout(
            remaining,
            transport.receive_zero_copy(source_filter, sink_filter),
        )
        .await
        {
            Ok(Ok(frame)) => return Ok(frame),
            Ok(Err(error)) if error.get_code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {}
        }
    }
}

fn publish_metadata(cli: &Cli, source_authority: &str) -> Result<UFrameMetadata, UStatus> {
    frame_metadata(
        cli,
        UMessageBuilder::publish(topic_uri(source_authority, cli.topic_resource_id)?)
            .build()
            .map_err(|error| {
                invalid_config(format!("failed to build publish metadata: {error:?}"))
            })?,
    )
}

fn notification_metadata(
    cli: &Cli,
    source_authority: &str,
    sink_authority: &str,
) -> Result<UFrameMetadata, UStatus> {
    frame_metadata(
        cli,
        UMessageBuilder::notification(
            topic_uri(source_authority, cli.topic_resource_id)?,
            endpoint_uri(sink_authority)?,
        )
        .build()
        .map_err(|error| {
            invalid_config(format!("failed to build notification metadata: {error:?}"))
        })?,
    )
}

fn request_metadata(
    cli: &Cli,
    source_authority: &str,
    sink_authority: &str,
) -> Result<UFrameMetadata, UStatus> {
    frame_metadata(
        cli,
        UMessageBuilder::request(
            method_uri(sink_authority, cli.method_resource_id)?,
            endpoint_uri(source_authority)?,
            cli.timeout_ms.min(u32::MAX as u64) as u32,
        )
        .build()
        .map_err(|error| invalid_config(format!("failed to build request metadata: {error:?}")))?,
    )
}

fn response_metadata(
    cli: &Cli,
    request_metadata: &UFrameMetadata,
) -> Result<UFrameMetadata, UStatus> {
    let source = request_metadata
        .sink()
        .cloned()
        .ok_or_else(|| invalid_config("request metadata missing sink"))?;
    let sink = request_metadata.source().clone();
    let mut builder = UFrameMetadata::response(source, sink, request_metadata.id().clone())
        .with_payload_encoding(payload_encoding(cli.wire_format));
    if let Some(priority) = request_metadata.priority() {
        builder = builder.with_priority(priority);
    }
    if let Some(ttl) = request_metadata.ttl() {
        builder = builder.with_ttl(ttl);
    }
    builder
        .build()
        .map_err(|error| invalid_config(format!("failed to build response metadata: {error:?}")))
}

fn frame_metadata(cli: &Cli, message: UMessage) -> Result<UFrameMetadata, UStatus> {
    up_rust::try_project_attributes_to_frame_metadata(
        message.attributes(),
        Some(payload_encoding(cli.wire_format)),
    )
    .map_err(|error| invalid_config(format!("failed to build frame metadata: {error:?}")))
}

fn payload_encoding(wire_format: FlowWireFormat) -> PayloadEncoding {
    match wire_format {
        FlowWireFormat::Native => StableContainerPayload::<SelectedWireNativePayload>::encoding(),
        FlowWireFormat::Protobuf => ProtobufWire::encoding(),
        #[cfg(any(
            feature = "zenoh-zero-copy",
            feature = "iceoryx2-zero-copy",
            feature = "lola-transport",
            feature = "zenoh-owned-frame",
            feature = "iceoryx2-owned-frame",
            feature = "lola-owned-frame"
        ))]
        FlowWireFormat::Xcdrv2 => XcdrV2Wire::encoding(),
        FlowWireFormat::Arrow => ArrowWire::encoding(),
        FlowWireFormat::Omgidl => OmgIdlWire::encoding(),
        #[cfg(not(any(
            feature = "zenoh-zero-copy",
            feature = "iceoryx2-zero-copy",
            feature = "lola-transport",
            feature = "zenoh-owned-frame",
            feature = "iceoryx2-owned-frame",
            feature = "lola-owned-frame"
        )))]
        FlowWireFormat::Xcdrv2 => PayloadEncoding::RAW,
    }
}

fn payload_bytes(cli: &Cli) -> Result<Vec<u8>, UStatus> {
    match cli.wire_format {
        FlowWireFormat::Native => native_payload_bytes(NATIVE_FLOW_PAYLOAD_MAGIC, 1, &cli.payload),
        #[cfg(any(
            feature = "zenoh-zero-copy",
            feature = "iceoryx2-zero-copy",
            feature = "lola-transport",
            feature = "zenoh-owned-frame",
            feature = "iceoryx2-owned-frame",
            feature = "lola-owned-frame"
        ))]
        FlowWireFormat::Xcdrv2 => {
            xcdrv2_payload_bytes(1, cli.local_authority.clone(), &cli.payload)
        }
        FlowWireFormat::Protobuf => Ok(cli.payload.as_bytes().to_vec()),
        FlowWireFormat::Arrow => arrow_payload_bytes(1, &cli.payload),
        FlowWireFormat::Omgidl => omgidl_payload_bytes(1, &cli.payload),
        #[cfg(not(any(
            feature = "zenoh-zero-copy",
            feature = "iceoryx2-zero-copy",
            feature = "lola-transport",
            feature = "zenoh-owned-frame",
            feature = "iceoryx2-owned-frame",
            feature = "lola-owned-frame"
        )))]
        FlowWireFormat::Xcdrv2 => Ok(cli.payload.as_bytes().to_vec()),
    }
}

fn payload_alignment(cli: &Cli) -> usize {
    match cli.wire_format {
        FlowWireFormat::Native => native_payload_alignment(),
        _ => cli.payload_alignment,
    }
}

fn local_uri(cli: &Cli) -> Result<UUri, UStatus> {
    UUri::try_from_parts(&cli.local_authority, cli.ue_id, cli.ue_version_major, 0)
        .map_err(|error| invalid_config(format!("invalid local URI: {error:?}")))
}

fn topic_uri(authority: &str, resource_id: u16) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, 0x5BA0, 1, resource_id)
        .map_err(|error| invalid_config(format!("invalid topic URI: {error:?}")))
}

fn endpoint_uri(authority: &str) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, 0x5BA0, 1, 0)
        .map_err(|error| invalid_config(format!("invalid endpoint URI: {error:?}")))
}

fn endpoint_wildcard(authority: &str) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, 0xFFFF_FFFF, 0xFF, 0xFFFF)
        .map_err(|error| invalid_config(format!("invalid wildcard URI: {error:?}")))
}

fn method_uri(authority: &str, resource_id: u16) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, 0x5BA0, 1, resource_id)
        .map_err(|error| invalid_config(format!("invalid method URI: {error:?}")))
}

fn ensure_payload_matches(expected: &[u8], actual: &[u8]) -> Result<(), UStatus> {
    if expected == actual {
        Ok(())
    } else {
        Err(UStatus::fail_with_code(
            UCode::Internal,
            format!(
                "response payload mismatch: expected {} bytes, observed {} bytes",
                expected.len(),
                actual.len()
            ),
        ))
    }
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
