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
    pub(crate) usubscription_config: USubscriptionConfig,
    pub(crate) endpoints: Vec<EndpointConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct UpStreamerConfig {
    pub(crate) message_queue_size: u16,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct USubscriptionConfig {
    pub(crate) file_path: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: TransportKind,
    #[serde(default)]
    pub(crate) zenoh_config_file: Option<String>,
    #[serde(default)]
    pub(crate) mqtt_broker_uri: Option<String>,
    #[serde(default)]
    pub(crate) mqtt_client_id: Option<String>,
    #[serde(default)]
    pub(crate) mqtt_mode: MqttMode,
    #[serde(default)]
    pub(crate) vsomeip_config_file: Option<String>,
    #[serde(default)]
    pub(crate) remote_authority: Option<String>,
    #[serde(default)]
    pub(crate) local_uentity: Option<u32>,
    #[serde(default)]
    pub(crate) local_uversion: Option<u8>,
    #[serde(default)]
    pub(crate) local_resource: Option<u16>,
    #[serde(default)]
    pub(crate) forwarding: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    ZenohOwned,
    Iceoryx2ZeroCopy,
    Mqtt5Owned,
    VsomeipOwned,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MqttMode {
    #[default]
    InVehicle,
    OffVehicle,
}
