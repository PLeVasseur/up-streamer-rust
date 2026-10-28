// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Publish uProtocol messages through an explicitly configured iceoryx2 endpoint.
use example_streamer_uses::common;

use clap::Parser;
use common::flow::{self, FlowConfig, FlowMode, FlowRole, FlowTransport, FlowWireFormat};
use common::payloads::NativeContext;
use up_rust::UStatus;

#[derive(Parser)]
#[command()]
struct Cli {
    /// Complete encoded sample capacity, including metadata and alignment slack.
    #[arg(long)]
    iceoryx2_publisher_initial_max_slice_len: Option<usize>,
    /// Publisher strategy: static, best_fit or power_of_two.
    #[arg(long)]
    iceoryx2_publisher_allocation_strategy:
        Option<configurable_streamer_wire_support::capacity::PublisherAllocationStrategy>,
    /// Native subscriber ceiling per service; values above one enable fan-out.
    #[arg(long)]
    iceoryx2_max_subscribers: Option<usize>,
    /// Maximum simultaneous native publisher TX loans.
    #[arg(long)]
    iceoryx2_publisher_max_loaned_samples: Option<usize>,
    /// Delegate static publisher backing prewarming to the owning binding.
    #[arg(long)]
    iceoryx2_publisher_prewarm: bool,
    #[arg(skip)]
    native: NativeContext,
    /// Controller-provisioned sustained workload over this configured endpoint.
    #[cfg(feature = "sustained-workload")]
    #[arg(long)]
    workload_plan: Option<std::path::PathBuf>,
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
    /// Optional native Zenoh pool size forwarded to the binding's public builder.
    #[arg(long)]
    zenoh_shm_segment_size: Option<usize>,
    /// Require actual matching-subscriber presence before a native Zenoh send.
    #[arg(long)]
    zenoh_wait_for_subscriber: bool,
    /// Optional binding receive-buffer/borrow limit using the configured role namespace.
    #[arg(long)]
    iceoryx2_receive_capacity: Option<usize>,
    /// Actual subscriber discovery requirement before owned or native iceoryx2 publication.
    #[arg(long, default_value_t = 0)]
    iceoryx2_min_subscribers: usize,
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
    let _ = tracing_subscriber::fmt::try_init();
    let mut cli = Cli::parse();
    let capacities = configurable_streamer_wire_support::capacity::Iceoryx2Capacity {
        publisher_initial_max_slice_len: cli.iceoryx2_publisher_initial_max_slice_len,
        publisher_allocation_strategy: cli.iceoryx2_publisher_allocation_strategy,
        max_subscribers: cli.iceoryx2_max_subscribers,
        publisher_max_loaned_samples: cli.iceoryx2_publisher_max_loaned_samples,
        publisher_prewarm: cli.iceoryx2_publisher_prewarm,
    };
    capacities.validate()?;
    if capacities.configured() && cli.transport != FlowTransport::Iceoryx2 {
        return Err(flow::invalid(
            "iceoryx2 capacity options require the iceoryx2 backend",
        ));
    }
    #[cfg(feature = "sustained-workload")]
    if cli.workload_plan.is_some()
        && (cli.mode != FlowMode::CopyMinimized
            || matches!(cli.role, FlowRole::NotifySender | FlowRole::NotifyReceiver))
    {
        return Err(flow::invalid(
            "sustained generic roles require native API pub/sub or RPC",
        ));
    }
    #[cfg(feature = "payload-contract-large-fixtures")]
    let sensor_fixture = cli
        .workload_plan
        .as_ref()
        .map(|path| common::workload::WorkerPlan::load(path))
        .transpose()?
        .and_then(|plan| plan.native_fixture);
    #[cfg(feature = "payload-contract-large-fixtures")]
    if sensor_fixture.is_some()
        && (cli.wire_format != FlowWireFormat::Native
            || !matches!(cli.role, FlowRole::PubSender | FlowRole::PubReceiver))
    {
        return Err(flow::invalid("sensor workload requires native pub/sub"));
    }
    #[cfg(feature = "payload-contract-large-fixtures")]
    {
        cli.native = match sensor_fixture {
            Some(fixture) => {
                example_streamer_uses::common::payloads::load_native_context_for_fixture(fixture)?
            }
            None => example_streamer_uses::common::payloads::load_native_context()?,
        };
    }
    #[cfg(not(feature = "payload-contract-large-fixtures"))]
    {
        cli.native = example_streamer_uses::common::payloads::load_native_context()?;
    }
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
        zenoh_wait_for_subscriber: cli.zenoh_wait_for_subscriber,
        #[cfg(feature = "sustained-workload")]
        workload_plan: cli.workload_plan.clone(),
    };
    common::endpoint::run(
        common::endpoint::Options {
            mode: cli.mode,
            transport: cli.transport,
            zenoh_config: cli.zenoh_config,
            zenoh_shm_segment_size: cli.zenoh_shm_segment_size,
            iceoryx2_capacity: capacities,
            iceoryx2_receive_capacity: cli.iceoryx2_receive_capacity,
            iceoryx2_min_subscribers: cli.iceoryx2_min_subscribers,
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
