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

use crate::claims::{
    evaluate_claims, load_claims_for_scenario, split_claim_outcomes, ClaimCategory, ClaimKind,
    ClaimSpec, Thresholds,
};
use crate::env;
use crate::logs;
use crate::process::{run_shell_command, shell_escape, ManagedProcess, ProcessSpec};
use crate::report::{self, PhaseTiming, ProcessMetadata, ScenarioClassification, ScenarioReport};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{Args, ValueEnum};
use serde::Serialize;
use std::cmp::min;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const SCENARIO_IDS: [&str; 8] = [
    "smoke-zenoh-mqtt-rr-zenoh-client-mqtt-service",
    "smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service",
    "smoke-zenoh-mqtt-ps-zenoh-publisher-mqtt-subscriber",
    "smoke-zenoh-mqtt-ps-mqtt-publisher-zenoh-subscriber",
    "smoke-zenoh-someip-rr-zenoh-client-someip-service",
    "smoke-zenoh-someip-rr-someip-client-zenoh-service",
    "smoke-zenoh-someip-ps-zenoh-publisher-someip-subscriber",
    "smoke-zenoh-someip-ps-someip-publisher-zenoh-subscriber",
];

pub const MATRIX_SCENARIO_IDS: [&str; 15] = [
    "smoke-zenoh-mqtt-rr-zenoh-client-mqtt-service",
    "smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service",
    "smoke-zenoh-mqtt-ps-zenoh-publisher-mqtt-subscriber",
    "smoke-zenoh-mqtt-ps-mqtt-publisher-zenoh-subscriber",
    "smoke-zenoh-someip-rr-zenoh-client-someip-service",
    "smoke-zenoh-someip-rr-someip-client-zenoh-service",
    "smoke-zenoh-someip-ps-zenoh-publisher-someip-subscriber",
    "smoke-zenoh-someip-ps-someip-publisher-zenoh-subscriber",
    "smoke-zc-zenoh-shm-to-iceoryx2",
    "smoke-zc-iceoryx2-to-zenoh-shm",
    "smoke-zc-zenoh-shm-to-lola-bundled",
    "smoke-zc-lola-bundled-to-zenoh-shm",
    "smoke-zc-iceoryx2-to-lola-bundled",
    "smoke-zc-lola-bundled-to-iceoryx2",
    "smoke-zc-all-transports-bundled",
];

const NO_ARGS: &[&str] = &[];
const STREAMER_LOLA_BAZELISK_HELPER: &str = "scripts/ensure-lola-bazelisk.sh";
const STREAMER_LOLA_BAZELISK_CACHE_PATH: &str = ".cache/tools/bazelisk-v1.29.0-linux-amd64";
const ZERO_COPY_HARD_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MQTT_BROKER_URI: &str = "localhost:1883";
const MQTT_STREAMER_ENV: &[(&str, &str)] = &[(
    "RUST_LOG",
    "up_streamer=debug,up_transport_mqtt5=debug,configurable_streamer=debug",
)];
const ZERO_COPY_STREAMER_ENV: &[(&str, &str)] = &[(
    "RUST_LOG",
    "configurable_streamer=debug,up_streamer=debug,up_transport_zenoh=debug,up_transport_iceoryx2_rust=debug,up_transport_lola_rust=debug",
)];
const SOMEIP_STREAMER_ENV: &[(&str, &str)] = &[(
    "RUST_LOG",
    "up_transport_vsomeip=trace,up_streamer=debug,up_linux_streamer=debug,example_streamer_uses=debug",
)];
const ACTIVE_DEBUG_ENV: &[(&str, &str)] = &[("RUST_LOG", "info,example_streamer_uses=debug")];
const PASSIVE_INFO_ENV: &[(&str, &str)] = &[("RUST_LOG", "info,example_streamer_uses=debug")];

const PASSIVE_MQTT_SUBSCRIBER_ARGS_A: &[&str] = &[
    "--uauthority",
    "authority-a",
    "--uentity",
    "0x5678",
    "--uversion",
    "0x1",
    "--resource",
    "0x1234",
    "--source-authority",
    "authority-b",
    "--source-uentity",
    "0x3039",
    "--source-uversion",
    "0x1",
    "--source-resource",
    "0x8001",
    "--broker-uri",
    "localhost:1883",
];

const PASSIVE_ZENOH_SUBSCRIBER_ARGS_B: &[&str] = &[
    "--uauthority",
    "authority-b",
    "--uentity",
    "0x5678",
    "--uversion",
    "0x1",
    "--resource",
    "0x1234",
    "--source-authority",
    "authority-a",
    "--source-uentity",
    "0x5BA0",
    "--source-uversion",
    "0x1",
    "--source-resource",
    "0x8001",
];

const PASSIVE_ZENOH_SERVICE_ARGS_B_FROM_SOMEIP: &[&str] = &[
    "--uauthority",
    "authority-b",
    "--uentity",
    "0x11236",
    "--uversion",
    "0x1",
    "--resource",
    "0x0421",
];

const PASSIVE_ZENOH_SUBSCRIBER_ARGS_B_FROM_SOMEIP: &[&str] = &[
    "--uauthority",
    "authority-b",
    "--uentity",
    "0x5678",
    "--uversion",
    "0x1",
    "--resource",
    "0x1234",
    "--source-authority",
    "authority-a",
    "--source-uentity",
    "0x15BA0",
    "--source-uversion",
    "0x1",
    "--source-resource",
    "0x8001",
];

const PASSIVE_SOMEIP_SUBSCRIBER_ARGS_A: &[&str] = &[
    "--uauthority",
    "authority-a",
    "--uentity",
    "0x5678",
    "--uversion",
    "0x1",
    "--resource",
    "0x0",
    "--source-authority",
    "authority-b",
    "--source-uentity",
    "0x3039",
    "--source-uversion",
    "0x1",
    "--source-resource",
    "0x8001",
    "--remote-authority",
    "authority-b",
    "--vsomeip-config",
    "example-streamer-uses/vsomeip-configs/someip_client.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFamily {
    Mqtt,
    Someip,
    ZeroCopy,
}

impl TransportFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mqtt => "mqtt",
            Self::Someip => "someip",
            Self::ZeroCopy => "zero_copy",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessTemplate {
    pub name: &'static str,
    pub workdir: &'static str,
    pub binary: &'static str,
    pub args: &'static [&'static str],
    pub env: &'static [(&'static str, &'static str)],
    pub log_file: &'static str,
    pub readiness_marker: Option<&'static str>,
    pub readiness_timeout_secs: Option<u64>,
    pub bounded_sender: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScenarioTemplate {
    pub id: &'static str,
    pub transport_family: TransportFamily,
    pub build_commands: &'static [&'static str],
    pub required_paths: &'static [&'static str],
    pub stale_process_signatures: &'static [&'static str],
    pub requires_mqtt_broker: bool,
    pub requires_vsomeip_runtime: bool,
    pub streamer: ProcessTemplate,
    pub passive: ProcessTemplate,
    pub active: ProcessTemplate,
    pub hard_timeout_secs_default: u64,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum MqttBrokerMode {
    #[default]
    DockerCompose,
    External,
    Native,
}

impl MqttBrokerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DockerCompose => "docker-compose",
            Self::External => "external",
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ZeroCopyRouteTemplate {
    ingress: &'static str,
    ingress_authority: &'static str,
    egress: &'static str,
    egress_authority: &'static str,
    wire_format: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct ZeroCopyScenarioTemplate {
    id: &'static str,
    row_description: &'static str,
    config_file: &'static str,
    cargo_features: &'static str,
    build_command: &'static str,
    required_paths: &'static [&'static str],
    stale_process_signatures: &'static [&'static str],
    selected_route: Option<ZeroCopyRouteTemplate>,
    configured_routes: &'static [ZeroCopyRouteTemplate],
    requires_lola_bundled: bool,
    hard_timeout_secs_default: u64,
}

#[derive(Serialize)]
struct ZeroCopyMatrixRowArtifact {
    schema_version: &'static str,
    scenario_id: String,
    row_description: String,
    classification: ScenarioClassification,
    failure_reason: Option<String>,
    config_file: String,
    cargo_features: Vec<String>,
    selected_route: Option<ZeroCopyRouteArtifact>,
    configured_routes: Vec<ZeroCopyRouteArtifact>,
    payload_bytes: PayloadProbeArtifact,
    metadata: MetadataProbeArtifact,
    route_diagnostics: Vec<RouteDiagnosticArtifact>,
    listener_cleanup: ListenerCleanupArtifact,
    raw_logs: Vec<RawLogArtifact>,
    environment: ZeroCopyEnvironmentArtifact,
    dependency_sources: Vec<DependencySourceArtifact>,
}

#[derive(Serialize)]
struct ZeroCopyRouteArtifact {
    ingress: String,
    ingress_authority: String,
    egress: String,
    egress_authority: String,
    wire_format: String,
}

#[derive(Serialize)]
struct PayloadProbeArtifact {
    observed_payload_bytes: u64,
    probe: &'static str,
    note: &'static str,
}

#[derive(Serialize)]
struct MetadataProbeArtifact {
    observed_frame_metadata: bool,
    probe: &'static str,
    note: &'static str,
}

#[derive(Serialize)]
struct RouteDiagnosticArtifact {
    route_kind: &'static str,
    ingress: String,
    ingress_authority: String,
    egress: String,
    egress_authority: String,
    diagnostic_source: &'static str,
}

#[derive(Serialize)]
struct ListenerCleanupArtifact {
    teardown_phase_recorded: bool,
    process_exit_status: Option<i32>,
    cleanup_signal: &'static str,
    note: &'static str,
}

#[derive(Serialize)]
struct RawLogArtifact {
    name: String,
    path: String,
}

#[derive(Serialize)]
struct ZeroCopyEnvironmentArtifact {
    rust_log: String,
    mqtt_broker_required: bool,
    docker_compose_required: bool,
    lola_bundled_required: bool,
    lola_bridge_lib_dir: Option<String>,
    bazel: Option<String>,
}

#[derive(Serialize)]
struct DependencySourceArtifact {
    package: String,
    source: Option<String>,
    version: Option<String>,
}

const BUILD_MQTT_RR_ZENOH_CLIENT: &[&str] = &[
    "cargo build -p configurable-streamer",
    "cargo build -p example-streamer-uses --bin zenoh_client --features zenoh-transport",
    "cargo build -p example-streamer-uses --bin mqtt_server --features mqtt-transport",
];
const BUILD_MQTT_RR_MQTT_CLIENT: &[&str] = &[
    "cargo build -p configurable-streamer",
    "cargo build -p example-streamer-uses --bin mqtt_client --features mqtt-transport",
    "cargo build -p example-streamer-uses --bin zenoh_server --features zenoh-transport",
];
const BUILD_MQTT_PS_ZENOH_PUBLISHER: &[&str] = &[
    "cargo build -p configurable-streamer",
    "cargo build -p example-streamer-uses --bin zenoh_publisher --features zenoh-transport",
    "cargo build -p example-streamer-uses --bin mqtt_subscriber --features mqtt-transport",
];
const BUILD_MQTT_PS_MQTT_PUBLISHER: &[&str] = &[
    "cargo build -p configurable-streamer",
    "cargo build -p example-streamer-uses --bin mqtt_publisher --features mqtt-transport",
    "cargo build -p example-streamer-uses --bin zenoh_subscriber --features zenoh-transport",
];
const BUILD_SOMEIP_RR_ZENOH_CLIENT: &[&str] = &[
    "cargo build -p up-linux-streamer --bin zenoh_someip --features zenoh-transport,vsomeip-transport,bundled-vsomeip",
    "cargo build -p example-streamer-uses --bin zenoh_client --features zenoh-transport",
    "cargo build -p example-streamer-uses --bin someip_server --features vsomeip-transport,bundled-vsomeip",
];
const BUILD_SOMEIP_RR_SOMEIP_CLIENT: &[&str] = &[
    "cargo build -p up-linux-streamer --bin zenoh_someip --features zenoh-transport,vsomeip-transport,bundled-vsomeip",
    "cargo build -p example-streamer-uses --bin someip_client --features vsomeip-transport,bundled-vsomeip",
    "cargo build -p example-streamer-uses --bin zenoh_server --features zenoh-transport",
];
const BUILD_SOMEIP_PS_ZENOH_PUBLISHER: &[&str] = &[
    "cargo build -p up-linux-streamer --bin zenoh_someip --features zenoh-transport,vsomeip-transport,bundled-vsomeip",
    "cargo build -p example-streamer-uses --bin zenoh_publisher --features zenoh-transport",
    "cargo build -p example-streamer-uses --bin someip_subscriber --features vsomeip-transport,bundled-vsomeip",
];
const BUILD_SOMEIP_PS_SOMEIP_PUBLISHER: &[&str] = &[
    "cargo build -p up-linux-streamer --bin zenoh_someip --features zenoh-transport,vsomeip-transport,bundled-vsomeip",
    "cargo build -p example-streamer-uses --bin someip_publisher --features vsomeip-transport,bundled-vsomeip",
    "cargo build -p example-streamer-uses --bin zenoh_subscriber --features zenoh-transport",
];

