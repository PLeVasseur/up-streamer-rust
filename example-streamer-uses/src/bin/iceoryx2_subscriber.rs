// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Receive uProtocol messages through an explicitly configured iceoryx2 endpoint.
mod common;

use clap::Parser;
use common::flow::{self, FlowConfig, FlowMode, FlowRole, FlowTransport, FlowWireFormat};
use common::payloads::NativeContext;
use up_rust::UStatus;

// Keep this endpoint's concrete wire setup visible; only the role flow is shared.
macro_rules! run_endpoint {
    ($core:expr, $config:expr, $run:ident) => {{
        use std::sync::Arc;
        use up_rust::UWithNativePrefixWire;
        let core = $core;
        let config = $config;
        match config.wire_format {
            FlowWireFormat::Native => {
                flow::$run(
                    Arc::new(
                        core.into_stable_container_transport(config.native.agreement()?.clone()),
                    ),
                    config,
                )
                .await
            }
            FlowWireFormat::Protobuf => {
                flow::$run(Arc::new(core.into_protobuf_transport()), config).await
            }
            FlowWireFormat::Xcdrv2 => {
                flow::$run(
                    Arc::new(core.into_native_prefix_wire_transport(up_wire_xcdrv2::XcdrV2Wire)),
                    config,
                )
                .await
            }
            FlowWireFormat::Arrow => {
                flow::$run(
                    Arc::new(core.into_native_prefix_wire_transport(up_wire_arrow::ArrowWire)),
                    config,
                )
                .await
            }
            FlowWireFormat::Omgidl => {
                flow::$run(
                    Arc::new(core.into_native_prefix_wire_transport(up_wire_omgidl::OmgIdlWire)),
                    config,
                )
                .await
            }
        }
    }};
}

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
    #[arg(skip)]
    native: NativeContext,
    #[arg(long = "route-family", value_enum, default_value = "owned-frame")]
    mode: FlowMode,
    #[arg(long, value_enum, default_value = "iceoryx2", hide = true)]
    transport: FlowTransport,
    #[arg(long = "encoding", value_enum, default_value = "protobuf")]
    wire_format: FlowWireFormat,
    #[arg(long, value_enum, default_value = "pub-receiver", hide = true)]
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
    #[arg(long, default_value = "iceoryx2-subscriber")]
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
    let capacities = configurable_streamer_wire_support::capacity::Iceoryx2Capacity {
        publisher_initial_max_slice_len: cli.iceoryx2_publisher_initial_max_slice_len,
        publisher_allocation_strategy: cli.iceoryx2_publisher_allocation_strategy,
        max_subscribers: cli.iceoryx2_max_subscribers,
        publisher_max_loaned_samples: cli.iceoryx2_publisher_max_loaned_samples,
    };
    capacities.validate()?;
    if capacities.configured() && cli.transport != FlowTransport::Iceoryx2 {
        return Err(flow::invalid(
            "iceoryx2 capacity options require the iceoryx2 backend",
        ));
    }
    cli.native = NativeContext::from_env()?;
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
        #[cfg(feature = "sustained-workload")]
        workload_plan: None,
    };
    match (cli.mode, cli.transport) {
        #[cfg(feature = "iceoryx2-zero-copy")]
        (FlowMode::CopyMinimized, FlowTransport::Iceoryx2) => {
            let core = up_transport_iceoryx2_rust::Iceoryx2PubSub::try_with_config(
                common::endpoint_config::iceoryx2(None, 0, &capacities)?,
            )?;
            run_endpoint!(core, &config, run_native)
        }
        #[cfg(feature = "iceoryx2-owned-frame")]
        (FlowMode::OwnedFrame, FlowTransport::Iceoryx2) => {
            let physical = up_transport_iceoryx2_rust::Iceoryx2PubSub::try_with_config(
                common::endpoint_config::iceoryx2(None, 0, &capacities)?,
            )?;
            let core = up_transport_iceoryx2_rust::BenchmarkOwnedIceoryx2Core::new(physical);
            run_endpoint!(core, &config, run_owned)
        }
        #[cfg(feature = "zenoh-zero-copy")]
        (FlowMode::CopyMinimized, FlowTransport::Zenoh) => {
            let physical = up_transport_zenoh::zenoh_config::Config::from_file(&cli.zenoh_config)
                .map_err(|error| {
                flow::invalid(format!(
                    "failed to load Zenoh config {}: {error:?}",
                    cli.zenoh_config
                ))
            })?;
            let core = up_transport_zenoh::ZenohZeroCopyCore::new(
                physical,
                config.local_endpoint()?.to_string(),
            )
            .await?;
            run_endpoint!(core, &config, run_native)
        }
        #[cfg(feature = "zenoh-owned-frame")]
        (FlowMode::OwnedFrame, FlowTransport::Zenoh) => {
            let physical = up_transport_zenoh::zenoh_config::Config::from_file(&cli.zenoh_config)
                .map_err(|error| {
                flow::invalid(format!(
                    "failed to load Zenoh config {}: {error:?}",
                    cli.zenoh_config
                ))
            })?;
            let core = up_transport_zenoh::ZenohOwnedCore::new(
                physical,
                config.local_endpoint()?.to_string(),
            )
            .await?;
            run_endpoint!(core, &config, run_owned)
        }
        #[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
        (mode, FlowTransport::Lola) => {
            let options = common::endpoint_config::LolaOptions {
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
            };
            let (primary, response, channel) = options.configuration(&cli.local_authority)?;
            let physical =
                up_transport_lola_rust::UTransportLola::build_with_response_channel_and_default_rx(
                    primary, response, channel,
                )?;
            match mode {
                #[cfg(feature = "lola-transport")]
                FlowMode::CopyMinimized => {
                    run_endpoint!(physical.zero_copy_core(), &config, run_native)
                }
                #[cfg(feature = "lola-owned-frame")]
                FlowMode::OwnedFrame => {
                    run_endpoint!(
                        up_transport_lola_rust::LolaOwnedCore::new(physical.zero_copy_core()),
                        &config,
                        run_owned
                    )
                }
                #[cfg(not(all(feature = "lola-transport", feature = "lola-owned-frame")))]
                _ => Err(flow::invalid("requested LoLa API is not enabled")),
            }
        }
        #[cfg(not(all(
            feature = "iceoryx2-zero-copy",
            feature = "iceoryx2-owned-frame",
            feature = "zenoh-zero-copy",
            feature = "zenoh-owned-frame",
            feature = "lola-transport",
            feature = "lola-owned-frame"
        )))]
        _ => Err(flow::invalid("requested endpoint API is not enabled")),
    }
}
