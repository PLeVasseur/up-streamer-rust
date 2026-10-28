// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Receive directed uProtocol notifications through a configured LoLa endpoint.
use clap::Parser;
use common::flow::{FlowConfig, FlowMode, FlowRole, FlowTransport, FlowWireFormat};
use common::payloads::NativeContext;
use example_streamer_uses::common;
use up_rust::UStatus;

// Concrete wire constructors stay with the endpoint; message flow is shared.
#[derive(Parser)]
#[command()]
struct Cli {
    #[arg(skip)]
    native: NativeContext,
    #[arg(long = "route-family", value_enum, default_value = "owned-frame")]
    mode: FlowMode,
    #[arg(long, value_enum, default_value = "lola", hide = true)]
    transport: FlowTransport,
    #[arg(long = "encoding", value_enum, default_value = "protobuf")]
    wire_format: FlowWireFormat,
    #[arg(long, value_enum, default_value = "notify-receiver", hide = true)]
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
    #[arg(long, default_value = "lola-notifyee")]
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
    let mut cli = Cli::parse();
    cli.native = example_streamer_uses::common::payloads::load_native_context()?;
    if cli.wire_format == FlowWireFormat::Native {
        cli.native.identity()?;
    }
    let config = FlowConfig {
        native: cli.native.clone(),
        wire_format: cli.wire_format,
        role: cli.role,
        local_authority: cli.local_authority.clone(),
        peer_authority: cli.peer_authority.clone(),
        ue_id: cli.ue_id,
        ue_version_major: cli.ue_version_major,
        topic_resource_id: cli.topic_resource_id,
        method_resource_id: cli.method_resource_id,
        payload: cli.payload.clone(),
        payload_alignment: cli.payload_alignment,
        send_count: cli.send_count,
        send_interval_ms: cli.send_interval_ms,
        timeout_ms: cli.timeout_ms,
        rpc_response_delay_ms: cli.rpc_response_delay_ms,
        zenoh_wait_for_subscriber: false,
        #[cfg(feature = "sustained-workload")]
        workload_plan: None,
    };
    common::endpoint::run(
        common::endpoint::Options {
            mode: cli.mode,
            transport: cli.transport,
            zenoh_config: cli.zenoh_config,
            zenoh_shm_segment_size: None,
            iceoryx2_capacity: Default::default(),
            iceoryx2_receive_capacity: None,
            iceoryx2_min_subscribers: 0,
            ready: false,
            #[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
            lola: common::endpoint_config::LolaOptions {
                mw_com_config_file: cli.lola_mw_com_config_file,
                response_mw_com_config_file: cli.lola_rpc_response_mw_com_config_file,
                instance_specifier: cli.lola_instance_specifier,
                response_instance_specifier: cli.lola_rpc_response_instance_specifier,
                service_type: cli.lola_service_type,
                response_service_type: cli.lola_rpc_response_service_type,
                event_name: cli.lola_event_name,
                response_event_name: cli.lola_rpc_response_event_name,
                default_rx_channel: cli.lola_default_rx_channel,
                sample_size: cli.lola_sample_size,
                sample_alignment: cli.lola_sample_alignment,
                max_samples: cli.lola_max_samples,
            },
        },
        &config,
    )
    .await
}