const REQUIRED_MQTT_PATHS: &[&str] = &[
    "configurable-streamer/CONFIG.json5",
    "configurable-streamer/ZENOH_CONFIG.json5",
    "utils/mosquitto/docker-compose.yaml",
];
const REQUIRED_SOMEIP_PATHS: &[&str] = &[
    "example-streamer-implementations/DEFAULT_CONFIG.json5",
    "example-streamer-uses/vsomeip-configs/someip_client.json",
    "example-streamer-uses/vsomeip-configs/someip_publisher.json",
    "example-streamer-uses/vsomeip-configs/someip_server.json",
    "example-streamer-uses/vsomeip-configs/someip_subscriber.json",
];

const BUILD_ZC_ZENOH_ICEORYX2: &str = "cargo build -p configurable-streamer --features experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy";
const BUILD_ZC_ZENOH_LOLA: &str = "cargo build -p configurable-streamer --features experimental-copy-minimized-routing,zenoh-zero-copy,lola-transport";
const BUILD_ZC_ALL_TRANSPORTS: &str = "cargo build -p configurable-streamer --features experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy,lola-transport";

const REQUIRED_ZC_ZENOH_ICEORYX2_PATHS: &[&str] = &[
    "configurable-streamer/CONFIG_ZENOH_ICEORYX2_ZEROCOPY_EXAMPLE.json5",
    "configurable-streamer/CONFIG_ZEROCOPY_MISMATCH_NEGATIVE_EXAMPLE.json5",
    "configurable-streamer/ZENOH_CONFIG.json5",
    "configurable-streamer/subscription_data.json",
];
const REQUIRED_ZC_ZENOH_LOLA_PATHS: &[&str] = &[
    "configurable-streamer/CONFIG_LOLA_ZEROCOPY_EXAMPLE.json5",
    "configurable-streamer/CONFIG_ZEROCOPY_MISMATCH_NEGATIVE_EXAMPLE.json5",
    "configurable-streamer/ZENOH_CONFIG.json5",
    "configurable-streamer/subscription_data.json",
    "configurable-streamer/MW_COM_CONFIG_LOLA.json",
];
const REQUIRED_ZC_ALL_TRANSPORTS_PATHS: &[&str] = &[
    "configurable-streamer/CONFIG_ZEROCOPY_EXAMPLE.json5",
    "configurable-streamer/CONFIG_ZEROCOPY_MISMATCH_NEGATIVE_EXAMPLE.json5",
    "configurable-streamer/ZENOH_CONFIG.json5",
    "configurable-streamer/subscription_data.json",
    "configurable-streamer/MW_COM_CONFIG_LOLA.json",
];

const ROUTE_ZENOH_TO_ICEORYX2: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "zenoh-zc",
    ingress_authority: "authority-a",
    egress: "iceoryx2-zc",
    egress_authority: "authority-b",
    wire_format: "protobuf",
};
const ROUTE_ICEORYX2_TO_ZENOH: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "iceoryx2-zc",
    ingress_authority: "authority-b",
    egress: "zenoh-zc",
    egress_authority: "authority-a",
    wire_format: "protobuf",
};
const ROUTE_ZENOH_TO_LOLA: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "zenoh-zc",
    ingress_authority: "authority-a",
    egress: "lola-zc",
    egress_authority: "authority-b",
    wire_format: "protobuf",
};
const ROUTE_LOLA_TO_ZENOH: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "lola-zc",
    ingress_authority: "authority-b",
    egress: "zenoh-zc",
    egress_authority: "authority-a",
    wire_format: "protobuf",
};
const ROUTE_ALL_ZENOH_TO_LOLA: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "zenoh-zc",
    ingress_authority: "authority-a",
    egress: "lola-zc",
    egress_authority: "authority-c",
    wire_format: "protobuf",
};
const ROUTE_ALL_LOLA_TO_ZENOH: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "lola-zc",
    ingress_authority: "authority-c",
    egress: "zenoh-zc",
    egress_authority: "authority-a",
    wire_format: "protobuf",
};
const ROUTE_ICEORYX2_TO_LOLA: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "iceoryx2-zc",
    ingress_authority: "authority-b",
    egress: "lola-zc",
    egress_authority: "authority-c",
    wire_format: "protobuf",
};
const ROUTE_LOLA_TO_ICEORYX2: ZeroCopyRouteTemplate = ZeroCopyRouteTemplate {
    ingress: "lola-zc",
    ingress_authority: "authority-c",
    egress: "iceoryx2-zc",
    egress_authority: "authority-b",
    wire_format: "protobuf",
};

const ZC_ROUTES_ZENOH_ICEORYX2: &[ZeroCopyRouteTemplate] =
    &[ROUTE_ZENOH_TO_ICEORYX2, ROUTE_ICEORYX2_TO_ZENOH];
const ZC_ROUTES_ZENOH_LOLA: &[ZeroCopyRouteTemplate] = &[ROUTE_ZENOH_TO_LOLA, ROUTE_LOLA_TO_ZENOH];
const ZC_ROUTES_ALL_TRANSPORTS: &[ZeroCopyRouteTemplate] = &[
    ROUTE_ZENOH_TO_ICEORYX2,
    ROUTE_ALL_ZENOH_TO_LOLA,
    ROUTE_ICEORYX2_TO_ZENOH,
    ROUTE_ICEORYX2_TO_LOLA,
    ROUTE_ALL_LOLA_TO_ZENOH,
    ROUTE_LOLA_TO_ICEORYX2,
];

const ZC_SCENARIO_ZENOH_TO_ICEORYX2: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-zenoh-shm-to-iceoryx2",
    row_description: "Zenoh SHM to iceoryx2 copy-minimized startup row",
    config_file: "CONFIG_ZENOH_ICEORYX2_ZEROCOPY_EXAMPLE.json5",
    cargo_features: "experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy",
    build_command: BUILD_ZC_ZENOH_ICEORYX2,
    required_paths: REQUIRED_ZC_ZENOH_ICEORYX2_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: Some(ROUTE_ZENOH_TO_ICEORYX2),
    configured_routes: ZC_ROUTES_ZENOH_ICEORYX2,
    requires_lola_bundled: false,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};
const ZC_SCENARIO_ICEORYX2_TO_ZENOH: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-iceoryx2-to-zenoh-shm",
    row_description: "iceoryx2 to Zenoh SHM copy-minimized startup row",
    config_file: "CONFIG_ZENOH_ICEORYX2_ZEROCOPY_EXAMPLE.json5",
    cargo_features: "experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy",
    build_command: BUILD_ZC_ZENOH_ICEORYX2,
    required_paths: REQUIRED_ZC_ZENOH_ICEORYX2_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: Some(ROUTE_ICEORYX2_TO_ZENOH),
    configured_routes: ZC_ROUTES_ZENOH_ICEORYX2,
    requires_lola_bundled: false,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};
const ZC_SCENARIO_ZENOH_TO_LOLA: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-zenoh-shm-to-lola-bundled",
    row_description: "Zenoh SHM to LoLa bundled copy-minimized startup row",
    config_file: "CONFIG_LOLA_ZEROCOPY_EXAMPLE.json5",
    cargo_features: "experimental-copy-minimized-routing,zenoh-zero-copy,lola-transport",
    build_command: BUILD_ZC_ZENOH_LOLA,
    required_paths: REQUIRED_ZC_ZENOH_LOLA_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: Some(ROUTE_ZENOH_TO_LOLA),
    configured_routes: ZC_ROUTES_ZENOH_LOLA,
    requires_lola_bundled: true,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};
const ZC_SCENARIO_LOLA_TO_ZENOH: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-lola-bundled-to-zenoh-shm",
    row_description: "LoLa bundled to Zenoh SHM copy-minimized startup row",
    config_file: "CONFIG_LOLA_ZEROCOPY_EXAMPLE.json5",
    cargo_features: "experimental-copy-minimized-routing,zenoh-zero-copy,lola-transport",
    build_command: BUILD_ZC_ZENOH_LOLA,
    required_paths: REQUIRED_ZC_ZENOH_LOLA_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: Some(ROUTE_LOLA_TO_ZENOH),
    configured_routes: ZC_ROUTES_ZENOH_LOLA,
    requires_lola_bundled: true,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};
const ZC_SCENARIO_ICEORYX2_TO_LOLA: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-iceoryx2-to-lola-bundled",
    row_description: "iceoryx2 to LoLa bundled copy-minimized startup row",
    config_file: "CONFIG_ZEROCOPY_EXAMPLE.json5",
    cargo_features:
        "experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy,lola-transport",
    build_command: BUILD_ZC_ALL_TRANSPORTS,
    required_paths: REQUIRED_ZC_ALL_TRANSPORTS_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: Some(ROUTE_ICEORYX2_TO_LOLA),
    configured_routes: ZC_ROUTES_ALL_TRANSPORTS,
    requires_lola_bundled: true,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};
const ZC_SCENARIO_LOLA_TO_ICEORYX2: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-lola-bundled-to-iceoryx2",
    row_description: "LoLa bundled to iceoryx2 copy-minimized startup row",
    config_file: "CONFIG_ZEROCOPY_EXAMPLE.json5",
    cargo_features:
        "experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy,lola-transport",
    build_command: BUILD_ZC_ALL_TRANSPORTS,
    required_paths: REQUIRED_ZC_ALL_TRANSPORTS_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: Some(ROUTE_LOLA_TO_ICEORYX2),
    configured_routes: ZC_ROUTES_ALL_TRANSPORTS,
    requires_lola_bundled: true,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};
const ZC_SCENARIO_ALL_TRANSPORTS: ZeroCopyScenarioTemplate = ZeroCopyScenarioTemplate {
    id: "smoke-zc-all-transports-bundled",
    row_description: "Zenoh SHM, iceoryx2, and LoLa bundled aggregate copy-minimized startup row",
    config_file: "CONFIG_ZEROCOPY_EXAMPLE.json5",
    cargo_features:
        "experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy,lola-transport",
    build_command: BUILD_ZC_ALL_TRANSPORTS,
    required_paths: REQUIRED_ZC_ALL_TRANSPORTS_PATHS,
    stale_process_signatures: &["configurable-streamer"],
    selected_route: None,
    configured_routes: ZC_ROUTES_ALL_TRANSPORTS,
    requires_lola_bundled: true,
    hard_timeout_secs_default: ZERO_COPY_HARD_TIMEOUT_SECS,
};

