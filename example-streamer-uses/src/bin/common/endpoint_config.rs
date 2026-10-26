// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Reusable option validation and binding-owned prepared-core setup for role endpoints.

#[cfg(any(
    feature = "iceoryx2-zero-copy",
    feature = "iceoryx2-owned-frame",
    feature = "lola-transport",
    feature = "lola-owned-frame"
))]
use super::flow::invalid;
use up_rust::UStatus;

#[cfg(feature = "iceoryx2-zero-copy")]
pub fn prepared_iceoryx2(
    physical: up_transport_iceoryx2_rust::Iceoryx2PubSubConfig,
    config: &super::flow::FlowConfig,
    capacities: &configurable_streamer_wire_support::capacity::Iceoryx2Capacity,
) -> Result<std::sync::Arc<up_transport_iceoryx2_rust::Iceoryx2PreparedCore>, UStatus> {
    let catalog = prepared_catalog(config)?;
    configurable_streamer_wire_support::prepared::iceoryx2_core(
        physical, &catalog, 32, 4, capacities,
    )
}

#[cfg(feature = "zenoh-transport")]
pub async fn prepared_zenoh(
    physical: up_transport_zenoh::zenoh_config::Config,
    config: &super::flow::FlowConfig,
    segment_size: Option<usize>,
) -> Result<std::sync::Arc<up_transport_zenoh::ZenohPreparedCore>, UStatus> {
    configurable_streamer_wire_support::prepared::zenoh_core(
        physical,
        &config.local_endpoint()?.to_uri(false),
        &prepared_catalog(config)?,
        up_transport_zenoh::ZenohPreparedConfig {
            shm_segment_size: segment_size.unwrap_or(64 * 1024 * 1024),
            route_capacity: 32,
            registration_capacity: 4,
            allocation_observer: None,
        },
    )
    .await
}

#[cfg(feature = "lola-transport")]
pub fn prepared_lola(
    primary: &up_transport_lola_rust::LolaTransportConfig,
    response: Option<&up_transport_lola_rust::LolaTransportConfig>,
    config: &super::flow::FlowConfig,
) -> Result<std::sync::Arc<up_transport_lola_rust::LolaPreparedCore>, UStatus> {
    configurable_streamer_wire_support::prepared::lola_core(
        primary,
        response,
        &prepared_catalog(config)?,
        32,
        4,
        primary.max_samples,
    )
}

/// Exact role addresses and maximum logical extents chosen before native setup.
pub fn prepared_address(
    source: &up_rust::UUri,
    sink: Option<&up_rust::UUri>,
    maximum: usize,
    alignment: usize,
    transmit: bool,
) -> configurable_streamer_wire_support::prepared::PreparedAddress {
    configurable_streamer_wire_support::prepared::PreparedAddress {
        source: source.to_uri(false),
        sink: sink.map(|sink| sink.to_uri(false)),
        max_metadata_len: 3072,
        max_payload_len: maximum,
        payload_alignment: alignment,
        max_token_len: 2048,
        allow_traceparent: true,
        transmit,
    }
}

