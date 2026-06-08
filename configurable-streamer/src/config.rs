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
    pub(crate) routing_mode: RoutingMode,
    #[serde(default)]
    pub(crate) copy_minimized_payload_alignment: Option<usize>,
    #[serde(default)]
    pub(crate) lola_instance_specifier: Option<String>,
    #[serde(default)]
    pub(crate) lola_service_type: Option<String>,
    #[serde(default)]
    pub(crate) lola_event_name: Option<String>,
    #[serde(default)]
    pub(crate) lola_sample_size: Option<usize>,
    #[serde(default)]
    pub(crate) lola_sample_alignment: Option<usize>,
    #[serde(default)]
    pub(crate) lola_max_samples: Option<usize>,
    #[serde(default)]
    pub(crate) lola_mw_com_config_file: Option<String>,
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
