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

impl Config {
    /// Rejects reflective physical topologies before creating native resources.
    pub(crate) fn validate_physical_domains(&self) -> Result<(), up_rust::UStatus> {
        let Some(transport) = &self.transports.iceoryx2 else {
            return Ok(());
        };
        for endpoint in &transport.endpoints {
            if let Some(namespace) = &endpoint.iceoryx2_namespace {
                namespace.validate()?;
            }
        }
        if transport.endpoints.len() > 1 {
            for (index, endpoint) in transport.endpoints.iter().enumerate() {
                let namespace = endpoint.iceoryx2_namespace.as_ref().ok_or_else(|| {
                    up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument,
                        format!("iceoryx2 endpoint {} requires an explicit physical namespace in a multi-endpoint bridge", endpoint.endpoint))
                })?;
                for other in transport.endpoints.iter().skip(index + 1) {
                    let peer = other.iceoryx2_namespace.as_ref().ok_or_else(|| {
                        up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument,
                            format!("iceoryx2 endpoint {} requires an explicit physical namespace in a multi-endpoint bridge", other.endpoint))
                    })?;
                    if namespace.prefix.starts_with(&peer.prefix)
                        || peer.prefix.starts_with(&namespace.prefix)
                    {
                        return Err(up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument,
                            format!("iceoryx2 endpoints {} and {} have overlapping physical namespaces; logical authorities do not isolate a native bus", endpoint.endpoint, other.endpoint)));
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolve immutable peer agreements before opening endpoints or routes.
    pub(crate) fn load_native_profiles(
        &mut self,
        base: &std::path::Path,
    ) -> Result<(), up_rust::UStatus> {
        use configurable_streamer_wire_support::native_profile::{
            LOCAL_PROFILE_ENV, PEER_PROFILE_ENV,
        };
        if std::env::var_os(LOCAL_PROFILE_ENV).is_some()
            || std::env::var_os(PEER_PROFILE_ENV).is_some()
        {
            return Err(up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument,
                "legacy native profile paths are unsupported; configure native_deployment for each endpoint"));
        }
        let mut groups = vec![
            (false, &mut self.transports.zenoh.endpoints),
            (false, &mut self.transports.mqtt.endpoints),
        ];
        if let Some(transport) = &mut self.transports.iceoryx2 {
            groups.push((false, &mut transport.endpoints));
        }
        if let Some(transport) = &mut self.transports.lola {
            groups.push((false, &mut transport.endpoints));
        }
        if let Some(transport) = &mut self.transports.vsomeip {
            groups.push((false, &mut transport.endpoints));
        }
        if let Some(transport) = &mut self.transports.dds {
            groups.push((true, &mut transport.endpoints));
        }
        for (dds, endpoints) in groups {
            for endpoint in endpoints {
                if (!dds && endpoint.routing_mode == RoutingMode::EncodedOwned)
                    || (dds && endpoint.routing_mode == RoutingMode::CopyMinimized)
                {
                    return Err(up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument,
                        "encoded_owned is a DDS owned family; DDS copy_minimized routing is unsupported"));
                }
                endpoint.native_context = endpoint
                    .native_deployment
                    .as_ref()
                    .map(|deployment| deployment.load_context(base, &[]))
                    .transpose()?;
                endpoint.native_profile = endpoint
                    .native_context
                    .as_ref()
                    .map(|context| context.agreement.clone());
            }
        }
        Ok(())
    }