const SCENARIO_MQTT_RR_ZENOH_CLIENT_MQTT_SERVICE: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-mqtt-rr-zenoh-client-mqtt-service",
    transport_family: TransportFamily::Mqtt,
    build_commands: BUILD_MQTT_RR_ZENOH_CLIENT,
    required_paths: REQUIRED_MQTT_PATHS,
    stale_process_signatures: &["configurable-streamer", "mqtt_server", "zenoh_client"],
    requires_mqtt_broker: true,
    requires_vsomeip_runtime: false,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "configurable-streamer",
        binary: "configurable-streamer",
        args: &["--config", "CONFIG.json5"],
        env: MQTT_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "service",
        workdir: ".",
        binary: "mqtt_server",
        args: &["--broker-uri", "localhost:1883"],
        env: PASSIVE_INFO_ENV,
        log_file: "service.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "client",
        workdir: ".",
        binary: "zenoh_client",
        args: NO_ARGS,
        env: ACTIVE_DEBUG_ENV,
        log_file: "client.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::MQTT_HARD_TIMEOUT_SECS,
};

const SCENARIO_MQTT_RR_MQTT_CLIENT_ZENOH_SERVICE: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service",
    transport_family: TransportFamily::Mqtt,
    build_commands: BUILD_MQTT_RR_MQTT_CLIENT,
    required_paths: REQUIRED_MQTT_PATHS,
    stale_process_signatures: &["configurable-streamer", "zenoh_server", "mqtt_client"],
    requires_mqtt_broker: true,
    requires_vsomeip_runtime: false,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "configurable-streamer",
        binary: "configurable-streamer",
        args: &["--config", "CONFIG.json5"],
        env: MQTT_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "service",
        workdir: ".",
        binary: "zenoh_server",
        args: NO_ARGS,
        env: PASSIVE_INFO_ENV,
        log_file: "service.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "client",
        workdir: ".",
        binary: "mqtt_client",
        args: &["--broker-uri", "localhost:1883"],
        env: ACTIVE_DEBUG_ENV,
        log_file: "client.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::MQTT_HARD_TIMEOUT_SECS,
};

const SCENARIO_MQTT_PS_ZENOH_PUBLISHER_MQTT_SUBSCRIBER: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-mqtt-ps-zenoh-publisher-mqtt-subscriber",
    transport_family: TransportFamily::Mqtt,
    build_commands: BUILD_MQTT_PS_ZENOH_PUBLISHER,
    required_paths: REQUIRED_MQTT_PATHS,
    stale_process_signatures: &[
        "configurable-streamer",
        "mqtt_subscriber",
        "zenoh_publisher",
    ],
    requires_mqtt_broker: true,
    requires_vsomeip_runtime: false,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "configurable-streamer",
        binary: "configurable-streamer",
        args: &["--config", "CONFIG.json5"],
        env: MQTT_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "subscriber",
        workdir: ".",
        binary: "mqtt_subscriber",
        args: PASSIVE_MQTT_SUBSCRIBER_ARGS_A,
        env: PASSIVE_INFO_ENV,
        log_file: "subscriber.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "publisher",
        workdir: ".",
        binary: "zenoh_publisher",
        args: NO_ARGS,
        env: ACTIVE_DEBUG_ENV,
        log_file: "publisher.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::MQTT_HARD_TIMEOUT_SECS,
};

const SCENARIO_MQTT_PS_MQTT_PUBLISHER_ZENOH_SUBSCRIBER: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-mqtt-ps-mqtt-publisher-zenoh-subscriber",
    transport_family: TransportFamily::Mqtt,
    build_commands: BUILD_MQTT_PS_MQTT_PUBLISHER,
    required_paths: REQUIRED_MQTT_PATHS,
    stale_process_signatures: &[
        "configurable-streamer",
        "zenoh_subscriber",
        "mqtt_publisher",
    ],
    requires_mqtt_broker: true,
    requires_vsomeip_runtime: false,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "configurable-streamer",
        binary: "configurable-streamer",
        args: &["--config", "CONFIG.json5"],
        env: MQTT_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "subscriber",
        workdir: ".",
        binary: "zenoh_subscriber",
        args: PASSIVE_ZENOH_SUBSCRIBER_ARGS_B,
        env: PASSIVE_INFO_ENV,
        log_file: "subscriber.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "publisher",
        workdir: ".",
        binary: "mqtt_publisher",
        args: &["--broker-uri", "localhost:1883"],
        env: ACTIVE_DEBUG_ENV,
        log_file: "publisher.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::MQTT_HARD_TIMEOUT_SECS,
};

const SCENARIO_SOMEIP_RR_ZENOH_CLIENT_SOMEIP_SERVICE: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-someip-rr-zenoh-client-someip-service",
    transport_family: TransportFamily::Someip,
    build_commands: BUILD_SOMEIP_RR_ZENOH_CLIENT,
    required_paths: REQUIRED_SOMEIP_PATHS,
    stale_process_signatures: &["zenoh_someip", "someip_server", "zenoh_client"],
    requires_mqtt_broker: false,
    requires_vsomeip_runtime: true,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "example-streamer-implementations",
        binary: "zenoh_someip",
        args: &["--config", "DEFAULT_CONFIG.json5"],
        env: SOMEIP_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "service",
        workdir: ".",
        binary: "someip_server",
        args: NO_ARGS,
        env: PASSIVE_INFO_ENV,
        log_file: "service.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "client",
        workdir: ".",
        binary: "zenoh_client",
        args: NO_ARGS,
        env: ACTIVE_DEBUG_ENV,
        log_file: "client.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::SOMEIP_HARD_TIMEOUT_SECS,
};

const SCENARIO_SOMEIP_RR_SOMEIP_CLIENT_ZENOH_SERVICE: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-someip-rr-someip-client-zenoh-service",
    transport_family: TransportFamily::Someip,
    build_commands: BUILD_SOMEIP_RR_SOMEIP_CLIENT,
    required_paths: REQUIRED_SOMEIP_PATHS,
    stale_process_signatures: &["zenoh_someip", "zenoh_server", "someip_client"],
    requires_mqtt_broker: false,
    requires_vsomeip_runtime: true,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "example-streamer-implementations",
        binary: "zenoh_someip",
        args: &["--config", "DEFAULT_CONFIG.json5"],
        env: SOMEIP_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "service",
        workdir: ".",
        binary: "zenoh_server",
        args: PASSIVE_ZENOH_SERVICE_ARGS_B_FROM_SOMEIP,
        env: PASSIVE_INFO_ENV,
        log_file: "service.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "client",
        workdir: ".",
        binary: "someip_client",
        args: NO_ARGS,
        env: ACTIVE_DEBUG_ENV,
        log_file: "client.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::SOMEIP_HARD_TIMEOUT_SECS,
};

const SCENARIO_SOMEIP_PS_ZENOH_PUBLISHER_SOMEIP_SUBSCRIBER: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-someip-ps-zenoh-publisher-someip-subscriber",
    transport_family: TransportFamily::Someip,
    build_commands: BUILD_SOMEIP_PS_ZENOH_PUBLISHER,
    required_paths: REQUIRED_SOMEIP_PATHS,
    stale_process_signatures: &["zenoh_someip", "someip_subscriber", "zenoh_publisher"],
    requires_mqtt_broker: false,
    requires_vsomeip_runtime: true,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "example-streamer-implementations",
        binary: "zenoh_someip",
        args: &["--config", "DEFAULT_CONFIG.json5"],
        env: SOMEIP_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "subscriber",
        workdir: ".",
        binary: "someip_subscriber",
        args: PASSIVE_SOMEIP_SUBSCRIBER_ARGS_A,
        env: PASSIVE_INFO_ENV,
        log_file: "subscriber.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "publisher",
        workdir: ".",
        binary: "zenoh_publisher",
        args: NO_ARGS,
        env: ACTIVE_DEBUG_ENV,
        log_file: "publisher.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::SOMEIP_HARD_TIMEOUT_SECS,
};

const SCENARIO_SOMEIP_PS_SOMEIP_PUBLISHER_ZENOH_SUBSCRIBER: ScenarioTemplate = ScenarioTemplate {
    id: "smoke-zenoh-someip-ps-someip-publisher-zenoh-subscriber",
    transport_family: TransportFamily::Someip,
    build_commands: BUILD_SOMEIP_PS_SOMEIP_PUBLISHER,
    required_paths: REQUIRED_SOMEIP_PATHS,
    stale_process_signatures: &["zenoh_someip", "zenoh_subscriber", "someip_publisher"],
    requires_mqtt_broker: false,
    requires_vsomeip_runtime: true,
    streamer: ProcessTemplate {
        name: "streamer",
        workdir: "example-streamer-implementations",
        binary: "zenoh_someip",
        args: &["--config", "DEFAULT_CONFIG.json5"],
        env: SOMEIP_STREAMER_ENV,
        log_file: "streamer.log",
        readiness_marker: Some(env::READY_STREAMER_INITIALIZED),
        readiness_timeout_secs: Some(env::STREAMER_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    passive: ProcessTemplate {
        name: "subscriber",
        workdir: ".",
        binary: "zenoh_subscriber",
        args: PASSIVE_ZENOH_SUBSCRIBER_ARGS_B_FROM_SOMEIP,
        env: PASSIVE_INFO_ENV,
        log_file: "subscriber.log",
        readiness_marker: Some(env::READY_LISTENER_REGISTERED),
        readiness_timeout_secs: Some(env::PASSIVE_READY_TIMEOUT_SECS),
        bounded_sender: false,
    },
    active: ProcessTemplate {
        name: "publisher",
        workdir: ".",
        binary: "someip_publisher",
        args: NO_ARGS,
        env: ACTIVE_DEBUG_ENV,
        log_file: "publisher.log",
        readiness_marker: None,
        readiness_timeout_secs: None,
        bounded_sender: true,
    },
    hard_timeout_secs_default: env::SOMEIP_HARD_TIMEOUT_SECS,
};

#[derive(Debug, Clone, Args)]
pub struct ScenarioCliArgs {
    #[arg(long)]
    pub skip_build: bool,

    #[arg(long)]
    pub artifacts_root: Option<PathBuf>,

    #[arg(long)]
    pub claims_path: Option<PathBuf>,

    #[arg(long, default_value_t = env::DEFAULT_SEND_COUNT)]
    pub send_count: u64,

    #[arg(long, default_value_t = env::DEFAULT_SEND_INTERVAL_MS)]
    pub send_interval_ms: u64,

    #[arg(long)]
    pub scenario_timeout_secs: Option<u64>,

    #[arg(long)]
    pub expected_branch: Option<String>,

    #[arg(long)]
    pub no_bootstrap: bool,

    #[arg(long, value_enum, default_value = "docker-compose")]
    pub mqtt_broker_mode: MqttBrokerMode,

    #[arg(long, default_value = "localhost:1883")]
    pub mqtt_broker_uri: String,

    #[arg(long, default_value_t = env::DEFAULT_ENDPOINT_CLAIM_MIN_COUNT)]
    pub endpoint_claim_min_count: usize,

    #[arg(long, default_value_t = env::DEFAULT_EGRESS_SEND_ATTEMPT_MIN_COUNT)]
    pub egress_send_attempt_min_count: usize,

    #[arg(long, default_value_t = env::DEFAULT_EGRESS_SEND_OK_MIN_COUNT)]
    pub egress_send_ok_min_count: usize,

    #[arg(long, default_value_t = env::DEFAULT_EGRESS_WORKER_MIN_COUNT)]
    pub egress_worker_min_count: usize,
}

#[derive(Debug, Clone)]
pub struct ScenarioRunResult {
    pub pass: bool,
    pub exit_code: i32,
    pub artifact_dir: PathBuf,
    pub scenario_report_json: PathBuf,
    pub scenario_report_txt: PathBuf,
    pub failure_reason: Option<String>,
}

pub fn scenario_ids() -> &'static [&'static str] {
    &SCENARIO_IDS
}

pub fn matrix_scenario_ids() -> &'static [&'static str] {
    &MATRIX_SCENARIO_IDS
}