/// Exact role addresses and maximum logical extents chosen before native setup.
pub fn prepared_catalog(
    config: &super::flow::FlowConfig,
) -> Result<Vec<configurable_streamer_wire_support::prepared::PreparedAddress>, UStatus> {
    use super::flow::FlowRole;
    #[cfg(feature = "sustained-workload")]
    let maximum = match &config.workload_plan {
        Some(path) => super::workload::WorkerPlan::load(path)?.maximum_payload_bytes,
        None => config.payload(1, false)?.len(),
    };
    #[cfg(not(feature = "sustained-workload"))]
    let maximum = config.payload(1, false)?.len();
    let entry = |source: up_rust::UUri, sink: Option<up_rust::UUri>, transmit| {
        prepared_address(
            &source,
            sink.as_ref(),
            maximum,
            config.alignment(),
            transmit,
        )
    };
    let local = &config.local_authority;
    let peer = &config.peer_authority;
    Ok(match config.role {
        FlowRole::PubSender => vec![entry(
            config.uri(local, config.topic_resource_id)?,
            None,
            true,
        )],
        FlowRole::PubReceiver => vec![entry(
            config.uri(peer, config.topic_resource_id)?,
            None,
            false,
        )],
        FlowRole::NotifySender => vec![entry(
            config.uri(local, config.topic_resource_id)?,
            Some(config.uri(peer, 0)?),
            true,
        )],
        FlowRole::NotifyReceiver => vec![entry(
            config.uri(peer, config.topic_resource_id)?,
            Some(config.uri(local, 0)?),
            false,
        )],
        FlowRole::RpcClient => vec![
            entry(
                config.uri(local, 0)?,
                Some(config.uri(peer, config.method_resource_id)?),
                true,
            ),
            entry(
                config.uri(peer, config.method_resource_id)?,
                Some(config.uri(local, 0)?),
                false,
            ),
        ],
        FlowRole::RpcServer => vec![
            entry(
                config.uri(peer, 0)?,
                Some(config.uri(local, config.method_resource_id)?),
                false,
            ),
            entry(
                config.uri(local, config.method_resource_id)?,
                Some(config.uri(peer, 0)?),
                true,
            ),
        ],
    })
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
pub struct LolaOptions {
    pub mw_com_config_file: Option<String>,
    pub response_mw_com_config_file: Option<String>,
    pub instance_specifier: String,
    pub response_instance_specifier: Option<String>,
    pub service_type: String,
    pub response_service_type: Option<String>,
    pub event_name: String,
    pub response_event_name: Option<String>,
    pub default_rx_channel: String,
    pub sample_size: usize,
    pub sample_alignment: usize,
    pub max_samples: usize,
}

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
impl LolaOptions {
    pub fn configuration(
        &self,
        authority: &str,
    ) -> Result<
        (
            up_transport_lola_rust::LolaTransportConfig,
            Option<up_transport_lola_rust::LolaTransportConfig>,
            up_transport_lola_rust::LolaDefaultRxChannel,
        ),
        UStatus,
    > {
        use up_transport_lola_rust::{LolaDefaultRxChannel, LolaTransportConfig};
        let path = self
            .mw_com_config_file
            .as_deref()
            .ok_or_else(|| invalid("--lola-mw-com-config-file is required for LoLa roles"))?;
        let canonical = |path: &str| {
            std::fs::canonicalize(path)
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| {
                    invalid(format!(
                        "LoLa MW COM config {path} could not be loaded: {error}"
                    ))
                })
        };
        let path = canonical(path)?;
        let primary = LolaTransportConfig {
            local_authority: authority.into(),
            instance_specifier: self.instance_specifier.clone(),
            service_type: self.service_type.clone(),
            event_name: self.event_name.clone(),
            sample_size: self.sample_size,
            sample_alignment: self.sample_alignment,
            max_samples: self.max_samples,
            pull_mismatch_queue_capacity: LolaTransportConfig::DEFAULT_PULL_MISMATCH_QUEUE_CAPACITY,
            pull_mismatch_queue_full_policy:
                LolaTransportConfig::DEFAULT_PULL_MISMATCH_QUEUE_FULL_POLICY,
            mw_com_config_path: Some(path.clone()),
        };
        let response = if let Some(instance) = &self.response_instance_specifier {
            let service = self.response_service_type.clone().ok_or_else(|| invalid("--lola-rpc-response-service-type is required with --lola-rpc-response-instance-specifier"))?;
            let event = self.response_event_name.clone().ok_or_else(|| invalid("--lola-rpc-response-event-name is required with --lola-rpc-response-instance-specifier"))?;
            let response_path = self
                .response_mw_com_config_file
                .as_deref()
                .map(canonical)
                .transpose()?
                .unwrap_or_else(|| path.clone());
            if response_path != path {
                return Err(invalid(format!("LoLa RPC response MW COM config {response_path} differs from primary config {path}; use one complete manifest per process")));
            }
            Some(LolaTransportConfig {
                instance_specifier: instance.clone(),
                service_type: service,
                event_name: event,
                mw_com_config_path: Some(response_path),
                ..primary.clone()
            })
        } else {
            None
        };
        let channel = match self.default_rx_channel.as_str() {
            "primary" => LolaDefaultRxChannel::Primary,
            "response" => LolaDefaultRxChannel::Response,
            "both" => LolaDefaultRxChannel::Both,
            other => {
                return Err(invalid(format!(
                    "unsupported --lola-default-rx-channel {other}"
                )))
            }
        };
        Ok((primary, response, channel))
    }
}

#[cfg(any(feature = "iceoryx2-zero-copy", feature = "iceoryx2-owned-frame"))]
pub fn iceoryx2(
    receive_capacity: Option<usize>,
    min_subscribers: usize,
    capacities: &configurable_streamer_wire_support::capacity::Iceoryx2Capacity,
) -> Result<up_transport_iceoryx2_rust::Iceoryx2PubSubConfig, UStatus> {
    let mut config = up_transport_iceoryx2_rust::Iceoryx2PubSubConfig::default()
        .with_publisher_readiness(min_subscribers, std::time::Duration::from_secs(5));
    if receive_capacity.is_some() || capacities.configured() {
        if receive_capacity == Some(0) {
            return Err(invalid("native receive capacity must be nonzero"));
        }
        let root = std::env::var("UP_ICEORYX2_ROOT_PATH")
            .map_err(|_| invalid("native capacity override requires configured namespace root"))?;
        let prefix = std::env::var("UP_ICEORYX2_PREFIX").map_err(|_| {
            invalid("native capacity override requires configured namespace prefix")
        })?;
        config = config.with_namespace(&root, &prefix)?;
        if let Some(capacity) = receive_capacity {
            let native = config
                .iceoryx2_config
                .as_mut()
                .expect("explicit namespace configuration");
            native.defaults.publish_subscribe.subscriber_max_buffer_size = capacity;
            native
                .defaults
                .publish_subscribe
                .subscriber_max_borrowed_samples = capacity;
        }
    }
    capacities.apply(&mut config)?;
    Ok(config)
}