    /// Validates complete native route membership and API classes before opening transports.
    pub(crate) fn native_route_policies(
        &self,
    ) -> Result<
        std::collections::HashMap<(String, String), up_streamer::NativeRoutePolicy>,
        up_rust::UStatus,
    > {
        use std::collections::{BTreeMap, BTreeSet, HashMap};
        use std::sync::Arc;
        use up_streamer::{NativeRouteApi, NativeRouteHop, NativeRoutePolicy};
        let invalid = |message: &str| {
            up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument, message)
        };
        let mut groups = vec![
            self.transports.zenoh.endpoints.as_slice(),
            self.transports.mqtt.endpoints.as_slice(),
        ];
        if let Some(value) = &self.transports.iceoryx2 {
            groups.push(&value.endpoints);
        }
        if let Some(value) = &self.transports.lola {
            groups.push(&value.endpoints);
        }
        if let Some(value) = &self.transports.dds {
            groups.push(&value.endpoints);
        }
        if let Some(value) = &self.transports.vsomeip {
            groups.push(&value.endpoints);
        }
        let mut endpoints = BTreeMap::new();
        for endpoint in groups.into_iter().flatten() {
            if endpoints
                .insert(endpoint.endpoint.as_str(), endpoint)
                .is_some()
            {
                return Err(invalid("duplicate configured endpoint"));
            }
        }
        let mut policies = HashMap::new();
        for ingress in endpoints.values() {
            if ingress.native_context.is_some() && !ingress.forwarding.is_empty() {
                return Err(invalid(
                    "native forwarding requires forwarding_routes with complete native_hops",
                ));
            }
            for route in &ingress.forwarding_routes {
                let native = match route.wire_format.as_deref() {
                    Some(format) => {
                        configurable_streamer_wire_support::RouteWireFormat::parse(format)?
                            == configurable_streamer_wire_support::RouteWireFormat::Native
                    }
                    None => ingress.native_context.is_some(),
                };
                if !native {
                    if !route.native_hops.is_empty()
                        || route.allow_contract_defined_over_classic_hops
                    {
                        return Err(invalid(
                            "native route policy requires a native deployment route",
                        ));
                    }
                    continue;
                }
                let egress = endpoints
                    .get(route.endpoint.as_str())
                    .ok_or_else(|| invalid("unknown native forwarding target"))?;
                let context = ingress
                    .native_context
                    .as_ref()
                    .ok_or_else(|| invalid("native route ingress lacks a loaded deployment"))?;
                let peer_context = egress
                    .native_context
                    .as_ref()
                    .ok_or_else(|| invalid("native route egress lacks a loaded deployment"))?;
                let declaration = ingress
                    .native_deployment
                    .as_ref()
                    .ok_or_else(|| invalid("native ingress locator missing"))?;
                let peer_declaration = egress
                    .native_deployment
                    .as_ref()
                    .ok_or_else(|| invalid("native egress locator missing"))?;
                let members: BTreeSet<_> = context.members.iter().map(String::as_str).collect();
                let declared: BTreeSet<_> = route
                    .native_hops
                    .iter()
                    .map(|hop| hop.endpoint.as_str())
                    .collect();
                if declared != members || declared.len() != route.native_hops.len() {
                    return Err(invalid(
                        "native_hops must name every provisioned route member exactly once",
                    ));
                }
                if declaration.route != peer_declaration.route
                    || context.agreement_sha256 != peer_context.agreement_sha256
                    || members != peer_context.members.iter().map(String::as_str).collect()
                {
                    return Err(invalid(
                        "native route endpoints disagree on full deployment artifact or membership",
                    ));
                }
                let mut bindings = BTreeMap::new();
                for candidate in endpoints.values() {
                    let Some(locator) = &candidate.native_deployment else {
                        continue;
                    };
                    if locator.route != declaration.route
                        || !members.contains(locator.local_endpoint.as_str())
                    {
                        continue;
                    }
                    let candidate_context = candidate
                        .native_context
                        .as_ref()
                        .ok_or_else(|| invalid("native route member lacks loaded deployment"))?;
                    if candidate_context.agreement_sha256 != context.agreement_sha256 {
                        return Err(invalid(
                            "configured native route member has a different agreement artifact",
                        ));
                    }
                    if bindings
                        .insert(locator.local_endpoint.as_str(), *candidate)
                        .is_some()
                    {
                        return Err(invalid(
                            "multiple configured endpoints claim the same native route member",
                        ));
                    }
                }
                let mut hops = Vec::with_capacity(route.native_hops.len());
                for hop in &route.native_hops {
                    let api = NativeRouteApi::parse(&hop.api)?;
                    let name = if let Some(binding) = bindings.get(hop.endpoint.as_str()) {
                        if binding.routing_mode.native_api() != api {
                            return Err(invalid(
                                "declared native hop API differs from the configured transport API",
                            ));
                        }
                        binding.endpoint.clone()
                    } else {
                        format!("deployment:{}", hop.endpoint)
                    };
                    hops.push(NativeRouteHop {
                        name,
                        api,
                        profile: Arc::new(context.agreement.profile().clone()),
                    });
                }
                let policy = NativeRoutePolicy::new(
                    format!(
                        "{}:{}->{}",
                        declaration.route, ingress.endpoint, egress.endpoint
                    ),
                    hops,
                )?
                .with_allow_contract_defined_over_classic_hops(
                    route.allow_contract_defined_over_classic_hops,
                );
                policy.validate_boundary(
                    (
                        &ingress.endpoint,
                        ingress.routing_mode.native_api(),
                        context.agreement.profile(),
                    ),
                    (
                        &egress.endpoint,
                        egress.routing_mode.native_api(),
                        peer_context.agreement.profile(),
                    ),
                )?;
                if policies
                    .insert((ingress.endpoint.clone(), egress.endpoint.clone()), policy)
                    .is_some()
                {
                    return Err(invalid("duplicate native forwarding route"));
                }
            }
        }
        Ok(policies)
    }
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
    /// Classic vSomeIP endpoints.
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