pub fn scenario_template(scenario_id: &str) -> Option<&'static ScenarioTemplate> {
    match scenario_id {
        "smoke-zenoh-mqtt-rr-zenoh-client-mqtt-service" => {
            Some(&SCENARIO_MQTT_RR_ZENOH_CLIENT_MQTT_SERVICE)
        }
        "smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service" => {
            Some(&SCENARIO_MQTT_RR_MQTT_CLIENT_ZENOH_SERVICE)
        }
        "smoke-zenoh-mqtt-ps-zenoh-publisher-mqtt-subscriber" => {
            Some(&SCENARIO_MQTT_PS_ZENOH_PUBLISHER_MQTT_SUBSCRIBER)
        }
        "smoke-zenoh-mqtt-ps-mqtt-publisher-zenoh-subscriber" => {
            Some(&SCENARIO_MQTT_PS_MQTT_PUBLISHER_ZENOH_SUBSCRIBER)
        }
        "smoke-zenoh-someip-rr-zenoh-client-someip-service" => {
            Some(&SCENARIO_SOMEIP_RR_ZENOH_CLIENT_SOMEIP_SERVICE)
        }
        "smoke-zenoh-someip-rr-someip-client-zenoh-service" => {
            Some(&SCENARIO_SOMEIP_RR_SOMEIP_CLIENT_ZENOH_SERVICE)
        }
        "smoke-zenoh-someip-ps-zenoh-publisher-someip-subscriber" => {
            Some(&SCENARIO_SOMEIP_PS_ZENOH_PUBLISHER_SOMEIP_SUBSCRIBER)
        }
        "smoke-zenoh-someip-ps-someip-publisher-zenoh-subscriber" => {
            Some(&SCENARIO_SOMEIP_PS_SOMEIP_PUBLISHER_ZENOH_SUBSCRIBER)
        }
        _ => None,
    }
}

fn zero_copy_scenario_template(scenario_id: &str) -> Option<&'static ZeroCopyScenarioTemplate> {
    match scenario_id {
        "smoke-zc-zenoh-shm-to-iceoryx2" => Some(&ZC_SCENARIO_ZENOH_TO_ICEORYX2),
        "smoke-zc-iceoryx2-to-zenoh-shm" => Some(&ZC_SCENARIO_ICEORYX2_TO_ZENOH),
        "smoke-zc-zenoh-shm-to-lola-bundled" => Some(&ZC_SCENARIO_ZENOH_TO_LOLA),
        "smoke-zc-lola-bundled-to-zenoh-shm" => Some(&ZC_SCENARIO_LOLA_TO_ZENOH),
        "smoke-zc-iceoryx2-to-lola-bundled" => Some(&ZC_SCENARIO_ICEORYX2_TO_LOLA),
        "smoke-zc-lola-bundled-to-iceoryx2" => Some(&ZC_SCENARIO_LOLA_TO_ICEORYX2),
        "smoke-zc-all-transports-bundled" => Some(&ZC_SCENARIO_ALL_TRANSPORTS),
        _ => None,
    }
}

pub fn is_known_scenario(scenario_id: &str) -> bool {
    scenario_template(scenario_id).is_some() || zero_copy_scenario_template(scenario_id).is_some()
}

