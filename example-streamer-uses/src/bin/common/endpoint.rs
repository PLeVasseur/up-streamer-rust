// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Shared backend construction for selected-wire example roles.
use super::{
    endpoint_config,
    flow::{self, FlowConfig, FlowMode, FlowTransport, FlowWireFormat},
};
use configurable_streamer_wire_support::capacity::Iceoryx2Capacity;
use up_rust::UStatus;

pub struct Options {
    pub mode: FlowMode,
    pub transport: FlowTransport,
    pub zenoh_config: String,
    pub zenoh_shm_segment_size: Option<usize>,
    pub iceoryx2_capacity: Iceoryx2Capacity,
    pub iceoryx2_receive_capacity: Option<usize>,
    pub iceoryx2_min_subscribers: usize,
    pub ready: bool,
    #[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
    pub lola: endpoint_config::LolaOptions,
}
macro_rules! run_endpoint {
    ($core:expr, $config:expr, $run:ident $(, $ready:expr)?) => {{
        use std::sync::Arc;
        use up_rust::UWithNativePrefixWire;
        let core = $core;
        let config = $config;
        match config.wire_format {
            FlowWireFormat::Native => flow::$run(Arc::new(core.into_stable_container_transport(config.native.agreement()?.clone())), config $(, $ready)?).await,
            FlowWireFormat::Protobuf => flow::$run(Arc::new(core.into_protobuf_transport()), config $(, $ready)?).await,
            FlowWireFormat::Xcdrv2 => flow::$run(Arc::new(core.into_native_prefix_wire_transport(up_wire_xcdrv2::XcdrV2Wire)), config $(, $ready)?).await,
            FlowWireFormat::Arrow => flow::$run(Arc::new(core.into_native_prefix_wire_transport(up_wire_arrow::ArrowWire)), config $(, $ready)?).await,
            FlowWireFormat::Omgidl => flow::$run(Arc::new(core.into_native_prefix_wire_transport(up_wire_omgidl::OmgIdlWire)), config $(, $ready)?).await,
        }
    }};
}
macro_rules! invoke {
    ($core:expr, $config:expr, $ready:expr, $normal:ident, $marked:ident) => {{
        let core = $core;
        if $ready {
            run_endpoint!(core, $config, $marked, super::receive::zenoh_listener_ready)
        } else {
            run_endpoint!(core, $config, $normal)
        }
    }};
}
pub async fn run(options: Options, config: &FlowConfig) -> Result<(), UStatus> {
    match options.transport {
        FlowTransport::Iceoryx2 => iceoryx2(options, config).await,
        FlowTransport::Zenoh => zenoh(options, config).await,
        FlowTransport::Lola => lola(options, config).await,
    }
}
async fn iceoryx2(options: Options, config: &FlowConfig) -> Result<(), UStatus> {
    let _ = (&options, config);
    match options.mode {
        #[cfg(feature = "iceoryx2-zero-copy")]
        FlowMode::CopyMinimized => {
            let physical = endpoint_config::iceoryx2(
                options.iceoryx2_receive_capacity,
                options.iceoryx2_min_subscribers,
                &options.iceoryx2_capacity,
            )?;
            let core =
                endpoint_config::prepared_iceoryx2(physical, config, &options.iceoryx2_capacity)?;
            invoke!(
                core,
                config,
                options.ready,
                run_native,
                run_native_with_ready
            )
        }
        #[cfg(feature = "iceoryx2-owned-frame")]
        FlowMode::OwnedFrame => {
            let physical = up_transport_iceoryx2_rust::Iceoryx2PubSub::try_with_config(
                endpoint_config::iceoryx2(
                    options.iceoryx2_receive_capacity,
                    options.iceoryx2_min_subscribers,
                    &options.iceoryx2_capacity,
                )?,
            )?;
            let core = up_transport_iceoryx2_rust::BenchmarkOwnedIceoryx2Core::new(physical);
            invoke!(core, config, options.ready, run_owned, run_owned_with_ready)
        }
        #[cfg(not(all(feature = "iceoryx2-zero-copy", feature = "iceoryx2-owned-frame")))]
        _ => Err(flow::invalid("requested endpoint API is not enabled")),
    }
}
async fn zenoh(options: Options, config: &FlowConfig) -> Result<(), UStatus> {
    let _ = (&options, config);
    #[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
    let physical = || {
        up_transport_zenoh::zenoh_config::Config::from_file(&options.zenoh_config).map_err(
            |error| {
                flow::invalid(format!(
                    "failed to load Zenoh config {}: {error:?}",
                    options.zenoh_config
                ))
            },
        )
    };
    match options.mode {
        #[cfg(feature = "zenoh-zero-copy")]
        FlowMode::CopyMinimized => {
            let core = endpoint_config::prepared_zenoh(
                physical()?,
                config,
                options.zenoh_shm_segment_size,
            )
            .await?;
            invoke!(
                core,
                config,
                options.ready,
                run_native,
                run_native_with_ready
            )
        }
        #[cfg(feature = "zenoh-owned-frame")]
        FlowMode::OwnedFrame => {
            let core =
                super::backend::zenoh_owned(physical()?, config.local_endpoint()?.to_string())
                    .await?;
            invoke!(core, config, options.ready, run_owned, run_owned_with_ready)
        }
        #[cfg(not(all(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame")))]
        _ => Err(flow::invalid("requested endpoint API is not enabled")),
    }
}
async fn lola(options: Options, config: &FlowConfig) -> Result<(), UStatus> {
    #[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
    {
        let (primary, response, _channel) = options.lola.configuration(&config.local_authority)?;
        match options.mode {
            #[cfg(feature = "lola-transport")]
            FlowMode::CopyMinimized => {
                let core = endpoint_config::prepared_lola(&primary, response.as_ref(), config)?;
                invoke!(
                    core,
                    config,
                    options.ready,
                    run_native,
                    run_native_with_ready
                )
            }
            #[cfg(feature = "lola-owned-frame")]
            FlowMode::OwnedFrame => {
                let physical = up_transport_lola_rust::UTransportLola::build_with_response_channel_and_default_rx(primary, response, _channel)?;
                let core = up_transport_lola_rust::LolaOwnedCore::new(physical.zero_copy_core());
                invoke!(core, config, options.ready, run_owned, run_owned_with_ready)
            }
            #[cfg(not(all(feature = "lola-transport", feature = "lola-owned-frame")))]
            _ => Err(flow::invalid("requested LoLa API is not enabled")),
        }
    }
    #[cfg(not(any(feature = "lola-transport", feature = "lola-owned-frame")))]
    {
        let _ = (options, config);
        Err(flow::invalid("requested endpoint API is not enabled"))
    }
}