/// Classic vSomeIP transport section. `config_file` is the vsomeip JSON
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
    /// Optional actual subscriber discovery before a TX loan becomes sendable.
    #[serde(default)]
    pub(crate) publisher_readiness: Option<Iceoryx2PublisherReadiness>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Iceoryx2PublisherReadiness {
    pub(crate) minimum_subscribers: usize,
    pub(crate) timeout_ms: u64,
}

#[derive(Deserialize, Serialize, Debug, Clone, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Iceoryx2Namespace {
    /// Optional native receive-buffer capacity delegated to the binding runtime.
    pub(crate) subscriber_buffer_size: Option<usize>,
    /// Optional native borrowed-sample limit, distinct from application queue occupancy.
    pub(crate) max_borrowed_samples: Option<usize>,
    pub(crate) root_path: String,
    pub(crate) prefix: String,
}

impl Iceoryx2Namespace {
    fn validate(&self) -> Result<(), up_rust::UStatus> {
        if !std::path::Path::new(&self.root_path).is_absolute()
            || self.prefix.is_empty()
            || self.prefix.len() > 32
            || !self
                .prefix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument,
                "iceoryx2 namespace needs an absolute root_path and a 1..32 byte alphanumeric/underscore prefix"));
        }
        Ok(())
    }
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
    /// Selected-wire DDS encoded-owned carriage through the owned routing API.
    EncodedOwned,
    CopyMinimized,
}