pub async fn run_scenario(
    scenario_id: &str,
    cli_args: ScenarioCliArgs,
) -> Result<ScenarioRunResult> {
    if let Some(template) = zero_copy_scenario_template(scenario_id) {
        return run_zero_copy_scenario(template, cli_args).await;
    }

    let template = scenario_template(scenario_id)
        .ok_or_else(|| anyhow!("unknown scenario id '{}'", scenario_id))?;

    let repo_root = env::repo_root()?;
    let expected_branch = env::resolve_expected_branch(cli_args.expected_branch.clone());
    let artifacts_root = env::resolve_artifacts_root(&repo_root, cli_args.artifacts_root.clone());
    let artifact_dir = artifacts_root
        .join(template.id)
        .join(env::scenario_timestamp());
    std::fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("unable to create artifact dir {}", artifact_dir.display()))?;

    let scenario_start_wall = Utc::now();
    let scenario_start_instant = Instant::now();
    let hard_timeout_secs = cli_args
        .scenario_timeout_secs
        .unwrap_or(template.hard_timeout_secs_default);
    let scenario_deadline = scenario_start_instant + Duration::from_secs(hard_timeout_secs);

    let thresholds = Thresholds {
        endpoint_communication_min_count: cli_args.endpoint_claim_min_count,
        egress_send_attempt_min_count: cli_args.egress_send_attempt_min_count,
        egress_send_ok_min_count: cli_args.egress_send_ok_min_count,
        egress_worker_create_or_reuse_min_count: cli_args.egress_worker_min_count,
    };

    let mut phase_timings = Vec::new();
    let mut failure_reason = None;
    let mut claim_outcomes = Vec::new();
    let mut forbidden_claim_outcomes = Vec::new();
    let mut loaded_claims = Vec::new();
    let mut claims_source_path: Option<PathBuf> = None;

    let mut streamer_process: Option<ManagedProcess> = None;
    let mut passive_process: Option<ManagedProcess> = None;
    let mut active_process: Option<ManagedProcess> = None;
    let mut mqtt_broker_handle: Option<MqttBrokerHandle> = None;
    let mut vsomeip_runtime_lib: Option<PathBuf> = None;

    let preflight_result = execute_phase("Preflight", &mut phase_timings, || async {
        ensure_remaining_timeout(scenario_deadline, "Preflight")?;

        env::enforce_expected_branch(&repo_root, expected_branch.as_deref()).await?;
        env::ensure_paths_exist(&repo_root, template.required_paths)?;

        let loaded = load_claims_for_scenario(
            &repo_root,
            template.id,
            cli_args.claims_path.as_deref(),
            thresholds,
        )?;
        claims_source_path = Some(loaded.source_path.clone());
        loaded_claims = loaded.claims;

        ensure_no_stale_processes(template.stale_process_signatures).await?;

        if template.requires_mqtt_broker {
            preflight_mqtt_broker(&repo_root, &cli_args).await?;
        }

        if template.requires_vsomeip_runtime {
            vsomeip_runtime_lib = Some(env::detect_vsomeip_runtime_lib(&repo_root)?);
        }

        if !cli_args.skip_build {
            for build_command in template.build_commands {
                let outcome =
                    run_shell_command(&repo_root, &repo_root, build_command, cli_args.no_bootstrap)
                        .await?;
                assert_command_success(outcome, build_command)?;
            }
        }

        Ok(())
    })
    .await;

    if let Err(error) = preflight_result {
        failure_reason = Some(error.to_string());
    }

    if failure_reason.is_none() {
        let start_infra_result = execute_phase("StartInfra", &mut phase_timings, || async {
            ensure_remaining_timeout(scenario_deadline, "StartInfra")?;

            if template.requires_mqtt_broker {
                mqtt_broker_handle =
                    start_mqtt_broker(&repo_root, &artifact_dir, &cli_args, scenario_deadline)
                        .await?;
            }

            streamer_process = Some(
                spawn_template_process(
                    &repo_root,
                    &artifact_dir,
                    template,
                    &template.streamer,
                    &cli_args,
                    vsomeip_runtime_lib.as_ref(),
                )
                .await?,
            );

            Ok(())
        })
        .await;

        if let Err(error) = start_infra_result {
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let wait_streamer_ready_result =
            execute_phase("WaitStreamerReady", &mut phase_timings, || async {
                ensure_remaining_timeout(scenario_deadline, "WaitStreamerReady")?;

                let streamer = streamer_process
                    .as_ref()
                    .ok_or_else(|| anyhow!("streamer process missing before readiness wait"))?;
                let marker = template
                    .streamer
                    .readiness_marker
                    .ok_or_else(|| anyhow!("streamer readiness marker not configured"))?;

                let marker_timeout = Duration::from_secs(
                    template
                        .streamer
                        .readiness_timeout_secs
                        .unwrap_or(env::STREAMER_READY_TIMEOUT_SECS),
                );
                let timeout = min(
                    marker_timeout,
                    ensure_remaining_timeout(scenario_deadline, "WaitStreamerReady")?,
                );

                logs::wait_for_exact_marker(
                    &streamer.log_path,
                    marker,
                    timeout,
                    Duration::from_millis(env::LOG_POLL_INTERVAL_MS),
                )
                .await?;

                Ok(())
            })
            .await;

        if let Err(error) = wait_streamer_ready_result {
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let start_passive_result = execute_phase("StartPassive", &mut phase_timings, || async {
            ensure_remaining_timeout(scenario_deadline, "StartPassive")?;

            passive_process = Some(
                spawn_template_process(
                    &repo_root,
                    &artifact_dir,
                    template,
                    &template.passive,
                    &cli_args,
                    vsomeip_runtime_lib.as_ref(),
                )
                .await?,
            );

            Ok(())
        })
        .await;

        if let Err(error) = start_passive_result {
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let wait_passive_ready_result =
            execute_phase("WaitPassiveReady", &mut phase_timings, || async {
                ensure_remaining_timeout(scenario_deadline, "WaitPassiveReady")?;

                let passive = passive_process
                    .as_ref()
                    .ok_or_else(|| anyhow!("passive process missing before readiness wait"))?;
                let marker = template
                    .passive
                    .readiness_marker
                    .ok_or_else(|| anyhow!("passive readiness marker not configured"))?;

                let marker_timeout = Duration::from_secs(
                    template
                        .passive
                        .readiness_timeout_secs
                        .unwrap_or(env::PASSIVE_READY_TIMEOUT_SECS),
                );
                let timeout = min(
                    marker_timeout,
                    ensure_remaining_timeout(scenario_deadline, "WaitPassiveReady")?,
                );

                logs::wait_for_exact_marker(
                    &passive.log_path,
                    marker,
                    timeout,
                    Duration::from_millis(env::LOG_POLL_INTERVAL_MS),
                )
                .await?;

                Ok(())
            })
            .await;

        if let Err(error) = wait_passive_ready_result {
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let start_active_result = execute_phase("StartActive", &mut phase_timings, || async {
            ensure_remaining_timeout(scenario_deadline, "StartActive")?;

            active_process = Some(
                spawn_template_process(
                    &repo_root,
                    &artifact_dir,
                    template,
                    &template.active,
                    &cli_args,
                    vsomeip_runtime_lib.as_ref(),
                )
                .await?,
            );

            let active = active_process
                .as_mut()
                .ok_or_else(|| anyhow!("active process missing after spawn"))?;
            let remaining = ensure_remaining_timeout(scenario_deadline, "StartActive")?;
            let exited = active.wait_with_timeout(remaining).await?;
            if !exited {
                return Err(anyhow!(
                    "scenario hard timeout reached while waiting for bounded active sender completion"
                ));
            }

            if active.exit_status_code != Some(0) {
                return Err(anyhow!(
                    "active process '{}' exited with status {:?}",
                    active.name,
                    active.exit_status_code
                ));
            }

            Ok(())
        })
        .await;

        if let Err(error) = start_active_result {
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let validate_claims_result =
            execute_phase("ValidateClaims", &mut phase_timings, || async {
                ensure_remaining_timeout(scenario_deadline, "ValidateClaims")?;

                if loaded_claims.is_empty() {
                    return Err(anyhow!(
                        "no claims were loaded before validation for scenario '{}'",
                        template.id
                    ));
                }

                let outcomes = evaluate_claims(&artifact_dir, &loaded_claims);
                let (must_outcomes, forbidden_outcomes, first_failed_claim_reason) =
                    split_claim_outcomes(outcomes);

                claim_outcomes = must_outcomes;
                forbidden_claim_outcomes = forbidden_outcomes;

                let streamer = streamer_process
                    .as_ref()
                    .ok_or_else(|| anyhow!("streamer process missing during claim validation"))?;
                let passive = passive_process
                    .as_ref()
                    .ok_or_else(|| anyhow!("passive process missing during claim validation"))?;

                let streamer_marker_count =
                    logs::count_exact_marker(&streamer.log_path, env::READY_STREAMER_INITIALIZED)?;
                if streamer_marker_count != 1 {
                    return Err(anyhow!(
                        "streamer readiness marker '{}' observed {} times (expected exactly 1)",
                        env::READY_STREAMER_INITIALIZED,
                        streamer_marker_count
                    ));
                }

                let passive_marker_count =
                    logs::count_exact_marker(&passive.log_path, env::READY_LISTENER_REGISTERED)?;
                if passive_marker_count != 1 {
                    return Err(anyhow!(
                        "passive readiness marker '{}' observed {} times (expected exactly 1)",
                        env::READY_LISTENER_REGISTERED,
                        passive_marker_count
                    ));
                }

                if let Some(first_failed_claim_reason) = first_failed_claim_reason {
                    return Err(anyhow!(first_failed_claim_reason));
                }

                Ok(())
            })
            .await;

        if let Err(error) = validate_claims_result {
            failure_reason = Some(error.to_string());
        }
    }

    let teardown_result = execute_phase("Teardown", &mut phase_timings, || async {
        if let Some(active) = active_process.as_mut() {
            active
                .terminate_gracefully(
                    Duration::from_secs(env::SIGINT_GRACE_SECS),
                    Duration::from_secs(env::SIGTERM_GRACE_SECS),
                )
                .await?;
        }

        if let Some(passive) = passive_process.as_mut() {
            passive
                .terminate_gracefully(
                    Duration::from_secs(env::SIGINT_GRACE_SECS),
                    Duration::from_secs(env::SIGTERM_GRACE_SECS),
                )
                .await?;
        }

        if let Some(streamer) = streamer_process.as_mut() {
            streamer
                .terminate_gracefully(
                    Duration::from_secs(env::SIGINT_GRACE_SECS),
                    Duration::from_secs(env::SIGTERM_GRACE_SECS),
                )
                .await?;
        }

        if let Some(handle) = mqtt_broker_handle.as_mut() {
            stop_mqtt_broker(&repo_root, cli_args.no_bootstrap, handle).await?;
        }

        ensure_process_exited(active_process.as_mut(), "active")?;
        ensure_process_exited(passive_process.as_mut(), "passive")?;
        ensure_process_exited(streamer_process.as_mut(), "streamer")?;

        Ok(())
    })
    .await;

    if failure_reason.is_none() {
        if let Err(error) = teardown_result {
            failure_reason = Some(error.to_string());
        }
    }

    let finalize_report_result =
        execute_phase("FinalizeReport", &mut phase_timings, || async { Ok(()) }).await;
    if failure_reason.is_none() {
        if let Err(error) = finalize_report_result {
            failure_reason = Some(error.to_string());
        }
    }

    let scenario_end_wall = Utc::now();
    let scenario_duration_ms = scenario_start_instant.elapsed().as_millis();

    let process_metadata =
        gather_process_metadata(&streamer_process, &passive_process, &active_process);

    let pass = failure_reason.is_none()
        && claim_outcomes.iter().all(|outcome| outcome.pass)
        && forbidden_claim_outcomes.iter().all(|outcome| outcome.pass);
    let exit_code = if pass { 0 } else { 1 };

    let repro_command = render_repro_command(template.id, &cli_args, &artifact_dir);

    let scenario_report = ScenarioReport {
        schema_version: report::SCENARIO_REPORT_SCHEMA_VERSION.to_string(),
        scenario_id: template.id.to_string(),
        transport_family: template.transport_family.as_str().to_string(),
        pass,
        classification: ScenarioClassification::from_pass(pass),
        exit_code,
        phase_timings,
        processes: process_metadata,
        claim_outcomes,
        forbidden_claim_outcomes,
        failure_reason: failure_reason.clone(),
        repro_command,
        artifact_dir: artifact_dir.display().to_string(),
        claims_source_path: claims_source_path
            .as_ref()
            .map(|path| path.display().to_string()),
        supporting_artifacts: Vec::new(),
        start_ts: report::timestamp_to_string(scenario_start_wall),
        end_ts: report::timestamp_to_string(scenario_end_wall),
        duration_ms: scenario_duration_ms,
    };

    let (scenario_report_json, scenario_report_txt) =
        report::write_scenario_report(&scenario_report, &artifact_dir)?;

    println!("SCENARIO_ID={}", template.id);
    println!(
        "SCENARIO_STATUS={}",
        if scenario_report.pass { "PASS" } else { "FAIL" }
    );
    println!("SCENARIO_ARTIFACT_DIR={}", artifact_dir.display());
    println!("SCENARIO_REPORT_JSON={}", scenario_report_json.display());
    println!(
        "SCENARIO_CLAIMS_SOURCE_PATH={}",
        scenario_report
            .claims_source_path
            .as_deref()
            .unwrap_or("<not-resolved>")
    );

    Ok(ScenarioRunResult {
        pass: scenario_report.pass,
        exit_code,
        artifact_dir,
        scenario_report_json,
        scenario_report_txt,
        failure_reason,
    })
}

async fn run_zero_copy_scenario(
    template: &'static ZeroCopyScenarioTemplate,
    cli_args: ScenarioCliArgs,
) -> Result<ScenarioRunResult> {
    let repo_root = env::repo_root()?;
    let expected_branch = env::resolve_expected_branch(cli_args.expected_branch.clone());
    let artifacts_root = env::resolve_artifacts_root(&repo_root, cli_args.artifacts_root.clone());
    let artifact_dir = artifacts_root
        .join(template.id)
        .join(env::scenario_timestamp());
    std::fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("unable to create artifact dir {}", artifact_dir.display()))?;

    let scenario_start_wall = Utc::now();
    let scenario_start_instant = Instant::now();
    let hard_timeout_secs = cli_args
        .scenario_timeout_secs
        .unwrap_or(template.hard_timeout_secs_default);
    let scenario_deadline = scenario_start_instant + Duration::from_secs(hard_timeout_secs);

    let mut phase_timings = Vec::new();
    let mut failure_reason = None;
    let mut classification = ScenarioClassification::Pass;
    let mut blocked_reason = None;
    let mut claim_outcomes = Vec::new();
    let mut forbidden_claim_outcomes = Vec::new();
    let mut loaded_claims = Vec::new();
    let claims_source_path: Option<PathBuf> = None;
    let mut streamer_process: Option<ManagedProcess> = None;
    let mut lola_bazel: Option<String> = None;
    let mut lola_bridge_lib_dir: Option<PathBuf> = None;

    let preflight_result = execute_phase("Preflight", &mut phase_timings, || async {
        ensure_remaining_timeout(scenario_deadline, "Preflight")?;

        env::enforce_expected_branch(&repo_root, expected_branch.as_deref()).await?;
        env::ensure_paths_exist(&repo_root, template.required_paths)?;

        loaded_claims = zero_copy_startup_claims();

        if let Err(error) = ensure_no_stale_processes(template.stale_process_signatures).await {
            blocked_reason = Some(error.to_string());
            return Err(error);
        }

        if template.requires_lola_bundled {
            lola_bazel = Some(
                ensure_lola_bazel_available(&repo_root, cli_args.no_bootstrap, &mut blocked_reason)
                    .await?,
            );
        }

        if !cli_args.skip_build {
            let build_command = zero_copy_build_command(template, lola_bazel.as_deref())?;
            let outcome = run_shell_command(
                &repo_root,
                &repo_root,
                &build_command,
                cli_args.no_bootstrap,
            )
            .await?;
            assert_command_success(outcome, &build_command)?;
        }

        if template.requires_lola_bundled {
            match env::detect_lola_bridge_lib_dir(&repo_root) {
                Ok(lib_dir) => lola_bridge_lib_dir = Some(lib_dir),
                Err(error) => {
                    blocked_reason = Some(error.to_string());
                    return Err(error);
                }
            }
        }

        Ok(())
    })
    .await;

    if let Err(error) = preflight_result {
        classification = classify_zero_copy_failure(blocked_reason.as_ref());
        failure_reason = Some(error.to_string());
    }

    if failure_reason.is_none() {
        let start_infra_result = execute_phase("StartInfra", &mut phase_timings, || async {
            ensure_remaining_timeout(scenario_deadline, "StartInfra")?;

            streamer_process = Some(
                spawn_zero_copy_streamer(
                    &repo_root,
                    &artifact_dir,
                    template,
                    &cli_args,
                    lola_bridge_lib_dir.as_ref(),
                )
                .await?,
            );

            Ok(())
        })
        .await;

        if let Err(error) = start_infra_result {
            classification = classify_zero_copy_failure(blocked_reason.as_ref());
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let wait_streamer_ready_result =
            execute_phase("WaitStreamerReady", &mut phase_timings, || async {
                ensure_remaining_timeout(scenario_deadline, "WaitStreamerReady")?;

                let streamer = streamer_process
                    .as_ref()
                    .ok_or_else(|| anyhow!("streamer process missing before readiness wait"))?;
                let timeout = min(
                    Duration::from_secs(env::STREAMER_READY_TIMEOUT_SECS),
                    ensure_remaining_timeout(scenario_deadline, "WaitStreamerReady")?,
                );

                logs::wait_for_exact_marker(
                    &streamer.log_path,
                    env::READY_STREAMER_INITIALIZED,
                    timeout,
                    Duration::from_millis(env::LOG_POLL_INTERVAL_MS),
                )
                .await?;

                Ok(())
            })
            .await;

        if let Err(error) = wait_streamer_ready_result {
            if is_zero_copy_native_blocker(template, &error) {
                blocked_reason = Some(error.to_string());
                classification = ScenarioClassification::Blocked;
            } else {
                classification = ScenarioClassification::ValidatedFail;
            }
            failure_reason = Some(error.to_string());
        }
    }

    if failure_reason.is_none() {
        let validate_claims_result =
            execute_phase("ValidateClaims", &mut phase_timings, || async {
                ensure_remaining_timeout(scenario_deadline, "ValidateClaims")?;

                if loaded_claims.is_empty() {
                    return Err(anyhow!(
                        "no claims were loaded before validation for scenario '{}'",
                        template.id
                    ));
                }

                let outcomes = evaluate_claims(&artifact_dir, &loaded_claims);
                let (must_outcomes, forbidden_outcomes, first_failed_claim_reason) =
                    split_claim_outcomes(outcomes);

                claim_outcomes = must_outcomes;
                forbidden_claim_outcomes = forbidden_outcomes;

                let streamer = streamer_process
                    .as_ref()
                    .ok_or_else(|| anyhow!("streamer process missing during claim validation"))?;
                let streamer_marker_count =
                    logs::count_exact_marker(&streamer.log_path, env::READY_STREAMER_INITIALIZED)?;
                if streamer_marker_count != 1 {
                    return Err(anyhow!(
                        "streamer readiness marker '{}' observed {} times (expected exactly 1)",
                        env::READY_STREAMER_INITIALIZED,
                        streamer_marker_count
                    ));
                }

                if let Some(first_failed_claim_reason) = first_failed_claim_reason {
                    return Err(anyhow!(first_failed_claim_reason));
                }

                Ok(())
            })
            .await;

        if let Err(error) = validate_claims_result {
            classification = ScenarioClassification::ValidatedFail;
            failure_reason = Some(error.to_string());
        }
    }

    let teardown_result = execute_phase("Teardown", &mut phase_timings, || async {
        if let Some(streamer) = streamer_process.as_mut() {
            streamer
                .terminate_gracefully(
                    Duration::from_secs(env::SIGINT_GRACE_SECS),
                    Duration::from_secs(env::SIGTERM_GRACE_SECS),
                )
                .await?;
        }

        ensure_process_exited(streamer_process.as_mut(), "streamer")?;

        Ok(())
    })
    .await;

    if failure_reason.is_none() {
        if let Err(error) = teardown_result {
            classification = classify_zero_copy_failure(blocked_reason.as_ref());
            failure_reason = Some(error.to_string());
        }
    }

    let finalize_report_result =
        execute_phase("FinalizeReport", &mut phase_timings, || async { Ok(()) }).await;
    if failure_reason.is_none() {
        if let Err(error) = finalize_report_result {
            classification = ScenarioClassification::ValidatedFail;
            failure_reason = Some(error.to_string());
        }
    }

    let scenario_end_wall = Utc::now();
    let scenario_duration_ms = scenario_start_instant.elapsed().as_millis();
    let pass = failure_reason.is_none()
        && claim_outcomes.iter().all(|outcome| outcome.pass)
        && forbidden_claim_outcomes.iter().all(|outcome| outcome.pass);
    if pass {
        classification = ScenarioClassification::Pass;
    } else if classification == ScenarioClassification::Pass {
        classification = ScenarioClassification::ValidatedFail;
    }
    let exit_code = match classification {
        ScenarioClassification::Pass => 0,
        ScenarioClassification::ValidatedFail => 1,
        ScenarioClassification::Blocked => 2,
    };

    let zero_copy_artifact = write_zero_copy_matrix_row_artifact(ZeroCopyMatrixRowArtifactInput {
        repo_root: &repo_root,
        artifact_dir: &artifact_dir,
        template,
        classification,
        failure_reason: failure_reason.clone(),
        streamer_process: &streamer_process,
        lola_bridge_lib_dir: lola_bridge_lib_dir.as_ref(),
        lola_bazel: lola_bazel.as_deref(),
    })?;

    let no_process: Option<ManagedProcess> = None;
    let process_metadata = gather_process_metadata(&streamer_process, &no_process, &no_process);
    let repro_command = render_zero_copy_repro_command(template.id, &cli_args, &artifact_dir);

    let scenario_report = ScenarioReport {
        schema_version: report::SCENARIO_REPORT_SCHEMA_VERSION.to_string(),
        scenario_id: template.id.to_string(),
        transport_family: TransportFamily::ZeroCopy.as_str().to_string(),
        pass,
        classification,
        exit_code,
        phase_timings,
        processes: process_metadata,
        claim_outcomes,
        forbidden_claim_outcomes,
        failure_reason: failure_reason.clone(),
        repro_command,
        artifact_dir: artifact_dir.display().to_string(),
        claims_source_path: claims_source_path
            .as_ref()
            .map(|path| path.display().to_string()),
        supporting_artifacts: vec![zero_copy_artifact.display().to_string()],
        start_ts: report::timestamp_to_string(scenario_start_wall),
        end_ts: report::timestamp_to_string(scenario_end_wall),
        duration_ms: scenario_duration_ms,
    };

    let (scenario_report_json, scenario_report_txt) =
        report::write_scenario_report(&scenario_report, &artifact_dir)?;

    println!("SCENARIO_ID={}", template.id);
    println!(
        "SCENARIO_STATUS={}",
        if scenario_report.pass { "PASS" } else { "FAIL" }
    );
    println!("SCENARIO_CLASSIFICATION={}", classification.as_str());
    println!("SCENARIO_ARTIFACT_DIR={}", artifact_dir.display());
    println!("SCENARIO_REPORT_JSON={}", scenario_report_json.display());
    println!("ZERO_COPY_MATRIX_ROW_JSON={}", zero_copy_artifact.display());
    println!(
        "SCENARIO_CLAIMS_SOURCE_PATH={}",
        scenario_report
            .claims_source_path
            .as_deref()
            .unwrap_or("<not-resolved>")
    );

    Ok(ScenarioRunResult {
        pass: scenario_report.pass,
        exit_code,
        artifact_dir,
        scenario_report_json,
        scenario_report_txt,
        failure_reason,
    })
}

fn classify_zero_copy_failure(blocked_reason: Option<&String>) -> ScenarioClassification {
    if blocked_reason.is_some() {
        ScenarioClassification::Blocked
    } else {
        ScenarioClassification::ValidatedFail
    }
}

fn is_zero_copy_native_blocker(template: &ZeroCopyScenarioTemplate, error: &anyhow::Error) -> bool {
    if !template.requires_lola_bundled {
        return false;
    }

    let message = error.to_string();
    message.contains("libup_lola_bridge.so")
        || message.contains("error while loading shared libraries")
        || message.contains("cannot open shared object file")
}

fn zero_copy_startup_claims() -> Vec<ClaimSpec> {
    vec![
        ClaimSpec {
            claim_id: "streamer_ready".to_string(),
            category: ClaimCategory::Readiness,
            kind: ClaimKind::MustMatch,
            file: "streamer.log".to_string(),
            pattern: "READY streamer_initialized".to_string(),
            min_count: 1,
        },
        ClaimSpec {
            claim_id: "streamer_initialized_log".to_string(),
            category: ClaimCategory::Readiness,
            kind: ClaimKind::MustMatch,
            file: "streamer.log".to_string(),
            pattern: "Streamer initialized; waiting for shutdown signal".to_string(),
            min_count: 1,
        },
        ClaimSpec {
            claim_id: "streamer_no_panic".to_string(),
            category: ClaimCategory::ForbiddenSignature,
            kind: ClaimKind::MustNotMatch,
            file: "streamer.log".to_string(),
            pattern: "panicked at".to_string(),
            min_count: 0,
        },
        ClaimSpec {
            claim_id: "streamer_no_copy_minimized_failure".to_string(),
            category: ClaimCategory::ForbiddenSignature,
            kind: ClaimKind::MustNotMatch,
            file: "streamer.log".to_string(),
            pattern: "copy-minimized route .* failed|copy_minimized_.*_failed".to_string(),
            min_count: 0,
        },
    ]
}

async fn ensure_lola_bazel_available(
    repo_root: &Path,
    no_bootstrap: bool,
    blocked_reason: &mut Option<String>,
) -> Result<String> {
    if let Ok(bazel) = std::env::var("BAZEL") {
        let bazel = bazel.trim();
        if !bazel.is_empty() {
            return validate_lola_bazel_command(repo_root, bazel, "BAZEL", blocked_reason).await;
        }
    }

    let cached_bazelisk = repo_root.join(STREAMER_LOLA_BAZELISK_CACHE_PATH);
    if is_executable_file(&cached_bazelisk) {
        return Ok(cached_bazelisk.display().to_string());
    }

    let helper = repo_root.join(STREAMER_LOLA_BAZELISK_HELPER);
    if no_bootstrap {
        let error = anyhow!(
            "LoLa bundled zero-copy row is blocked because BAZEL is unset, repo-local Bazelisk is missing at {}, and --no-bootstrap prevents running {}",
            cached_bazelisk.display(),
            STREAMER_LOLA_BAZELISK_HELPER
        );
        *blocked_reason = Some(error.to_string());
        return Err(error);
    }

    if !helper.is_file() {
        let error = anyhow!(
            "LoLa bundled zero-copy row is blocked because the Streamer Bazelisk helper is missing: {}",
            helper.display()
        );
        *blocked_reason = Some(error.to_string());
        return Err(error);
    }

    let command = format!(
        "{} --print",
        shell_escape(helper.to_string_lossy().as_ref())
    );
    let outcome = run_shell_command(repo_root, repo_root, &command, true).await?;
    if outcome.status_code == Some(0) {
        let resolved = outcome.stdout.trim();
        if !resolved.is_empty() && is_executable_file(Path::new(resolved)) {
            return Ok(resolved.to_string());
        }
    }

    let error = anyhow!(
        "LoLa bundled zero-copy row is blocked because {} failed to provide an executable Bazelisk path (status={:?})\nstdout:\n{}\nstderr:\n{}",
        STREAMER_LOLA_BAZELISK_HELPER,
        outcome.status_code,
        outcome.stdout,
        outcome.stderr
    );
    *blocked_reason = Some(error.to_string());
    Err(error)
}

async fn validate_lola_bazel_command(
    repo_root: &Path,
    bazel: &str,
    source: &str,
    blocked_reason: &mut Option<String>,
) -> Result<String> {
    let bazel_path = Path::new(bazel);
    if bazel_path.is_absolute() || bazel.contains('/') {
        if is_executable_file(bazel_path) {
            return Ok(bazel.to_string());
        }
        let error = anyhow!(
            "LoLa bundled zero-copy row is blocked because {source} points to a missing or non-executable Bazel command: {bazel}"
        );
        *blocked_reason = Some(error.to_string());
        return Err(error);
    }

    let command = format!("command -v {}", shell_escape(bazel));
    let outcome = run_shell_command(repo_root, repo_root, &command, true).await?;
    if outcome.status_code == Some(0) {
        let resolved = outcome.stdout.lines().next().unwrap_or(bazel).trim();
        if !resolved.is_empty() {
            return Ok(resolved.to_string());
        }
    }

    let error = anyhow!(
        "LoLa bundled zero-copy row is blocked because {source} command '{}' is not on PATH",
        bazel
    );
    *blocked_reason = Some(error.to_string());
    Err(error)
}

fn zero_copy_build_command(
    template: &ZeroCopyScenarioTemplate,
    lola_bazel: Option<&str>,
) -> Result<String> {
    if template.requires_lola_bundled {
        let bazel = lola_bazel.ok_or_else(|| {
            anyhow!("LoLa bundled zero-copy build requires a resolved Bazel command before build")
        })?;
        Ok(format!(
            "BAZEL={} {}",
            shell_escape(bazel),
            template.build_command
        ))
    } else {
        Ok(template.build_command.to_string())
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

async fn spawn_zero_copy_streamer(
    repo_root: &Path,
    artifact_dir: &Path,
    template: &ZeroCopyScenarioTemplate,
    cli_args: &ScenarioCliArgs,
    lola_bridge_lib_dir: Option<&PathBuf>,
) -> Result<ManagedProcess> {
    let mut env_pairs = ZERO_COPY_STREAMER_ENV
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    if template.requires_lola_bundled {
        let lib_dir = lola_bridge_lib_dir.ok_or_else(|| {
            anyhow!("LoLa bundled row requires libup_lola_bridge.so but no library directory was resolved")
        })?;
        let existing_ld_library_path = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let merged_ld_library_path = if existing_ld_library_path.is_empty() {
            lib_dir.display().to_string()
        } else {
            format!("{}:{existing_ld_library_path}", lib_dir.display())
        };
        env_pairs.push(("LD_LIBRARY_PATH".to_string(), merged_ld_library_path));
    }

    let process_spec = ProcessSpec {
        name: "streamer".to_string(),
        workdir: repo_root.join("configurable-streamer"),
        executable: target_debug_binary(repo_root, "configurable-streamer"),
        args: vec!["--config".to_string(), template.config_file.to_string()],
        env: env_pairs,
        log_file_name: "streamer.log".to_string(),
    };

    ManagedProcess::spawn(process_spec, repo_root, artifact_dir, cli_args.no_bootstrap).await
}

fn render_zero_copy_repro_command(
    scenario_id: &str,
    cli_args: &ScenarioCliArgs,
    artifact_dir: &Path,
) -> String {
    let mut args = vec![
        "cargo run -p transport-smoke-suite --bin".to_string(),
        scenario_id.to_string(),
        "--".to_string(),
        format!(
            "--artifacts-root {}",
            shell_escape(
                artifact_dir
                    .parent()
                    .unwrap_or(artifact_dir)
                    .display()
                    .to_string()
                    .as_str()
            )
        ),
        format!("--send-count {}", cli_args.send_count),
        format!("--send-interval-ms {}", cli_args.send_interval_ms),
    ];

    if cli_args.skip_build {
        args.push("--skip-build".to_string());
    }
    if cli_args.no_bootstrap {
        args.push("--no-bootstrap".to_string());
    }
    if cli_args.mqtt_broker_mode != MqttBrokerMode::DockerCompose {
        args.push(format!(
            "--mqtt-broker-mode {}",
            cli_args.mqtt_broker_mode.as_str()
        ));
    }
    if cli_args.mqtt_broker_uri != DEFAULT_MQTT_BROKER_URI {
        args.push(format!(
            "--mqtt-broker-uri {}",
            shell_escape(&cli_args.mqtt_broker_uri)
        ));
    }
    if let Some(claims_path) = &cli_args.claims_path {
        args.push(format!(
            "--claims-path {}",
            shell_escape(claims_path.display().to_string().as_str())
        ));
    }
    if let Some(expected_branch) = &cli_args.expected_branch {
        args.push(format!(
            "--expected-branch {}",
            shell_escape(expected_branch)
        ));
    }
    if let Some(scenario_timeout_secs) = cli_args.scenario_timeout_secs {
        args.push(format!("--scenario-timeout-secs {}", scenario_timeout_secs));
    }

    args.join(" ")
}

struct ZeroCopyMatrixRowArtifactInput<'a> {
    repo_root: &'a Path,
    artifact_dir: &'a Path,
    template: &'a ZeroCopyScenarioTemplate,
    classification: ScenarioClassification,
    failure_reason: Option<String>,
    streamer_process: &'a Option<ManagedProcess>,
    lola_bridge_lib_dir: Option<&'a PathBuf>,
    lola_bazel: Option<&'a str>,
}

fn write_zero_copy_matrix_row_artifact(
    input: ZeroCopyMatrixRowArtifactInput<'_>,
) -> Result<PathBuf> {
    let artifact_path = input.artifact_dir.join("zero-copy-matrix-row.json");
    let row = ZeroCopyMatrixRowArtifact {
        schema_version: "1.0",
        scenario_id: input.template.id.to_string(),
        row_description: input.template.row_description.to_string(),
        classification: input.classification,
        failure_reason: input.failure_reason,
        config_file: format!("configurable-streamer/{}", input.template.config_file),
        cargo_features: split_features(input.template.cargo_features),
        selected_route: input.template.selected_route.map(route_to_artifact),
        configured_routes: input
            .template
            .configured_routes
            .iter()
            .copied()
            .map(route_to_artifact)
            .collect(),
        payload_bytes: PayloadProbeArtifact {
            observed_payload_bytes: 0,
            probe: "startup_only",
            note: "09D3 validates native zero-copy example startup and route wiring; no payload sender is introduced in this scoped harness branch",
        },
        metadata: MetadataProbeArtifact {
            observed_frame_metadata: false,
            probe: "startup_only",
            note: "frame metadata is not observed because the row does not inject payload traffic",
        },
        route_diagnostics: input
            .template
            .configured_routes
            .iter()
            .copied()
            .map(route_to_diagnostic_artifact)
            .collect(),
        listener_cleanup: ListenerCleanupArtifact {
            teardown_phase_recorded: true,
            process_exit_status: input
                .streamer_process
                .as_ref()
                .and_then(|process| process.exit_status_code),
            cleanup_signal: "SIGINT_then_SIGTERM_if_needed",
            note: "scenario teardown terminates the configurable-streamer process group and verifies process exit",
        },
        raw_logs: input
            .streamer_process
            .as_ref()
            .map(|process| {
                vec![RawLogArtifact {
                    name: process.name.clone(),
                    path: process.log_path.display().to_string(),
                }]
            })
            .unwrap_or_default(),
        environment: ZeroCopyEnvironmentArtifact {
            rust_log: ZERO_COPY_STREAMER_ENV
                .iter()
                .find_map(|(key, value)| (*key == "RUST_LOG").then_some((*value).to_string()))
                .unwrap_or_default(),
            mqtt_broker_required: false,
            docker_compose_required: false,
            lola_bundled_required: input.template.requires_lola_bundled,
            lola_bridge_lib_dir: input
                .lola_bridge_lib_dir
                .map(|path| path.display().to_string()),
            bazel: input.lola_bazel.map(ToString::to_string),
        },
        dependency_sources: dependency_sources(input.repo_root)?,
    };

    let payload = serde_json::to_string_pretty(&row).context("serialize zero-copy row artifact")?;
    fs::write(&artifact_path, payload).with_context(|| {
        format!(
            "unable to write zero-copy matrix row artifact {}",
            artifact_path.display()
        )
    })?;
    Ok(artifact_path)
}

fn split_features(features: &str) -> Vec<String> {
    features
        .split(',')
        .map(str::trim)
        .filter(|feature| !feature.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn route_to_artifact(route: ZeroCopyRouteTemplate) -> ZeroCopyRouteArtifact {
    ZeroCopyRouteArtifact {
        ingress: route.ingress.to_string(),
        ingress_authority: route.ingress_authority.to_string(),
        egress: route.egress.to_string(),
        egress_authority: route.egress_authority.to_string(),
        wire_format: route.wire_format.to_string(),
    }
}

fn route_to_diagnostic_artifact(route: ZeroCopyRouteTemplate) -> RouteDiagnosticArtifact {
    RouteDiagnosticArtifact {
        route_kind: "copy_minimized",
        ingress: route.ingress.to_string(),
        ingress_authority: route.ingress_authority.to_string(),
        egress: route.egress.to_string(),
        egress_authority: route.egress_authority.to_string(),
        diagnostic_source: "09D2 config forwarding plus successful configurable-streamer readiness",
    }
}

fn dependency_sources(repo_root: &Path) -> Result<Vec<DependencySourceArtifact>> {
    let lock_path = repo_root.join("Cargo.lock");
    let lock_contents = fs::read_to_string(&lock_path)
        .with_context(|| format!("unable to read {}", lock_path.display()))?;
    let packages = [
        "up-rust",
        "up-transport-zenoh",
        "up-transport-iceoryx2-rust",
        "up-transport-lola-rust",
        "up-transport-mqtt5",
        "up-streamer",
        "configurable-streamer",
    ];
    Ok(packages
        .iter()
        .map(|package| dependency_source_from_lock(&lock_contents, package))
        .collect())
}

fn dependency_source_from_lock(lock_contents: &str, package: &str) -> DependencySourceArtifact {
    let package_marker = format!("name = \"{package}\"");
    let mut lines = lock_contents.lines();
    while let Some(line) = lines.next() {
        if line.trim() != package_marker {
            continue;
        }

        let mut version = None;
        let mut source = None;
        for line in lines.by_ref() {
            let line = line.trim();
            if line == "[[package]]" {
                break;
            }
            if let Some(value) = line.strip_prefix("version = ") {
                version = Some(unquote_lock_value(value));
            }
            if let Some(value) = line.strip_prefix("source = ") {
                source = Some(unquote_lock_value(value));
            }
        }

        return DependencySourceArtifact {
            package: package.to_string(),
            source,
            version,
        };
    }

    DependencySourceArtifact {
        package: package.to_string(),
        source: None,
        version: None,
    }
}

fn unquote_lock_value(value: &str) -> String {
    value.trim_matches('"').to_string()
}

fn render_repro_command(
    scenario_id: &str,
    cli_args: &ScenarioCliArgs,
    artifact_dir: &Path,
) -> String {
    let mut args = vec![
        "cargo run -p transport-smoke-suite --bin".to_string(),
        scenario_id.to_string(),
        "--".to_string(),
        format!(
            "--artifacts-root {}",
            shell_escape(
                artifact_dir
                    .parent()
                    .unwrap_or(artifact_dir)
                    .display()
                    .to_string()
                    .as_str()
            )
        ),
        format!("--send-count {}", cli_args.send_count),
        format!("--send-interval-ms {}", cli_args.send_interval_ms),
    ];

    if cli_args.skip_build {
        args.push("--skip-build".to_string());
    }
    if cli_args.no_bootstrap {
        args.push("--no-bootstrap".to_string());
    }
    if cli_args.mqtt_broker_mode != MqttBrokerMode::DockerCompose {
        args.push(format!(
            "--mqtt-broker-mode {}",
            cli_args.mqtt_broker_mode.as_str()
        ));
    }
    if cli_args.mqtt_broker_uri != DEFAULT_MQTT_BROKER_URI {
        args.push(format!(
            "--mqtt-broker-uri {}",
            shell_escape(&cli_args.mqtt_broker_uri)
        ));
    }
    if let Some(claims_path) = &cli_args.claims_path {
        args.push(format!(
            "--claims-path {}",
            shell_escape(claims_path.display().to_string().as_str())
        ));
    }
    if let Some(expected_branch) = &cli_args.expected_branch {
        args.push(format!(
            "--expected-branch {}",
            shell_escape(expected_branch)
        ));
    }
    if let Some(scenario_timeout_secs) = cli_args.scenario_timeout_secs {
        args.push(format!("--scenario-timeout-secs {}", scenario_timeout_secs));
    }

    args.join(" ")
}

fn gather_process_metadata(
    streamer_process: &Option<ManagedProcess>,
    passive_process: &Option<ManagedProcess>,
    active_process: &Option<ManagedProcess>,
) -> Vec<ProcessMetadata> {
    [streamer_process, passive_process, active_process]
        .into_iter()
        .flatten()
        .map(|process| ProcessMetadata {
            name: process.name.clone(),
            pid: process.pid,
            process_group_id: process.process_group_id,
            command: process.command_line.clone(),
            workdir: process.workdir.display().to_string(),
            log_file: process.log_path.display().to_string(),
            exit_status: process.exit_status_code,
        })
        .collect()
}

fn ensure_process_exited(process: Option<&mut ManagedProcess>, role: &str) -> Result<()> {
    let Some(process) = process else {
        return Ok(());
    };

    if !process.has_exited() {
        return Err(anyhow!(
            "{} process '{}' remained alive after teardown",
            role,
            process.name
        ));
    }
    Ok(())
}

enum MqttBrokerHandle {
    DockerCompose,
    Native(Box<ManagedProcess>),
}

async fn preflight_mqtt_broker(repo_root: &Path, cli_args: &ScenarioCliArgs) -> Result<()> {
    match cli_args.mqtt_broker_mode {
        MqttBrokerMode::DockerCompose => {
            assert_command_success(
                run_shell_command(repo_root, repo_root, "docker --version", true).await?,
                "docker --version",
            )?;
            assert_command_success(
                run_shell_command(repo_root, repo_root, "docker compose version", true).await?,
                "docker compose version",
            )
        }
        MqttBrokerMode::External => probe_mqtt_broker_uri(&cli_args.mqtt_broker_uri).await,
        MqttBrokerMode::Native => {
            if probe_mqtt_broker_uri(&cli_args.mqtt_broker_uri)
                .await
                .is_ok()
            {
                return Ok(());
            }
            let _ = resolve_mosquitto_command(repo_root).await?;
            Ok(())
        }
    }
}

async fn start_mqtt_broker(
    repo_root: &Path,
    artifact_dir: &Path,
    cli_args: &ScenarioCliArgs,
    deadline: Instant,
) -> Result<Option<MqttBrokerHandle>> {
    match cli_args.mqtt_broker_mode {
        MqttBrokerMode::DockerCompose => {
            start_docker_compose_mqtt_broker(repo_root, cli_args.no_bootstrap, deadline).await?;
            Ok(Some(MqttBrokerHandle::DockerCompose))
        }
        MqttBrokerMode::External => {
            probe_mqtt_broker_uri(&cli_args.mqtt_broker_uri).await?;
            Ok(None)
        }
        MqttBrokerMode::Native => {
            if probe_mqtt_broker_uri(&cli_args.mqtt_broker_uri)
                .await
                .is_ok()
            {
                return Ok(None);
            }
            start_native_mqtt_broker(
                repo_root,
                artifact_dir,
                &cli_args.mqtt_broker_uri,
                cli_args.no_bootstrap,
                deadline,
            )
            .await
            .map(Box::new)
            .map(MqttBrokerHandle::Native)
            .map(Some)
        }
    }
}

async fn stop_mqtt_broker(
    repo_root: &Path,
    no_bootstrap: bool,
    handle: &mut MqttBrokerHandle,
) -> Result<()> {
    match handle {
        MqttBrokerHandle::DockerCompose => {
            stop_docker_compose_mqtt_broker(repo_root, no_bootstrap).await
        }
        MqttBrokerHandle::Native(process) => {
            process
                .terminate_gracefully(
                    Duration::from_secs(env::SIGINT_GRACE_SECS),
                    Duration::from_secs(env::SIGTERM_GRACE_SECS),
                )
                .await?;
            ensure_process_exited(Some(process.as_mut()), "mqtt broker")
        }
    }
}

async fn start_docker_compose_mqtt_broker(
    repo_root: &Path,
    no_bootstrap: bool,
    deadline: Instant,
) -> Result<()> {
    let compose_path = repo_root
        .join("utils")
        .join("mosquitto")
        .join("docker-compose.yaml");
    let compose_path_quoted = shell_escape(compose_path.display().to_string().as_str());

    let down_command = format!("docker compose -f {compose_path_quoted} down --remove-orphans");
    let _ = run_shell_command(repo_root, repo_root, &down_command, no_bootstrap).await;

    let up_command = format!("docker compose -f {compose_path_quoted} up -d");
    let up_outcome = run_shell_command(repo_root, repo_root, &up_command, no_bootstrap).await?;
    assert_command_success(up_outcome, "docker compose up -d")?;

    let broker_deadline = Instant::now() + Duration::from_secs(env::BROKER_READY_TIMEOUT_SECS);
    loop {
        let poll_command =
            format!("docker compose -f {compose_path_quoted} ps --status running --services");
        let poll_outcome = run_shell_command(repo_root, repo_root, &poll_command, true).await?;
        if poll_outcome.status_code == Some(0)
            && poll_outcome
                .stdout
                .lines()
                .any(|line| line.trim() == "mosquitto")
        {
            return Ok(());
        }

        if Instant::now() >= broker_deadline {
            return Err(anyhow!(
                "timed out waiting for MQTT broker readiness (stdout='{}', stderr='{}')",
                poll_outcome.stdout.trim(),
                poll_outcome.stderr.trim()
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "scenario hard timeout reached while waiting for MQTT broker readiness"
            ));
        }

        tokio::time::sleep(Duration::from_millis(env::LOG_POLL_INTERVAL_MS)).await;
    }
}

async fn stop_docker_compose_mqtt_broker(repo_root: &Path, no_bootstrap: bool) -> Result<()> {
    let compose_path = repo_root
        .join("utils")
        .join("mosquitto")
        .join("docker-compose.yaml");
    let compose_path_quoted = shell_escape(compose_path.display().to_string().as_str());
    let down_command = format!("docker compose -f {compose_path_quoted} down --remove-orphans");

    let outcome = run_shell_command(repo_root, repo_root, &down_command, no_bootstrap).await?;
    assert_command_success(outcome, "docker compose down")
}

async fn start_native_mqtt_broker(
    repo_root: &Path,
    artifact_dir: &Path,
    broker_uri: &str,
    no_bootstrap: bool,
    deadline: Instant,
) -> Result<ManagedProcess> {
    let (host, port) = parse_mqtt_broker_uri(broker_uri)?;
    if !is_local_mqtt_host(&host) {
        return Err(anyhow!(
            "native MQTT broker mode can only start local mosquitto instances, got host '{}' from '{}'",
            host,
            broker_uri
        ));
    }

    let mosquitto = resolve_mosquitto_command(repo_root).await?;
    let process_spec = ProcessSpec {
        name: "mosquitto".to_string(),
        workdir: repo_root.to_path_buf(),
        executable: mosquitto,
        args: vec!["-p".to_string(), port.to_string(), "-v".to_string()],
        env: Vec::new(),
        log_file_name: "mosquitto.log".to_string(),
    };
    let mut process =
        ManagedProcess::spawn(process_spec, repo_root, artifact_dir, no_bootstrap).await?;
    let broker_deadline = Instant::now() + Duration::from_secs(env::BROKER_READY_TIMEOUT_SECS);

    loop {
        if probe_mqtt_broker_uri(broker_uri).await.is_ok() {
            return Ok(process);
        }
        process.refresh_exit_status().await?;
        if process.has_exited() {
            return Err(anyhow!(
                "native mosquitto process exited before broker readiness; see {}",
                process.log_path.display()
            ));
        }
        if Instant::now() >= broker_deadline {
            return Err(anyhow!(
                "timed out waiting for native MQTT broker readiness at {}",
                broker_uri
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "scenario hard timeout reached while waiting for native MQTT broker readiness"
            ));
        }

        tokio::time::sleep(Duration::from_millis(env::LOG_POLL_INTERVAL_MS)).await;
    }
}

async fn resolve_mosquitto_command(repo_root: &Path) -> Result<PathBuf> {
    let outcome = run_shell_command(repo_root, repo_root, "command -v mosquitto", true).await?;
    if outcome.status_code == Some(0) {
        if let Some(path) = outcome
            .stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
        {
            return Ok(PathBuf::from(path));
        }
    }

    Err(anyhow!(
        "native MQTT broker mode requires mosquitto on PATH (status={:?})\nstdout:\n{}\nstderr:\n{}",
        outcome.status_code,
        outcome.stdout,
        outcome.stderr
    ))
}

async fn probe_mqtt_broker_uri(broker_uri: &str) -> Result<()> {
    let (host, port) = parse_mqtt_broker_uri(broker_uri)?;
    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .with_context(|| format!("unable to resolve MQTT broker at {broker_uri}"))?;
    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    Err(anyhow!(
        "unable to connect to MQTT broker at {}: {}",
        broker_uri,
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "no resolved socket addresses".to_string())
    ))
}

fn parse_mqtt_broker_uri(broker_uri: &str) -> Result<(String, u16)> {
    let trimmed = broker_uri.trim();
    let without_scheme = trimmed
        .strip_prefix("mqtt://")
        .or_else(|| trimmed.strip_prefix("tcp://"))
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    let (host, port) = without_scheme
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("MQTT broker URI must be host:port, got '{}'", broker_uri))?;
    if host.is_empty() {
        return Err(anyhow!("MQTT broker URI host is empty: '{}'", broker_uri));
    }
    let port = port
        .parse::<u16>()
        .with_context(|| format!("MQTT broker URI port is invalid: '{broker_uri}'"))?;
    Ok((host.to_string(), port))
}

fn is_local_mqtt_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn target_debug_binary(repo_root: &Path, binary: &str) -> PathBuf {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                repo_root.join(path)
            }
        })
        .unwrap_or_else(|| repo_root.join("target"));

    target_dir.join("debug").join(binary)
}

async fn spawn_template_process(
    repo_root: &Path,
    artifact_dir: &Path,
    scenario_template: &ScenarioTemplate,
    template: &ProcessTemplate,
    cli_args: &ScenarioCliArgs,
    vsomeip_runtime_lib: Option<&PathBuf>,
) -> Result<ManagedProcess> {
    let mut args = template
        .args
        .iter()
        .map(|arg| resolve_process_arg(arg, cli_args))
        .collect::<Vec<_>>();
    if scenario_template.requires_vsomeip_runtime && template.name == "streamer" {
        args = resolve_vsomeip_streamer_args(repo_root, artifact_dir, &args)?;
    }
    if template.bounded_sender {
        args.push("--send-count".to_string());
        args.push(cli_args.send_count.to_string());
        args.push("--send-interval-ms".to_string());
        args.push(cli_args.send_interval_ms.to_string());
    }

    let mut env_pairs = template
        .env
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();

    if scenario_template.requires_vsomeip_runtime {
        let runtime_lib = vsomeip_runtime_lib.ok_or_else(|| {
            anyhow!("vsomeip runtime library was required but no path was resolved")
        })?;
        let existing_ld_library_path = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let merged_ld_library_path = if existing_ld_library_path.is_empty() {
            runtime_lib.display().to_string()
        } else {
            format!("{}:{existing_ld_library_path}", runtime_lib.display())
        };
        env_pairs.push(("LD_LIBRARY_PATH".to_string(), merged_ld_library_path));
    }

    let process_spec = ProcessSpec {
        name: template.name.to_string(),
        workdir: repo_root.join(template.workdir),
        executable: target_debug_binary(repo_root, template.binary),
        args,
        env: env_pairs,
        log_file_name: template.log_file.to_string(),
    };

    ManagedProcess::spawn(process_spec, repo_root, artifact_dir, cli_args.no_bootstrap).await
}

fn resolve_process_arg(arg: &str, cli_args: &ScenarioCliArgs) -> String {
    if arg == DEFAULT_MQTT_BROKER_URI {
        cli_args.mqtt_broker_uri.clone()
    } else {
        arg.to_string()
    }
}

fn resolve_vsomeip_streamer_args(
    repo_root: &Path,
    artifact_dir: &Path,
    args: &[String],
) -> Result<Vec<String>> {
    let mut resolved = args.to_vec();
    let Some(config_index) = resolved.iter().position(|arg| arg == "--config") else {
        return Ok(resolved);
    };
    let Some(config_file) = resolved.get(config_index + 1) else {
        return Ok(resolved);
    };
    if config_file != "DEFAULT_CONFIG.json5" {
        return Ok(resolved);
    }

    let source_config = repo_root
        .join("example-streamer-implementations")
        .join(config_file);
    let source_contents = fs::read_to_string(&source_config)
        .with_context(|| format!("failed to read {}", source_config.display()))?;
    let someip_config = repo_root
        .join("example-streamer-implementations")
        .join("vsomeip-configs")
        .join("point_to_point.json");
    let generated_contents = source_contents.replace(
        "config_file: \"../../example-streamer-implementations/vsomeip-configs/point_to_point.json\"",
        &format!("config_file: \"{}\"", someip_config.display()),
    );
    let generated_config = artifact_dir.join("DEFAULT_CONFIG.vsomeip-smoke.json5");
    fs::write(&generated_config, generated_contents)
        .with_context(|| format!("failed to write {}", generated_config.display()))?;
    resolved[config_index + 1] = generated_config.display().to_string();
    Ok(resolved)
}

async fn ensure_no_stale_processes(signatures: &[&str]) -> Result<()> {
    if signatures.is_empty() {
        return Ok(());
    }

    let pattern = signatures.join("|");
    let output = tokio::process::Command::new("pgrep")
        .arg("-fa")
        .arg(&pattern)
        .output()
        .await
        .context("failed to run stale-process preflight check")?;

    if output.status.code() == Some(1) {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return Err(anyhow!(
            "stale process check failed; matching processes still running: {}",
            stdout
        ));
    }

    Err(anyhow!(
        "stale process check failed with status {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn assert_command_success(outcome: crate::process::CommandOutcome, label: &str) -> Result<()> {
    if outcome.status_code == Some(0) {
        return Ok(());
    }

    Err(anyhow!(
        "{} failed (status={:?})\ncommand: {}\nstdout:\n{}\nstderr:\n{}",
        label,
        outcome.status_code,
        outcome.command,
        outcome.stdout,
        outcome.stderr
    ))
}

fn ensure_remaining_timeout(deadline: Instant, phase: &str) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| anyhow!("scenario hard timeout reached in phase '{phase}'"))
}

async fn execute_phase<F, Fut>(
    phase_name: &str,
    phase_timings: &mut Vec<PhaseTiming>,
    f: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let phase_start_wall = Utc::now();
    let phase_start_instant = Instant::now();
    let result = f().await;
    let phase_end_wall = Utc::now();

    phase_timings.push(PhaseTiming {
        phase: phase_name.to_string(),
        start_ts: report::timestamp_to_string(phase_start_wall),
        end_ts: report::timestamp_to_string(phase_end_wall),
        duration_ms: phase_start_instant.elapsed().as_millis(),
    });

    result
}

pub fn scenario_ids_for_transport(transport_family: TransportFamily) -> Vec<&'static str> {
    MATRIX_SCENARIO_IDS
        .iter()
        .copied()
        .filter(|scenario_id| {
            if transport_family == TransportFamily::ZeroCopy {
                return zero_copy_scenario_template(scenario_id).is_some();
            }
            scenario_template(scenario_id)
                .map(|template| template.transport_family == transport_family)
                .unwrap_or(false)
        })
        .collect()
}
