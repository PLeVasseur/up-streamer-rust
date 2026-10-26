// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Reusable validation of physical configuration; role files construct the bindings.
#![allow(dead_code)]

use super::flow::invalid;
use up_rust::UStatus;

#[cfg(any(feature = "lola-transport", feature = "lola-owned-frame"))]
pub(crate) struct LolaOptions {
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
    pub(crate) fn configuration(
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
pub(crate) fn iceoryx2(
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