impl RoutingMode {
    pub(crate) fn native_api(self) -> up_streamer::NativeRouteApi {
        match self {
            Self::Owned => up_streamer::NativeRouteApi::Classic,
            Self::OwnedFrame | Self::EncodedOwned => up_streamer::NativeRouteApi::OwnedFrame,
            Self::CopyMinimized => up_streamer::NativeRouteApi::NativeLoan,
        }
    }
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
    /// Optional Zenoh native transmit-pool size, forwarded to the binding builder.
    /// Omission keeps the binding's default; zero is rejected by the binding.
    pub(crate) zenoh_shm_segment_size: Option<usize>,
    /// Whether the native Zenoh publisher waits for an actual matching subscriber.
    #[serde(default)]
    pub(crate) zenoh_wait_for_subscriber: bool,
    #[serde(default)]
    pub(crate) zenoh_config_file: Option<String>,
    #[serde(default)]
    pub(crate) zenoh_client_config_file: Option<String>,
    #[serde(default)]
    pub(crate) iceoryx2_namespace: Option<Iceoryx2Namespace>,
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
    /// Independently provisioned native artifact/route locator for this endpoint.
    #[serde(default)]
    pub(crate) native_deployment:
        Option<configurable_streamer_wire_support::native_profile::NativeDeploymentConfig>,
    #[serde(skip)]
    pub(crate) native_profile: Option<up_rust::NativeProfileAgreement>,
    #[serde(skip)]
    pub(crate) native_context:
        Option<configurable_streamer_wire_support::native_profile::NativeDeploymentContext>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ForwardingRouteConfig {
    pub(crate) endpoint: String,
    /// Required when either boundary endpoint has a frame API, including one-way egress.
    #[serde(default)]
    pub(crate) wire_format: Option<String>,
    /// Every provisioned native route member, including remote classic hops.
    #[serde(default)]
    pub(crate) native_hops: Vec<NativeHopConfig>,
    /// Route-scoped acknowledgement of ContractDefined identity loss on classic hops.
    #[serde(default)]
    pub(crate) allow_contract_defined_over_classic_hops: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct NativeHopConfig {
    /// Endpoint identity from the provisioned deployment manifest.
    pub(crate) endpoint: String,
    /// Declared API class: classic, owned_frame or native_loan.
    pub(crate) api: String,
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

    fn two_bus_config(namespace_a: &str, namespace_b: &str) -> Config {
        json5::from_str(&format!(r#"{{
            up_streamer_config: {{ message_queue_size: 32 }},
            streamer_uuri: {{ authority: "bridge", ue_id: 1, ue_version_major: 1 }},
            usubscription_config: {{ file_path: "subscriptions.json" }},
            transports: {{ zenoh: {{config_file:"unused", endpoints:[]}}, mqtt: {{config_file:"unused", endpoints:[]}},
                iceoryx2: {{ endpoints: [
                    {{endpoint:"a",authority:"authority-a",routing_mode:"owned_frame", {namespace_a} forwarding:["b"]}},
                    {{endpoint:"b",authority:"authority-b",routing_mode:"copy_minimized", {namespace_b} forwarding:["a"]}}
                ] }} }} }}"#)).unwrap()
    }

    #[test]
    fn reflective_legacy_iceoryx2_config_is_rejected_before_activation() {
        let error = two_bus_config("", "")
            .validate_physical_domains()
            .unwrap_err();
        assert_eq!(error.code(), up_rust::UCode::InvalidArgument);
    }

    #[test]
    fn overlapping_native_prefixes_do_not_establish_physical_isolation() {
        let config = two_bus_config(
            "iceoryx2_namespace:{root_path:'/one',prefix:'bus_'},",
            "iceoryx2_namespace:{root_path:'/two',prefix:'bus_other_'},",
        );
        assert!(config.validate_physical_domains().is_err());
    }

    #[test]
    fn distinct_native_namespaces_allow_bidirectional_routes() {
        let config = two_bus_config(
            "iceoryx2_namespace:{root_path:'/same',prefix:'bus_a_'},",
            "iceoryx2_namespace:{root_path:'/same',prefix:'bus_b_'},",
        );
        config.validate_physical_domains().unwrap();
    }

    #[test]
    fn native_namespace_rejects_relative_root_or_invalid_prefix() {
        assert!(Iceoryx2Namespace {
            subscriber_buffer_size: None,
            max_borrowed_samples: None,
            root_path: "relative".into(),
            prefix: "valid_".into()
        }
        .validate()
        .is_err());
        assert!(Iceoryx2Namespace {
            subscriber_buffer_size: None,
            max_borrowed_samples: None,
            root_path: "/absolute".into(),
            prefix: "bad/prefix".into()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn iceoryx2_readiness_is_optional_and_explicitly_bounded() {
        let legacy: Iceoryx2Transport = json5::from_str("{ endpoints: [] }").unwrap();
        assert!(legacy.publisher_readiness.is_none());
        let finite: Iceoryx2Transport = json5::from_str(
            "{ endpoints: [], publisher_readiness: { minimum_subscribers: 1, timeout_ms: 5000 } }",
        )
        .unwrap();
        let readiness = finite.publisher_readiness.unwrap();
        assert_eq!(readiness.minimum_subscribers, 1);
        assert_eq!(readiness.timeout_ms, 5000);
        assert!(json5::from_str::<Iceoryx2Transport>(
            "{ endpoints: [], publisher_readiness: { minimum_subscribers: 1 } }"
        )
        .is_err());
    }

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
    fn retired_native_profile_fields_are_rejected() {
        for field in ["native_profile_file", "native_peer_profile_file"] {
            assert!(
                json5::from_str::<Config>(&base_config(&format!("{field}: 'old.json',"))).is_err()
            );
        }
        assert!(json5::from_str::<Config>(&base_config(
            "native_deployment: { profile_root: 'profiles' },"
        ))
        .is_err());
    }

    #[test]
    fn encoded_owned_is_not_a_native_endpoint_capability() {
        let mut config: Config = json5::from_str(&base_config("")).unwrap();
        config.transports.zenoh.endpoints[0].routing_mode = RoutingMode::EncodedOwned;
        assert!(config
            .load_native_profiles(std::path::Path::new("."))
            .is_err());
    }

    fn native_policy_config(contract: bool, classic_remote: bool, allow: bool) -> Config {
        use configurable_streamer_wire_support::native_profile::{
            matrix_table_profile_document, NativeDeploymentConfig, NativeDeploymentContext,
            SelectedWireNativePayload,
        };
        use std::sync::Arc;
        use up_rust::StablePayload;
        let mut config = two_bus_config(
            "iceoryx2_namespace:{root_path:'/same',prefix:'a_'},",
            "iceoryx2_namespace:{root_path:'/same',prefix:'b_'},",
        );
        let profile = if contract {
            up_rust::NativeProfile::new(
                "policy-test",
                1,
                up_rust::NativeProfileMode::ContractDefined(
                    up_rust::NativeContract::new(
                        "door-operation",
                        SelectedWireNativePayload::native_representation(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap()
        } else {
            matrix_table_profile_document().resolve().unwrap()
        };
        let agreement =
            up_rust::NativeProfileAgreement::new(Arc::new(profile.clone()), &profile).unwrap();
        for endpoint in &mut config.transports.iceoryx2.as_mut().unwrap().endpoints {
            endpoint.forwarding.clear();
            endpoint.native_deployment = Some(NativeDeploymentConfig {
                profile_root: "unit-fixture".into(),
                manifest: "manifest.json".into(),
                route: "whole".into(),
                local_endpoint: endpoint.endpoint.clone(),
                peer_endpoint: "remote".into(),
            });
            endpoint.native_profile = Some(agreement.clone());
            endpoint.native_context = Some(NativeDeploymentContext {
                agreement: agreement.clone(),
                members: ["remote", "a", "b"].map(str::to_owned).to_vec(),
                agreement_sha256: "a".repeat(64),
            });
        }
        config.transports.iceoryx2.as_mut().unwrap().endpoints[0]
            .forwarding_routes
            .push(ForwardingRouteConfig {
                endpoint: "b".into(),
                wire_format: Some("up_native".into()),
                allow_contract_defined_over_classic_hops: allow,
                native_hops: [
                    (
                        "remote",
                        if classic_remote {
                            "classic"
                        } else {
                            "owned_frame"
                        },
                    ),
                    ("a", "owned_frame"),
                    ("b", "native_loan"),
                ]
                .map(|(endpoint, api)| NativeHopConfig {
                    endpoint: endpoint.into(),
                    api: api.into(),
                })
                .to_vec(),
            });
        config
    }

    #[test]
    fn full_configuration_rejects_remote_classic_contract_without_per_route_opt_in() {
        assert!(native_policy_config(true, false, false)
            .native_route_policies()
            .is_ok());
        let error = native_policy_config(true, true, false)
            .native_route_policies()
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("allow_contract_defined_over_classic_hops"));
        assert!(native_policy_config(true, true, true)
            .native_route_policies()
            .is_ok());
        assert!(native_policy_config(false, true, false)
            .native_route_policies()
            .is_ok());
    }

    #[test]
    fn opt_in_cannot_omit_misclassify_or_change_a_provisioned_member() {
        for mutation in ["missing", "duplicate", "class", "artifact", "unloaded"] {
            let mut config = native_policy_config(true, true, true);
            let endpoints = &mut config.transports.iceoryx2.as_mut().unwrap().endpoints;
            match mutation {
                "missing" => {
                    endpoints[0].forwarding_routes[0].native_hops.remove(0);
                }
                "duplicate" => {
                    let hop = endpoints[0].forwarding_routes[0].native_hops[0].clone();
                    endpoints[0].forwarding_routes[0].native_hops.push(hop);
                }
                "class" => endpoints[0].forwarding_routes[0].native_hops[1].api = "classic".into(),
                "artifact" => {
                    endpoints[1]
                        .native_context
                        .as_mut()
                        .unwrap()
                        .agreement_sha256 = "b".repeat(64)
                }
                "unloaded" => endpoints[1].native_context = None,
                _ => unreachable!(),
            }
            assert!(config.native_route_policies().is_err(), "{mutation}");
        }
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
