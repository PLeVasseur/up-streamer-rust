/********************************************************************************
 * Copyright (c) 2025 Contributors to the Eclipse Foundation
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

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub(crate) up_streamer_config: UpStreamerConfig,
    pub(crate) streamer_uuri: StreamerUuri,
    pub(crate) usubscription_config: USubscriptionConfig,
    pub(crate) transports: Transports,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UpStreamerConfig {
    pub(crate) message_queue_size: u16,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct StreamerUuri {
    pub(crate) authority: String,
    pub(crate) ue_id: u32,
    pub(crate) ue_version_major: u8,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct USubscriptionConfig {
    #[serde(default)]
    pub(crate) mode: SubscriptionProviderMode,
    pub(crate) file_path: String,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionProviderMode {
    #[default]
    StaticFile,
    LiveUsubscription,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Transports {
    pub(crate) zenoh: ZenohTransport,
    pub(crate) mqtt: MqttTransport,
    #[serde(default)]
    pub(crate) iceoryx2: Option<Iceoryx2Transport>,
    #[serde(default)]
    pub(crate) lola: Option<LolaTransport>,
    /// R3A: classic vSomeIP endpoints (Tier 1 of the vSomeIP plan).
    #[serde(default)]
    pub(crate) vsomeip: Option<VsomeipTransport>,
    #[serde(default)]
    pub(crate) dds: Option<DdsTransport>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DdsTransport {
    pub(crate) domain_id: i32,
    pub(crate) origin_id: String,
    #[serde(default)]
    pub(crate) qos: DdsQosConfig,
    #[serde(default = "default_dds_history_depth")]
    pub(crate) history_depth: u32,
    #[serde(default)]
    pub(crate) readiness: DdsReadinessConfig,
    #[serde(default)]
    pub(crate) endpoints: Vec<EndpointConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DdsQosConfig {
    #[serde(default)]
    pub(crate) reliability: DdsReliability,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DdsReliability {
    #[default]
    Reliable,
    BestEffort,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DdsReadinessConfig {
    #[serde(default = "default_dds_required_matched_readers")]
    pub(crate) required_matched_readers: usize,
    #[serde(default = "default_dds_readiness_timeout_ms")]
    pub(crate) timeout_ms: u64,
}

impl Default for DdsReadinessConfig {
    fn default() -> Self {
        Self {
            required_matched_readers: default_dds_required_matched_readers(),
            timeout_ms: default_dds_readiness_timeout_ms(),
        }
    }
}

const fn default_dds_history_depth() -> u32 {
    32
}

const fn default_dds_required_matched_readers() -> usize {
    1
}

const fn default_dds_readiness_timeout_ms() -> u64 {
    5_000
}

/// R3A: classic vSomeIP transport section. `config_file` is the vsomeip JSON
/// (applications/services/routing) the orchestrator generates per row;
/// `remote_authority` names the peer authority per the SOME/IP binding.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct VsomeipTransport {
    pub(crate) config_file: String,
    pub(crate) remote_authority: String,
    #[serde(default)]
    pub(crate) endpoints: Vec<EndpointConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ZenohTransport {
    pub(crate) config_file: String,
    pub(crate) endpoints: Vec<EndpointConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MqttTransport {
    pub(crate) config_file: String,
    pub(crate) endpoints: Vec<EndpointConfig>,
    #[serde(skip)]
    pub(crate) mqtt_details: Option<MqttConfigDetails>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Iceoryx2Transport {
    pub(crate) endpoints: Vec<EndpointConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LolaTransport {
    pub(crate) endpoints: Vec<EndpointConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    #[default]
    Owned,
    OwnedFrame,
    CopyMinimized,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    pub(crate) authority: String,
    pub(crate) endpoint: String,
    #[serde(default)]
    pub(crate) forwarding: Vec<String>,
    #[serde(default)]
    pub(crate) forwarding_routes: Vec<ForwardingRouteConfig>,
    #[serde(default)]
    pub(crate) routing_mode: RoutingMode,
    #[serde(default)]
    pub(crate) copy_minimized_payload_alignment: Option<usize>,
    #[serde(default)]
    pub(crate) zenoh_config_file: Option<String>,
    #[serde(default)]
    pub(crate) zenoh_client_config_file: Option<String>,
    #[serde(default)]
    pub(crate) lola_instance_specifier: Option<String>,
    #[serde(default)]
    pub(crate) lola_service_type: Option<String>,
    #[serde(default)]
    pub(crate) lola_event_name: Option<String>,
    #[serde(default)]
    pub(crate) lola_response_instance_specifier: Option<String>,
    #[serde(default)]
    pub(crate) lola_response_service_type: Option<String>,
    #[serde(default)]
    pub(crate) lola_response_event_name: Option<String>,
    #[serde(default)]
    pub(crate) lola_default_rx_channel: Option<String>,
    #[serde(default)]
    pub(crate) lola_sample_size: Option<usize>,
    #[serde(default)]
    pub(crate) lola_sample_alignment: Option<usize>,
    #[serde(default)]
    pub(crate) lola_max_samples: Option<usize>,
    #[serde(default)]
    pub(crate) lola_mw_com_config_file: Option<String>,
    #[serde(default)]
    pub(crate) lola_response_mw_com_config_file: Option<String>,
    /// Numeric payload-encoding convention for transports such as SOME/IP
    /// whose wire carries no payload-encoding identity.
    #[serde(default)]
    pub(crate) payload_encoding_id: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ForwardingRouteConfig {
    pub(crate) endpoint: String,
    #[serde(default)]
    pub(crate) wire_format: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MqttConfigDetails {
    pub(crate) hostname: String,
    pub(crate) port: u16,
    pub(crate) max_buffered_messages: i32,
    pub(crate) max_subscriptions: i32,
    pub(crate) session_expiry_interval: i32,
    pub(crate) username: String,
}

impl MqttTransport {
    pub fn load_mqtt_details(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let config_contents = std::fs::read_to_string(&self.config_file)?;
        self.mqtt_details = Some(json5::from_str(&config_contents)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config(route_fragment: &str) -> String {
        format!(
            r#"{{
                up_streamer_config: {{ message_queue_size: 4 }},
                streamer_uuri: {{ authority: "authority-streamer", ue_id: 1, ue_version_major: 1 }},
                usubscription_config: {{ mode: "static_file", file_path: "subscriptions.json" }},
                transports: {{
                    zenoh: {{
                        config_file: "ZENOH_CONFIG.json5",
                        endpoints: [{{
                            authority: "authority-a",
                            endpoint: "zenoh-zc",
                            routing_mode: "copy_minimized",
                            {route_fragment}
                        }}],
                    }},
                    mqtt: {{ config_file: "MQTT_CONFIG.json5", endpoints: [] }},
                }},
            }}"#
        )
    }

    #[test]
    fn forwarding_route_accepts_explicit_wire_format() {
        let config: Config = json5::from_str(&base_config(
            r#"forwarding_routes: [{ endpoint: "iceoryx2-zc", wire_format: "protobuf" }],"#,
        ))
        .expect("config parses");

        let endpoint = &config.transports.zenoh.endpoints[0];
        assert!(endpoint.forwarding.is_empty());
        assert_eq!(endpoint.forwarding_routes[0].endpoint, "iceoryx2-zc");
        assert_eq!(
            endpoint.forwarding_routes[0].wire_format.as_deref(),
            Some("protobuf")
        );
    }

    #[test]
    fn endpoint_accepts_owned_frame_routing_mode() {
        let config: Config = json5::from_str(
            r#"{
                up_streamer_config: { message_queue_size: 4 },
                streamer_uuri: { authority: "authority-streamer", ue_id: 1, ue_version_major: 1 },
                usubscription_config: { mode: "static_file", file_path: "subscriptions.json" },
                transports: {
                    zenoh: {
                        config_file: "ZENOH_CONFIG.json5",
                        endpoints: [{
                            authority: "authority-a",
                            endpoint: "zenoh-owned",
                            routing_mode: "owned_frame",
                            forwarding_routes: [{ endpoint: "iceoryx2-owned", wire_format: "xcdrv2" }],
                        }],
                    },
                    mqtt: { config_file: "MQTT_CONFIG.json5", endpoints: [] },
                },
            }"#,
        )
        .expect("config parses");

        assert_eq!(
            config.transports.zenoh.endpoints[0].routing_mode,
            RoutingMode::OwnedFrame
        );
        assert_eq!(
            config.transports.zenoh.endpoints[0].forwarding_routes[0]
                .wire_format
                .as_deref(),
            Some("xcdrv2")
        );
    }

    #[test]
    fn unknown_forwarding_route_field_is_rejected() {
        let result = json5::from_str::<Config>(&base_config(
            r#"forwarding_routes: [{ endpoint: "iceoryx2-zc", unexpected: "value" }],"#,
        ));

        assert!(result.is_err());
    }

    #[test]
    fn dds_config_accepts_domain_origin_qos_history_readiness_and_endpoints() {
        let config: Config = json5::from_str(
            r#"{
                up_streamer_config: { message_queue_size: 4 },
                streamer_uuri: { authority: "authority-streamer", ue_id: 1, ue_version_major: 1 },
                usubscription_config: { mode: "static_file", file_path: "subscriptions.json" },
                transports: {
                    zenoh: { config_file: "ZENOH_CONFIG.json5", endpoints: [] },
                    mqtt: { config_file: "MQTT_CONFIG.json5", endpoints: [] },
                    dds: {
                        domain_id: 91,
                        origin_id: "matrix-row-17-streamer",
                        qos: { reliability: "best_effort" },
                        history_depth: 12,
                        readiness: { required_matched_readers: 1, timeout_ms: 2500 },
                        endpoints: [{
                            authority: "authority-a",
                            endpoint: "dds-owned",
                            routing_mode: "owned_frame",
                            forwarding_routes: [{ endpoint: "dds-copy", wire_format: "arrow" }],
                        }],
                    },
                },
            }"#,
        )
        .expect("DDS config parses");

        let dds = config.transports.dds.expect("DDS transport present");
        assert_eq!(dds.domain_id, 91);
        assert_eq!(dds.origin_id, "matrix-row-17-streamer");
        assert_eq!(dds.qos.reliability, DdsReliability::BestEffort);
        assert_eq!(dds.history_depth, 12);
        assert_eq!(dds.readiness.required_matched_readers, 1);
        assert_eq!(dds.readiness.timeout_ms, 2500);
        assert_eq!(dds.endpoints.len(), 1);
    }
}
