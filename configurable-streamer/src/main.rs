/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
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

mod config;

use crate::config::{
    Config, EndpointConfig, ForwardingRouteConfig, RoutingMode, SubscriptionProviderMode,
};
use clap::Parser;
#[cfg(feature = "owned-frame-transport")]
use configurable_streamer_wire_support::RouteOwnedEndpoint;
#[cfg(feature = "experimental-copy-minimized-routing")]
use configurable_streamer_wire_support::RouteWireEndpoint;
use configurable_streamer_wire_support::RouteWireFormat;
use std::collections::HashMap;
#[cfg(any(
    feature = "experimental-copy-minimized-routing",
    feature = "owned-frame-transport"
))]
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
use std::path::{Path, PathBuf};
#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;
use up_rust::core::usubscription::USubscription;
use up_rust::{UCode, UStatus, UTransport, UUri};
#[cfg(feature = "experimental-copy-minimized-routing")]
use up_streamer::CopyMinimizedRouteOptions;
use up_streamer::{Endpoint, UStreamer};
#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "iceoryx2-zero-copy"
    ),
    feature = "iceoryx2-owned-frame"
))]
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
use up_transport_lola_rust::{LolaDefaultRxChannel, LolaTransportConfig, UTransportLola};
use up_transport_mqtt5::{Mqtt5Transport, Mqtt5TransportOptions, MqttClientOptions};
#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
use up_transport_zenoh::zenoh_config::EndPoint as ZenohEndPoint;
#[cfg(feature = "zenoh-owned-frame")]
use up_transport_zenoh::ZenohOwnedCore;
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "zenoh-zero-copy"
))]
use up_transport_zenoh::ZenohZeroCopyCore;
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};
use usubscription_static_file::USubscriptionStaticFile;

#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
const ZENOH_SELECTED_WIRE_CLIENT_ENDPOINT: &str = "tcp/127.0.0.1:7447";

#[derive(Parser)]
#[command()]
struct StreamerArgs {
    #[arg(short, long, value_name = "FILE")]
    config: String,
}

#[derive(Clone)]
struct ConfiguredEndpoint {
    routing_mode: RoutingMode,
    copy_minimized_payload_alignment: Option<usize>,
    standard: Option<Endpoint>,
    #[cfg(feature = "experimental-copy-minimized-routing")]
    route_wire_endpoints: HashMap<RouteWireFormat, RouteWireEndpoint>,
    #[cfg(feature = "owned-frame-transport")]
    owned_frame_endpoints: HashMap<RouteWireFormat, RouteOwnedEndpoint>,
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

fn insert_configured_endpoint(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_config: &EndpointConfig,
    endpoint: ConfiguredEndpoint,
) -> Result<(), UStatus> {
    if endpoints
        .insert(endpoint_config.endpoint.clone(), endpoint)
        .is_some()
    {
        return Err(invalid_config(format!(
            "Duplicate endpoint name found: {}",
            endpoint_config.endpoint
        )));
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct PendingRoute<'a> {
    target: &'a str,
    wire_format: Option<RouteWireFormat>,
}

fn endpoint_routes(endpoint_config: &EndpointConfig) -> Result<Vec<PendingRoute<'_>>, UStatus> {
    let mut routes = Vec::with_capacity(
        endpoint_config.forwarding.len() + endpoint_config.forwarding_routes.len(),
    );
    for target in &endpoint_config.forwarding {
        routes.push(PendingRoute {
            target,
            wire_format: None,
        });
    }
    for ForwardingRouteConfig {
        endpoint,
        wire_format,
    } in &endpoint_config.forwarding_routes
    {
        routes.push(PendingRoute {
            target: endpoint,
            wire_format: wire_format
                .as_deref()
                .map(RouteWireFormat::parse)
                .transpose()?,
        });
    }
    Ok(routes)
}

#[cfg(any(
    feature = "experimental-copy-minimized-routing",
    feature = "owned-frame-transport"
))]
fn required_route_wire_format(
    endpoint_name: &str,
    target_name: &str,
    configured: Option<RouteWireFormat>,
) -> Result<RouteWireFormat, UStatus> {
    configured.ok_or_else(|| {
        invalid_config(format!(
            "selected-wire route {endpoint_name}->{target_name} requires wire_format"
        ))
    })
}

#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
fn zenoh_config_file_for_endpoint<'a>(
    endpoint_config: &'a EndpointConfig,
    default_config_file: &'a str,
) -> &'a str {
    endpoint_config
        .zenoh_config_file
        .as_deref()
        .unwrap_or(default_config_file)
}

#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
fn load_zenoh_config_file(config_file: &str, purpose: &str) -> Result<ZenohConfig, UStatus> {
    ZenohConfig::from_file(config_file).map_err(|e| {
        UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!("Unable to load Zenoh config file for {purpose}: {e:?}"),
        )
    })
}

#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
fn default_zenoh_client_config() -> Result<ZenohConfig, UStatus> {
    let mut config = ZenohConfig::default();
    let endpoint = ZenohEndPoint::from_str(ZENOH_SELECTED_WIRE_CLIENT_ENDPOINT).map_err(|e| {
        UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!("Unable to parse Zenoh selected-wire client endpoint: {e:?}"),
        )
    })?;
    config.connect.endpoints.set(vec![endpoint]).map_err(|e| {
        UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!("Unable to configure Zenoh selected-wire client endpoint: {e:?}"),
        )
    })?;
    Ok(config)
}

#[cfg(any(feature = "zenoh-zero-copy", feature = "zenoh-owned-frame"))]
fn zenoh_selected_wire_config(
    endpoint_config: &EndpointConfig,
    default_config_file: &str,
    opened_router_configs: &mut HashSet<String>,
) -> Result<ZenohConfig, UStatus> {
    let config_file = zenoh_config_file_for_endpoint(endpoint_config, default_config_file);
    let use_client_config = !opened_router_configs.insert(config_file.to_string());
    if use_client_config {
        return match endpoint_config.zenoh_client_config_file.as_deref() {
            Some(client_config_file) => {
                load_zenoh_config_file(client_config_file, "selected-wire client endpoint")
            }
            None => default_zenoh_client_config(),
        };
    }

    load_zenoh_config_file(config_file, "selected-wire endpoint")
}

#[cfg(any(
    feature = "experimental-copy-minimized-routing",
    feature = "owned-frame-transport"
))]
fn collect_route_wire_formats(
    endpoint_configs: &[&[EndpointConfig]],
) -> Result<HashMap<String, HashSet<RouteWireFormat>>, UStatus> {
    let mut route_wire_formats: HashMap<String, HashSet<RouteWireFormat>> = HashMap::new();
    for configs in endpoint_configs {
        for endpoint_config in *configs {
            if !matches!(
                endpoint_config.routing_mode,
                RoutingMode::CopyMinimized | RoutingMode::OwnedFrame
            ) {
                continue;
            }
            for route in endpoint_routes(endpoint_config)? {
                let route_wire_format = required_route_wire_format(
                    &endpoint_config.endpoint,
                    route.target,
                    route.wire_format,
                )?;
                route_wire_formats
                    .entry(endpoint_config.endpoint.clone())
                    .or_default()
                    .insert(route_wire_format);
                route_wire_formats
                    .entry(route.target.to_string())
                    .or_default()
                    .insert(route_wire_format);
            }
        }
    }
    Ok(route_wire_formats)
}

/// R3A: resolves an endpoint's configured per-topic payload-encoding
/// convention. Note: registered literals resolve through `from_parts` here;
/// once the R3A-U normalization guard lands, this call site inherits the
/// canonical-triple hydration for free.
fn resolve_assumed_payload_encoding(
    endpoint_config: &EndpointConfig,
) -> Result<Option<up_rust::PayloadEncoding>, UStatus> {
    match (
        &endpoint_config.assumed_payload_encoding,
        &endpoint_config.assumed_payload_content_type,
    ) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(invalid_config(format!(
            "endpoint {} sets assumed_payload_content_type without assumed_payload_encoding",
            endpoint_config.endpoint
        ))),
        (Some(literal), content_type) => {
            up_rust::PayloadEncoding::from_parts(None, Some(literal.clone()), content_type.clone())
                .map(Some)
                .map_err(|e| {
                    invalid_config(format!(
                        "endpoint {}: invalid assumed payload encoding: {e}",
                        endpoint_config.endpoint
                    ))
                })
        }
    }
}

fn resolve_vsomeip_payload_encoding(
    endpoint_configs: &[EndpointConfig],
) -> Result<up_rust::PayloadEncoding, UStatus> {
    let mut resolved: Option<up_rust::PayloadEncoding> = None;
    for endpoint_config in endpoint_configs {
        let Some(encoding) = resolve_assumed_payload_encoding(endpoint_config)? else {
            return Err(invalid_config(format!(
                "vSomeIP endpoint {} requires assumed_payload_encoding: the \
                 SOME/IP wire carries no payload-encoding metadata, so the \
                 per-topic convention must be declared explicitly",
                endpoint_config.endpoint
            )));
        };

        if let Some(existing) = &resolved {
            if existing != &encoding {
                return Err(invalid_config(format!(
                    "all vSomeIP endpoints in one streamer process must use the same \
                     assumed_payload_encoding; endpoint {} uses `{}` but earlier \
                     vSomeIP endpoints use `{}`",
                    endpoint_config.endpoint,
                    encoding.describe(),
                    existing.describe()
                )));
            }
        } else {
            resolved = Some(encoding);
        }
    }

    resolved.ok_or_else(|| invalid_config("vSomeIP transport has no endpoints".to_string()))
}

/// R3A: wraps a classic transport in the assumed-encoding decorator when the
/// endpoint declares a per-topic convention. Do not use this for vSomeIP: that
/// transport reconstructs and validates its configured convention directly.
#[cfg(feature = "owned-frame-transport")]
fn classic_transport_for(
    transport: Arc<dyn UTransport>,
    endpoint_config: &EndpointConfig,
) -> Result<Arc<dyn UTransport>, UStatus> {
    Ok(match resolve_assumed_payload_encoding(endpoint_config)? {
        Some(encoding) => Arc::new(up_streamer::AssumedEncodingTransport::new(
            transport, encoding,
        )),
        None => transport,
    })
}

#[cfg(not(feature = "owned-frame-transport"))]
fn classic_transport_for(
    transport: Arc<dyn UTransport>,
    endpoint_config: &EndpointConfig,
) -> Result<Arc<dyn UTransport>, UStatus> {
    if endpoint_config.assumed_payload_encoding.is_some() {
        return Err(invalid_config(format!(
            "endpoint {} sets assumed_payload_encoding, which requires the owned-frame-transport feature",
            endpoint_config.endpoint
        )));
    }
    Ok(transport)
}

fn standard_endpoint<'a>(
    endpoint: &'a ConfiguredEndpoint,
    endpoint_name: &str,
) -> Result<&'a Endpoint, UStatus> {
    endpoint.standard.as_ref().ok_or_else(|| {
        invalid_config(format!(
            "endpoint {endpoint_name} is not available for regular UTransport routing"
        ))
    })
}

#[cfg(feature = "experimental-copy-minimized-routing")]
fn route_wire_endpoint<'a>(
    endpoint: &'a ConfiguredEndpoint,
    endpoint_name: &str,
    route_wire_format: RouteWireFormat,
) -> Result<&'a RouteWireEndpoint, UStatus> {
    endpoint
        .route_wire_endpoints
        .get(&route_wire_format)
        .ok_or_else(|| {
            invalid_config(format!(
                "endpoint {endpoint_name} is not available for requested route wire format"
            ))
        })
}

#[cfg(feature = "owned-frame-transport")]
fn owned_frame_endpoint<'a>(
    endpoint: &'a ConfiguredEndpoint,
    endpoint_name: &str,
    route_wire_format: RouteWireFormat,
) -> Result<&'a RouteOwnedEndpoint, UStatus> {
    endpoint
        .owned_frame_endpoints
        .get(&route_wire_format)
        .ok_or_else(|| {
            invalid_config(format!(
                "endpoint {endpoint_name} is not available for requested owned-frame wire format"
            ))
        })
}

fn ensure_copy_minimized_endpoint(
    endpoint: &ConfiguredEndpoint,
    endpoint_name: &str,
) -> Result<(), UStatus> {
    #[cfg(feature = "experimental-copy-minimized-routing")]
    {
        if !endpoint.route_wire_endpoints.is_empty() {
            return Ok(());
        }
        Err(invalid_config(format!(
            "endpoint {endpoint_name} uses copy_minimized routing but no route wire endpoint is available"
        )))
    }

    #[cfg(not(feature = "experimental-copy-minimized-routing"))]
    {
        let _ = endpoint;
        Err(invalid_config(format!(
            "endpoint {endpoint_name} uses copy_minimized routing but configurable-streamer was not built with experimental-copy-minimized-routing"
        )))
    }
}

fn ensure_owned_frame_endpoint(
    endpoint: &ConfiguredEndpoint,
    endpoint_name: &str,
) -> Result<(), UStatus> {
    #[cfg(feature = "owned-frame-transport")]
    {
        if !endpoint.owned_frame_endpoints.is_empty() {
            return Ok(());
        }
        Err(invalid_config(format!(
            "endpoint {endpoint_name} uses owned_frame routing but no owned-frame endpoint is available"
        )))
    }

    #[cfg(not(feature = "owned-frame-transport"))]
    {
        let _ = endpoint;
        Err(invalid_config(format!(
            "endpoint {endpoint_name} uses owned_frame routing but configurable-streamer was not built with owned-frame-transport"
        )))
    }
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "zenoh-zero-copy"
))]
async fn zenoh_route_wire_endpoints(
    endpoint_config: &EndpointConfig,
    config_file: &str,
    streamer_uri: &str,
    route_wire_formats: Option<&HashSet<RouteWireFormat>>,
    opened_router_configs: &mut HashSet<String>,
) -> Result<HashMap<RouteWireFormat, RouteWireEndpoint>, UStatus> {
    let mut endpoints = HashMap::new();
    if let Some(route_wire_formats) = route_wire_formats {
        for route_wire_format in route_wire_formats {
            let zenoh_config =
                zenoh_selected_wire_config(endpoint_config, config_file, opened_router_configs)?;
            let core = ZenohZeroCopyCore::new(zenoh_config, streamer_uri.to_string()).await?;
            endpoints.insert(
                *route_wire_format,
                configurable_streamer_wire_support::zenoh_endpoint(
                    &endpoint_config.endpoint,
                    &endpoint_config.authority,
                    core,
                    *route_wire_format,
                ),
            );
        }
    }
    Ok(endpoints)
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    not(feature = "zenoh-zero-copy")
))]
async fn zenoh_route_wire_endpoints(
    _endpoint_config: &EndpointConfig,
    _config_file: &str,
    _streamer_uri: &str,
    route_wire_formats: Option<&HashSet<RouteWireFormat>>,
    _opened_router_configs: &mut HashSet<String>,
) -> Result<HashMap<RouteWireFormat, RouteWireEndpoint>, UStatus> {
    if route_wire_formats.is_some_and(|formats| !formats.is_empty()) {
        return Err(invalid_config(
            "Zenoh route wire formats require configurable-streamer feature zenoh-zero-copy",
        ));
    }
    Ok(HashMap::new())
}

#[cfg(feature = "zenoh-owned-frame")]
async fn zenoh_owned_frame_endpoints(
    endpoint_config: &EndpointConfig,
    config_file: &str,
    streamer_uri: &str,
    route_wire_formats: Option<&HashSet<RouteWireFormat>>,
    owned_cores: &mut HashMap<RouteWireFormat, ZenohOwnedCore>,
    opened_router_configs: &mut HashSet<String>,
) -> Result<HashMap<RouteWireFormat, RouteOwnedEndpoint>, UStatus> {
    let mut endpoints = HashMap::new();
    if let Some(route_wire_formats) = route_wire_formats {
        for route_wire_format in route_wire_formats {
            let core = if let Some(core) = owned_cores.get(route_wire_format) {
                core.clone()
            } else {
                let zenoh_config = zenoh_selected_wire_config(
                    endpoint_config,
                    config_file,
                    opened_router_configs,
                )?;
                let core = ZenohOwnedCore::new(zenoh_config, streamer_uri.to_string()).await?;
                owned_cores.insert(*route_wire_format, core.clone());
                core
            };
            endpoints.insert(
                *route_wire_format,
                configurable_streamer_wire_support::zenoh_owned_endpoint(
                    &endpoint_config.endpoint,
                    &endpoint_config.authority,
                    core,
                    *route_wire_format,
                ),
            );
        }
    }
    Ok(endpoints)
}

#[cfg(all(feature = "owned-frame-transport", not(feature = "zenoh-owned-frame")))]
async fn zenoh_owned_frame_endpoints(
    _endpoint_config: &EndpointConfig,
    _config_file: &str,
    _streamer_uri: &str,
    route_wire_formats: Option<&HashSet<RouteWireFormat>>,
    _opened_router_configs: &mut HashSet<String>,
) -> Result<HashMap<RouteWireFormat, RouteOwnedEndpoint>, UStatus> {
    if route_wire_formats.is_some_and(|formats| !formats.is_empty()) {
        return Err(invalid_config(
            "Zenoh owned_frame routes require configurable-streamer feature zenoh-owned-frame",
        ));
    }
    Ok(HashMap::new())
}

async fn register_zenoh_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    config_file: &str,
    streamer_uri: &str,
    #[cfg(any(
        feature = "experimental-copy-minimized-routing",
        feature = "owned-frame-transport"
    ))]
    route_wire_formats: &HashMap<String, HashSet<RouteWireFormat>>,
    transport: Option<Arc<UPTransportZenoh>>,
) -> Result<(), UStatus> {
    #[cfg(not(any(
        feature = "experimental-copy-minimized-routing",
        feature = "owned-frame-transport"
    )))]
    {
        let _ = (config_file, streamer_uri);
    }
    #[cfg(feature = "zenoh-owned-frame")]
    let mut zenoh_owned_cores: HashMap<RouteWireFormat, ZenohOwnedCore> = HashMap::new();
    #[cfg(any(
        feature = "experimental-copy-minimized-routing",
        feature = "owned-frame-transport"
    ))]
    let mut opened_router_configs: HashSet<String> = {
        let mut configs = HashSet::new();
        if transport.is_some() {
            configs.insert(config_file.to_string());
        }
        configs
    };

    for endpoint_config in endpoint_configs {
        let standard = if endpoint_config.routing_mode != RoutingMode::Owned {
            None
        } else {
            let standard_transport: Arc<dyn UTransport> = if let Some(endpoint_config_file) =
                endpoint_config.zenoh_config_file.as_deref()
            {
                let zenoh_config = ZenohConfig::from_file(endpoint_config_file).map_err(|e| {
                    UStatus::fail_with_code(
                        UCode::InvalidArgument,
                        format!("Unable to load Zenoh config file: {e:?}"),
                    )
                })?;
                #[cfg(any(
                    feature = "experimental-copy-minimized-routing",
                    feature = "owned-frame-transport"
                ))]
                opened_router_configs.insert(endpoint_config_file.to_string());
                Arc::new(
                    UPTransportZenoh::new(zenoh_config, streamer_uri.to_string())
                        .await
                        .map_err(|e| {
                            UStatus::fail_with_code(
                                UCode::Internal,
                                format!("Unable to initialize Zenoh UTransport: {e:?}"),
                            )
                        })?,
                )
            } else {
                let transport = transport.as_ref().ok_or_else(|| {
                        invalid_config(format!(
                            "Zenoh endpoint {} requires regular UTransport routing but no Zenoh UTransport was initialized",
                            endpoint_config.endpoint
                        ))
                    })?;
                transport.clone()
            };
            Some(Endpoint::new(
                &endpoint_config.endpoint,
                &endpoint_config.authority,
                standard_transport,
            ))
        };
        #[cfg(any(
            feature = "experimental-copy-minimized-routing",
            feature = "owned-frame-transport"
        ))]
        let endpoint_route_wire_formats = route_wire_formats.get(&endpoint_config.endpoint);
        #[cfg(feature = "experimental-copy-minimized-routing")]
        let copy_minimized_route_wire_formats =
            if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
                endpoint_route_wire_formats
            } else {
                None
            };
        #[cfg(feature = "owned-frame-transport")]
        let owned_frame_route_wire_formats =
            if endpoint_config.routing_mode == RoutingMode::OwnedFrame {
                endpoint_route_wire_formats
            } else {
                None
            };
        let endpoint = ConfiguredEndpoint {
            routing_mode: endpoint_config.routing_mode,
            copy_minimized_payload_alignment: endpoint_config.copy_minimized_payload_alignment,
            standard,
            #[cfg(feature = "experimental-copy-minimized-routing")]
            route_wire_endpoints: zenoh_route_wire_endpoints(
                endpoint_config,
                config_file,
                streamer_uri,
                copy_minimized_route_wire_formats,
                &mut opened_router_configs,
            )
            .await?,
            #[cfg(feature = "owned-frame-transport")]
            owned_frame_endpoints: {
                #[cfg(feature = "zenoh-owned-frame")]
                {
                    zenoh_owned_frame_endpoints(
                        endpoint_config,
                        config_file,
                        streamer_uri,
                        owned_frame_route_wire_formats,
                        &mut zenoh_owned_cores,
                        &mut opened_router_configs,
                    )
                    .await?
                }
                #[cfg(not(feature = "zenoh-owned-frame"))]
                {
                    zenoh_owned_frame_endpoints(
                        endpoint_config,
                        config_file,
                        streamer_uri,
                        owned_frame_route_wire_formats,
                        &mut opened_router_configs,
                    )
                    .await?
                }
            },
        };
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            ensure_copy_minimized_endpoint(&endpoint, &endpoint_config.endpoint)?;
        }
        if endpoint_config.routing_mode == RoutingMode::OwnedFrame {
            ensure_owned_frame_endpoint(&endpoint, &endpoint_config.endpoint)?;
        }
        insert_configured_endpoint(endpoints, endpoint_config, endpoint)?;
    }

    Ok(())
}

/// R3A Tier 1: classic vSomeIP endpoints. `routing_mode` must be `owned`
/// (selected-wire over SOME/IP is the registered Tier-3 decision,
/// `vsomeip.uframe.native.v1`). The assumed-encoding convention is REQUIRED:
/// the SOME/IP wire carries no payload-encoding metadata (binding spec:
/// "fixed per topic"), so the convention must be declared to be auditable.
#[cfg(feature = "vsomeip-transport")]
fn register_vsomeip_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    transport: Arc<dyn UTransport>,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        match endpoint_config.routing_mode {
            RoutingMode::Owned => {}
            RoutingMode::CopyMinimized | RoutingMode::OwnedFrame => {
                return Err(invalid_config(format!(
                    "vSomeIP endpoint {} supports classic (owned) routing only; \
                     selected-wire over SOME/IP is the registered \
                     vsomeip.uframe.native.v1 decision (Tier 3)",
                    endpoint_config.endpoint
                )));
            }
        }
        if endpoint_config.assumed_payload_encoding.is_none() {
            return Err(invalid_config(format!(
                "vSomeIP endpoint {} requires assumed_payload_encoding: the \
                 SOME/IP wire carries no payload-encoding metadata, so the \
                 per-topic convention must be declared explicitly",
                endpoint_config.endpoint
            )));
        }
        insert_configured_endpoint(
            endpoints,
            endpoint_config,
            ConfiguredEndpoint {
                routing_mode: endpoint_config.routing_mode,
                copy_minimized_payload_alignment: endpoint_config.copy_minimized_payload_alignment,
                standard: Some(Endpoint::new(
                    &endpoint_config.endpoint,
                    &endpoint_config.authority,
                    transport.clone(),
                )),
                #[cfg(feature = "experimental-copy-minimized-routing")]
                route_wire_endpoints: HashMap::new(),
                #[cfg(feature = "owned-frame-transport")]
                owned_frame_endpoints: HashMap::new(),
            },
        )?;
    }

    Ok(())
}

fn register_mqtt_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    transport: Arc<dyn UTransport>,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        match endpoint_config.routing_mode {
            RoutingMode::Owned => {}
            RoutingMode::CopyMinimized => {
                return Err(invalid_config(format!(
                    "MQTT endpoint {} cannot use copy_minimized routing",
                    endpoint_config.endpoint
                )));
            }
            RoutingMode::OwnedFrame => {
                return Err(invalid_config(format!(
                    "MQTT endpoint {} cannot use owned_frame routing",
                    endpoint_config.endpoint
                )));
            }
        }
        insert_configured_endpoint(
            endpoints,
            endpoint_config,
            ConfiguredEndpoint {
                routing_mode: endpoint_config.routing_mode,
                copy_minimized_payload_alignment: endpoint_config.copy_minimized_payload_alignment,
                standard: Some(Endpoint::new(
                    &endpoint_config.endpoint,
                    &endpoint_config.authority,
                    classic_transport_for(transport.clone(), endpoint_config)?,
                )),
                #[cfg(feature = "experimental-copy-minimized-routing")]
                route_wire_endpoints: HashMap::new(),
                #[cfg(feature = "owned-frame-transport")]
                owned_frame_endpoints: HashMap::new(),
            },
        )?;
    }

    Ok(())
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "iceoryx2-zero-copy"
    ),
    feature = "iceoryx2-owned-frame"
))]
fn register_iceoryx2_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    route_wire_formats: &HashMap<String, HashSet<RouteWireFormat>>,
    transport: Iceoryx2PubSub,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        if endpoint_config.routing_mode == RoutingMode::Owned {
            return Err(invalid_config(format!(
                "iceoryx2 endpoint {} must use copy_minimized or owned_frame routing",
                endpoint_config.endpoint
            )));
        }
        #[cfg(feature = "experimental-copy-minimized-routing")]
        let mut route_wire_endpoints = HashMap::new();
        #[cfg(all(
            feature = "experimental-copy-minimized-routing",
            feature = "iceoryx2-zero-copy"
        ))]
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            if let Some(route_wire_formats) = route_wire_formats.get(&endpoint_config.endpoint) {
                for route_wire_format in route_wire_formats {
                    route_wire_endpoints.insert(
                        *route_wire_format,
                        configurable_streamer_wire_support::iceoryx2_endpoint(
                            &endpoint_config.endpoint,
                            &endpoint_config.authority,
                            transport.clone(),
                            *route_wire_format,
                        ),
                    );
                }
            }
        }
        #[cfg(feature = "owned-frame-transport")]
        let mut owned_frame_endpoints = HashMap::new();
        #[cfg(feature = "iceoryx2-owned-frame")]
        if endpoint_config.routing_mode == RoutingMode::OwnedFrame {
            if let Some(route_wire_formats) = route_wire_formats.get(&endpoint_config.endpoint) {
                for route_wire_format in route_wire_formats {
                    owned_frame_endpoints.insert(
                        *route_wire_format,
                        configurable_streamer_wire_support::iceoryx2_owned_endpoint(
                            &endpoint_config.endpoint,
                            &endpoint_config.authority,
                            up_transport_iceoryx2_rust::BenchmarkOwnedIceoryx2Core::new(
                                transport.clone(),
                            ),
                            *route_wire_format,
                        ),
                    );
                }
            }
        }
        let endpoint = ConfiguredEndpoint {
            routing_mode: endpoint_config.routing_mode,
            copy_minimized_payload_alignment: endpoint_config.copy_minimized_payload_alignment,
            standard: None,
            #[cfg(feature = "experimental-copy-minimized-routing")]
            route_wire_endpoints,
            #[cfg(feature = "owned-frame-transport")]
            owned_frame_endpoints,
        };
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            ensure_copy_minimized_endpoint(&endpoint, &endpoint_config.endpoint)?;
        }
        if endpoint_config.routing_mode == RoutingMode::OwnedFrame {
            ensure_owned_frame_endpoint(&endpoint, &endpoint_config.endpoint)?;
        }
        insert_configured_endpoint(endpoints, endpoint_config, endpoint)?;
    }

    Ok(())
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn register_lola_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    route_wire_formats: &HashMap<String, HashSet<RouteWireFormat>>,
    config_dir: &Path,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        if endpoint_config.routing_mode == RoutingMode::Owned {
            return Err(invalid_config(format!(
                "LoLa endpoint {} must use copy_minimized or owned_frame routing",
                endpoint_config.endpoint
            )));
        }
        let transport = lola_transport(endpoint_config, config_dir)?;
        #[cfg(feature = "experimental-copy-minimized-routing")]
        let mut route_wire_endpoints = HashMap::new();
        #[cfg(all(
            feature = "experimental-copy-minimized-routing",
            feature = "lola-transport"
        ))]
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            if let Some(route_wire_formats) = route_wire_formats.get(&endpoint_config.endpoint) {
                for route_wire_format in route_wire_formats {
                    route_wire_endpoints.insert(
                        *route_wire_format,
                        configurable_streamer_wire_support::lola_endpoint(
                            &endpoint_config.endpoint,
                            &endpoint_config.authority,
                            transport.zero_copy_core(),
                            *route_wire_format,
                        ),
                    );
                }
            }
        }
        #[cfg(feature = "owned-frame-transport")]
        let mut owned_frame_endpoints = HashMap::new();
        #[cfg(feature = "lola-owned-frame")]
        if endpoint_config.routing_mode == RoutingMode::OwnedFrame {
            if let Some(route_wire_formats) = route_wire_formats.get(&endpoint_config.endpoint) {
                for route_wire_format in route_wire_formats {
                    owned_frame_endpoints.insert(
                        *route_wire_format,
                        configurable_streamer_wire_support::lola_owned_endpoint(
                            &endpoint_config.endpoint,
                            &endpoint_config.authority,
                            up_transport_lola_rust::LolaOwnedCore::new(transport.zero_copy_core()),
                            *route_wire_format,
                        ),
                    );
                }
            }
        }
        insert_configured_endpoint(
            endpoints,
            endpoint_config,
            ConfiguredEndpoint {
                routing_mode: endpoint_config.routing_mode,
                copy_minimized_payload_alignment: endpoint_config.copy_minimized_payload_alignment,
                standard: None,
                #[cfg(feature = "experimental-copy-minimized-routing")]
                route_wire_endpoints,
                #[cfg(feature = "owned-frame-transport")]
                owned_frame_endpoints,
            },
        )?;
        let endpoint = endpoints
            .get(&endpoint_config.endpoint)
            .expect("endpoint inserted immediately above");
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            ensure_copy_minimized_endpoint(endpoint, &endpoint_config.endpoint)?;
        }
        if endpoint_config.routing_mode == RoutingMode::OwnedFrame {
            ensure_owned_frame_endpoint(endpoint, &endpoint_config.endpoint)?;
        }
    }

    Ok(())
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn lola_transport_config(
    endpoint_config: &EndpointConfig,
    config_dir: &Path,
) -> Result<LolaTransportConfig, UStatus> {
    Ok(LolaTransportConfig {
        local_authority: endpoint_config.authority.clone(),
        instance_specifier: required_lola_string(
            endpoint_config,
            "lola_instance_specifier",
            &endpoint_config.lola_instance_specifier,
        )?,
        service_type: required_lola_string(
            endpoint_config,
            "lola_service_type",
            &endpoint_config.lola_service_type,
        )?,
        event_name: required_lola_string(
            endpoint_config,
            "lola_event_name",
            &endpoint_config.lola_event_name,
        )?,
        sample_size: required_lola_usize(
            endpoint_config,
            "lola_sample_size",
            endpoint_config.lola_sample_size,
        )?,
        sample_alignment: required_lola_usize(
            endpoint_config,
            "lola_sample_alignment",
            endpoint_config.lola_sample_alignment,
        )?,
        max_samples: required_lola_usize(
            endpoint_config,
            "lola_max_samples",
            endpoint_config.lola_max_samples,
        )?,
        pull_mismatch_queue_capacity: LolaTransportConfig::DEFAULT_PULL_MISMATCH_QUEUE_CAPACITY,
        pull_mismatch_queue_full_policy:
            LolaTransportConfig::DEFAULT_PULL_MISMATCH_QUEUE_FULL_POLICY,
        mw_com_config_path: Some(required_lola_mw_com_manifest_path(
            endpoint_config,
            config_dir,
            "lola_mw_com_config_file",
            endpoint_config.lola_mw_com_config_file.as_ref(),
        )?),
    })
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn lola_transport(
    endpoint_config: &EndpointConfig,
    config_dir: &Path,
) -> Result<Arc<UTransportLola>, UStatus> {
    let base = lola_transport_config(endpoint_config, config_dir)?;
    let response = lola_response_transport_config(endpoint_config, config_dir, &base)?;
    UTransportLola::build_with_response_channel_and_default_rx(
        base,
        response,
        lola_default_rx_channel(endpoint_config)?,
    )
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn lola_default_rx_channel(
    endpoint_config: &EndpointConfig,
) -> Result<LolaDefaultRxChannel, UStatus> {
    match endpoint_config
        .lola_default_rx_channel
        .as_deref()
        .unwrap_or("primary")
    {
        "primary" => Ok(LolaDefaultRxChannel::Primary),
        "response" => Ok(LolaDefaultRxChannel::Response),
        "both" => Ok(LolaDefaultRxChannel::Both),
        other => Err(invalid_config(format!(
            "LoLa endpoint {} has unsupported lola_default_rx_channel {other}",
            endpoint_config.endpoint
        ))),
    }
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn lola_response_transport_config(
    endpoint_config: &EndpointConfig,
    config_dir: &Path,
    base: &LolaTransportConfig,
) -> Result<Option<LolaTransportConfig>, UStatus> {
    let has_response = endpoint_config.lola_response_instance_specifier.is_some()
        || endpoint_config.lola_response_service_type.is_some()
        || endpoint_config.lola_response_event_name.is_some()
        || endpoint_config.lola_response_mw_com_config_file.is_some();
    if !has_response {
        return Ok(None);
    }

    Ok(Some(LolaTransportConfig {
        local_authority: base.local_authority.clone(),
        instance_specifier: required_lola_string(
            endpoint_config,
            "lola_response_instance_specifier",
            &endpoint_config.lola_response_instance_specifier,
        )?,
        service_type: required_lola_string(
            endpoint_config,
            "lola_response_service_type",
            &endpoint_config.lola_response_service_type,
        )?,
        event_name: required_lola_string(
            endpoint_config,
            "lola_response_event_name",
            &endpoint_config.lola_response_event_name,
        )?,
        sample_size: base.sample_size,
        sample_alignment: base.sample_alignment,
        max_samples: base.max_samples,
        pull_mismatch_queue_capacity: base.pull_mismatch_queue_capacity,
        pull_mismatch_queue_full_policy: base.pull_mismatch_queue_full_policy,
        mw_com_config_path: Some(required_lola_mw_com_manifest_path(
            endpoint_config,
            config_dir,
            "lola_response_mw_com_config_file",
            endpoint_config
                .lola_response_mw_com_config_file
                .as_ref()
                .or(endpoint_config.lola_mw_com_config_file.as_ref()),
        )?),
    }))
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn validate_lola_mw_com_manifests(
    endpoint_configs: &[EndpointConfig],
    config_dir: &Path,
) -> Result<(), UStatus> {
    let mut process_manifest: Option<String> = None;
    for endpoint_config in endpoint_configs {
        let base_manifest = required_lola_mw_com_manifest_path(
            endpoint_config,
            config_dir,
            "lola_mw_com_config_file",
            endpoint_config.lola_mw_com_config_file.as_ref(),
        )?;
        ensure_same_lola_manifest(&mut process_manifest, &base_manifest, endpoint_config)?;

        if let Some(response_manifest) = &endpoint_config.lola_response_mw_com_config_file {
            let response_manifest =
                canonical_lola_mw_com_manifest_path(config_dir, response_manifest)?;
            ensure_same_lola_manifest(&mut process_manifest, &response_manifest, endpoint_config)?;
        }
    }
    Ok(())
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn ensure_same_lola_manifest(
    process_manifest: &mut Option<String>,
    manifest: &str,
    endpoint_config: &EndpointConfig,
) -> Result<(), UStatus> {
    if let Some(existing_manifest) = process_manifest.as_deref() {
        if existing_manifest != manifest {
            return Err(invalid_config(format!(
                "LoLa endpoint {} uses MW COM manifest {manifest}, but this streamer process already uses {existing_manifest}; configure one complete LoLa manifest per process",
                endpoint_config.endpoint
            )));
        }
        return Ok(());
    }

    *process_manifest = Some(manifest.to_string());
    Ok(())
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn required_lola_mw_com_manifest_path(
    endpoint_config: &EndpointConfig,
    config_dir: &Path,
    field: &str,
    configured_path: Option<&String>,
) -> Result<String, UStatus> {
    let path = configured_path.ok_or_else(|| {
        invalid_config(format!(
            "LoLa endpoint {} requires {field}; configurable-streamer examples must use an explicit checked-in MW COM manifest",
            endpoint_config.endpoint
        ))
    })?;
    canonical_lola_mw_com_manifest_path(config_dir, path)
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn canonical_lola_mw_com_manifest_path(
    config_dir: &Path,
    configured_path: &str,
) -> Result<String, UStatus> {
    let resolved = resolve_config_relative_path(config_dir, configured_path);
    std::fs::canonicalize(&resolved)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            invalid_config(format!(
                "LoLa MW COM manifest {configured_path} resolved to {resolved} but could not be loaded: {error}"
            ))
        })
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn required_lola_string(
    endpoint_config: &EndpointConfig,
    field: &str,
    value: &Option<String>,
) -> Result<String, UStatus> {
    value
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            invalid_config(format!(
                "LoLa endpoint {} requires non-empty {field}",
                endpoint_config.endpoint
            ))
        })
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn required_lola_usize(
    endpoint_config: &EndpointConfig,
    field: &str,
    value: Option<usize>,
) -> Result<usize, UStatus> {
    value.ok_or_else(|| {
        invalid_config(format!(
            "LoLa endpoint {} requires {field}",
            endpoint_config.endpoint
        ))
    })
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn config_parent(config_file: &str) -> PathBuf {
    Path::new(config_file)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(any(
    all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
    ),
    feature = "lola-owned-frame"
))]
fn resolve_config_relative_path(config_dir: &Path, configured_path: &str) -> String {
    let path = Path::new(configured_path);
    if path.is_absolute() {
        configured_path.to_string()
    } else {
        config_dir.join(path).to_string_lossy().into_owned()
    }
}

async fn wire_forwarding_rules(
    streamer: &mut UStreamer,
    endpoints: &HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        for route in endpoint_routes(endpoint_config)? {
            let forwarding_target = route.target;
            let left_endpoint = endpoints.get(&endpoint_config.endpoint).ok_or_else(|| {
                invalid_config(format!(
                    "Unknown endpoint in forwarding rules: {}",
                    endpoint_config.endpoint
                ))
            })?;
            let right_endpoint = endpoints.get(forwarding_target).ok_or_else(|| {
                invalid_config(format!(
                    "Unknown forwarding target endpoint: {forwarding_target}"
                ))
            })?;

            match (left_endpoint.routing_mode, right_endpoint.routing_mode) {
                (RoutingMode::Owned, RoutingMode::Owned) => {
                    streamer
                        .add_route_ref(
                            standard_endpoint(left_endpoint, &endpoint_config.endpoint)?,
                            standard_endpoint(right_endpoint, forwarding_target)?,
                        )
                        .await?;
                }
                (RoutingMode::OwnedFrame, RoutingMode::OwnedFrame) => {
                    wire_owned_frame_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        route.wire_format,
                    )
                    .await?;
                }
                (RoutingMode::CopyMinimized, RoutingMode::CopyMinimized) => {
                    wire_copy_minimized_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        endpoint_config.copy_minimized_payload_alignment,
                        route.wire_format,
                    )
                    .await?;
                }
                (RoutingMode::OwnedFrame, RoutingMode::CopyMinimized) => {
                    wire_owned_to_copy_minimized_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        right_endpoint.copy_minimized_payload_alignment,
                        route.wire_format,
                    )
                    .await?;
                }
                (RoutingMode::CopyMinimized, RoutingMode::OwnedFrame) => {
                    wire_copy_minimized_to_owned_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        route.wire_format,
                    )
                    .await?;
                }
                // R3A: classic <-> owned-frame pairs route through the
                // classic bridge adapters. The wire format applies to the
                // selected leg only; the classic leg is classically framed.
                (RoutingMode::Owned, RoutingMode::OwnedFrame) => {
                    wire_classic_to_owned_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        route.wire_format,
                    )
                    .await?;
                }
                (RoutingMode::OwnedFrame, RoutingMode::Owned) => {
                    wire_owned_to_classic_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        route.wire_format,
                    )
                    .await?;
                }
                (RoutingMode::Owned, RoutingMode::CopyMinimized) => {
                    wire_classic_to_copy_minimized_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        right_endpoint.copy_minimized_payload_alignment,
                        route.wire_format,
                    )
                    .await?;
                }
                (RoutingMode::CopyMinimized, RoutingMode::Owned) => {
                    wire_copy_minimized_to_classic_route(
                        streamer,
                        left_endpoint,
                        right_endpoint,
                        &endpoint_config.endpoint,
                        forwarding_target,
                        route.wire_format,
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(feature = "owned-frame-transport")]
async fn wire_owned_frame_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_owned_route_wire_format(
        streamer,
        owned_frame_endpoint(left_endpoint, left_name, route_wire_format)?,
        owned_frame_endpoint(right_endpoint, right_name, route_wire_format)?,
        route_wire_format,
    )
    .await
}

#[cfg(feature = "owned-frame-transport")]
async fn wire_classic_to_owned_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_classic_to_owned_route_wire_format(
        streamer,
        standard_endpoint(left_endpoint, left_name)?,
        owned_frame_endpoint(right_endpoint, right_name, route_wire_format)?,
        route_wire_format,
    )
    .await
}

#[cfg(feature = "owned-frame-transport")]
async fn wire_owned_to_classic_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_owned_to_classic_route_wire_format(
        streamer,
        owned_frame_endpoint(left_endpoint, left_name, route_wire_format)?,
        standard_endpoint(right_endpoint, right_name)?,
        route_wire_format,
    )
    .await
}

#[cfg(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
async fn wire_classic_to_copy_minimized_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    payload_alignment: Option<usize>,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let options = CopyMinimizedRouteOptions {
        payload_alignment: payload_alignment.unwrap_or(1),
    };
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    let right_route_endpoint = route_wire_endpoint(right_endpoint, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_classic_to_copy_minimized_route_wire_format(
        streamer,
        standard_endpoint(left_endpoint, left_name)?,
        right_route_endpoint,
        route_wire_format,
        options,
    )
    .await
}

#[cfg(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
async fn wire_copy_minimized_to_classic_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    let left_route_endpoint = route_wire_endpoint(left_endpoint, left_name, route_wire_format)?;
    configurable_streamer_wire_support::add_copy_minimized_to_classic_route_wire_format(
        streamer,
        left_route_endpoint,
        standard_endpoint(right_endpoint, right_name)?,
        route_wire_format,
    )
    .await
}

#[cfg(all(
    feature = "owned-frame-transport",
    not(feature = "experimental-copy-minimized-routing")
))]
async fn wire_classic_to_copy_minimized_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _payload_alignment: Option<usize>,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "classic to copy-minimized route {left_name} -> {right_name} requires the experimental-copy-minimized-routing feature"
    )))
}

#[cfg(all(
    feature = "owned-frame-transport",
    not(feature = "experimental-copy-minimized-routing")
))]
async fn wire_copy_minimized_to_classic_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "copy-minimized to classic route {left_name} -> {right_name} requires the experimental-copy-minimized-routing feature"
    )))
}

#[cfg(not(feature = "owned-frame-transport"))]
async fn wire_classic_to_copy_minimized_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _payload_alignment: Option<usize>,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "classic to copy-minimized route {left_name} -> {right_name} requires the owned-frame-transport feature"
    )))
}

#[cfg(not(feature = "owned-frame-transport"))]
async fn wire_copy_minimized_to_classic_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "copy-minimized to classic route {left_name} -> {right_name} requires the owned-frame-transport feature"
    )))
}

#[cfg(not(feature = "owned-frame-transport"))]
async fn wire_classic_to_owned_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "classic to owned-frame route {left_name} -> {right_name} requires the owned-frame-transport feature"
    )))
}

#[cfg(not(feature = "owned-frame-transport"))]
async fn wire_owned_to_classic_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "owned-frame to classic route {left_name} -> {right_name} requires the owned-frame-transport feature"
    )))
}

#[cfg(not(feature = "owned-frame-transport"))]
async fn wire_owned_frame_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "owned_frame route {left_name}->{right_name} requires configurable-streamer feature owned-frame-transport"
    )))
}

#[cfg(feature = "experimental-copy-minimized-routing")]
async fn wire_copy_minimized_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    payload_alignment: Option<usize>,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let options = CopyMinimizedRouteOptions {
        payload_alignment: payload_alignment.unwrap_or(1),
    };
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_route_wire_format(
        streamer,
        route_wire_endpoint(left_endpoint, left_name, route_wire_format)?,
        route_wire_endpoint(right_endpoint, right_name, route_wire_format)?,
        route_wire_format,
        options,
    )
    .await
}

#[cfg(not(feature = "experimental-copy-minimized-routing"))]
async fn wire_copy_minimized_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _payload_alignment: Option<usize>,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "copy_minimized route {left_name}->{right_name} requires configurable-streamer feature experimental-copy-minimized-routing"
    )))
}

#[cfg(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
async fn wire_owned_to_copy_minimized_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    payload_alignment: Option<usize>,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let options = CopyMinimizedRouteOptions {
        payload_alignment: payload_alignment.unwrap_or(1),
    };
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_owned_to_copy_minimized_route_wire_format(
        streamer,
        owned_frame_endpoint(left_endpoint, left_name, route_wire_format)?,
        route_wire_endpoint(right_endpoint, right_name, route_wire_format)?,
        route_wire_format,
        options,
    )
    .await
}

#[cfg(not(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
)))]
async fn wire_owned_to_copy_minimized_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _payload_alignment: Option<usize>,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "owned_frame to copy_minimized route {left_name}->{right_name} requires configurable-streamer features owned-frame-transport and experimental-copy-minimized-routing"
    )))
}

#[cfg(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
async fn wire_copy_minimized_to_owned_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    let route_wire_format = required_route_wire_format(left_name, right_name, route_wire_format)?;
    configurable_streamer_wire_support::add_copy_minimized_to_owned_route_wire_format(
        streamer,
        route_wire_endpoint(left_endpoint, left_name, route_wire_format)?,
        owned_frame_endpoint(right_endpoint, right_name, route_wire_format)?,
        route_wire_format,
    )
    .await
}

#[cfg(not(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
)))]
async fn wire_copy_minimized_to_owned_route(
    _streamer: &mut UStreamer,
    _left_endpoint: &ConfiguredEndpoint,
    _right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    _route_wire_format: Option<RouteWireFormat>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "copy_minimized to owned_frame route {left_name}->{right_name} requires configurable-streamer features owned-frame-transport and experimental-copy-minimized-routing"
    )))
}

async fn wait_for_shutdown_signal() -> Result<(), UStatus> {
    tokio::signal::ctrl_c().await.map_err(|error| {
        UStatus::fail_with_code(
            UCode::Internal,
            format!("Unable to wait for shutdown signal: {error:?}"),
        )
    })
}

#[cfg(all(test, feature = "experimental-copy-minimized-routing"))]
mod tests {
    use super::*;

    #[test]
    fn copy_minimized_route_requires_wire_format() {
        let result = required_route_wire_format("ingress", "egress", None);

        assert!(result.is_err());
    }

    #[test]
    fn copy_minimized_route_rejects_unsupported_wire_format() {
        let result = RouteWireFormat::parse("unsupported");

        assert!(result.is_err());
    }
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    info!("Started up-linux-streamer-configurable");

    // Get the config file.
    let args = StreamerArgs::parse();
    #[cfg(any(
        all(
            feature = "experimental-copy-minimized-routing",
            feature = "lola-transport"
        ),
        feature = "lola-owned-frame"
    ))]
    let config_dir = config_parent(&args.config);
    let mut file = File::open(args.config)
        .map_err(|e| UStatus::fail_with_code(UCode::NotFound, format!("File not found: {e:?}")))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(|e| {
        UStatus::fail_with_code(
            UCode::Internal,
            format!("Unable to read config file: {e:?}"),
        )
    })?;

    let mut config: Config = json5::from_str(&contents).map_err(|e| {
        UStatus::fail_with_code(
            UCode::Internal,
            format!("Unable to parse config file: {e:?}"),
        )
    })?;
    #[cfg(any(
        feature = "experimental-copy-minimized-routing",
        feature = "owned-frame-transport"
    ))]
    let route_wire_formats = collect_route_wire_formats(&[
        &config.transports.zenoh.endpoints,
        &config.transports.mqtt.endpoints,
        config
            .transports
            .iceoryx2
            .as_ref()
            .map(|transport| transport.endpoints.as_slice())
            .unwrap_or(&[]),
        config
            .transports
            .lola
            .as_ref()
            .map(|transport| transport.endpoints.as_slice())
            .unwrap_or(&[]),
    ])?;
    if !config.transports.mqtt.endpoints.is_empty() {
        config.transports.mqtt.load_mqtt_details().map_err(|e| {
            UStatus::fail_with_code(
                UCode::InvalidArgument,
                format!("Unable to load MQTT transport details: {e:?}"),
            )
        })?;
    }

    let usubscription: Arc<dyn USubscription> = match config.usubscription_config.mode {
        SubscriptionProviderMode::StaticFile => Arc::new(USubscriptionStaticFile::new(
            config.usubscription_config.file_path.clone(),
        )),
        SubscriptionProviderMode::LiveUsubscription => {
            return Err(UStatus::fail_with_code(
                    UCode::Unimplemented,
                    "live_usubscription mode is reserved in this phase; live runtime integration is deferred (see reports/usubscription-decoupled-pubsub-migration/05-live-integration-deferred.md)",
                ));
        }
    };

    // Start the streamer instance.
    let mut streamer = UStreamer::new(
        "up-streamer",
        config.up_streamer_config.message_queue_size,
        usubscription,
    )
    .await?;

    let mut endpoints: HashMap<String, ConfiguredEndpoint> = HashMap::new();

    let streamer_uuri = UUri::try_from_parts(
        &config.streamer_uuri.authority,
        config.streamer_uuri.ue_id,
        config.streamer_uuri.ue_version_major,
        0,
    )
    .map_err(|e| {
        UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!("Unable to form streamer UUri: {e:?}"),
        )
    })?;

    // Build regular Zenoh UTransport only for non-copy-minimized endpoints. Copy-minimized
    // Zenoh endpoints open their own wire core from the same config and cannot share a router
    // listen port with an unused regular transport session.
    let zenoh_transport = if config.transports.zenoh.endpoints.iter().any(|endpoint| {
        endpoint.routing_mode == RoutingMode::Owned && endpoint.zenoh_config_file.is_none()
    }) {
        let zenoh_config = ZenohConfig::from_file(config.transports.zenoh.config_file.clone())
            .map_err(|e| {
                UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    format!("Unable to load Zenoh config file: {e:?}"),
                )
            })?;
        Some(Arc::new(
            UPTransportZenoh::new(zenoh_config, streamer_uuri.to_string())
                .await
                .map_err(|e| {
                    UStatus::fail_with_code(
                        UCode::Internal,
                        format!("Unable to initialize Zenoh UTransport: {e:?}"),
                    )
                })?,
        ))
    } else {
        None
    };

    // R3A: build the classic vSomeIP transport when its section is present.
    #[cfg(feature = "vsomeip-transport")]
    let vsomeip_transport: Option<Arc<dyn UTransport>> = match &config.transports.vsomeip {
        None => None,
        Some(vsomeip) if vsomeip.endpoints.is_empty() => None,
        Some(vsomeip) => {
            let assumed_payload_encoding = resolve_vsomeip_payload_encoding(&vsomeip.endpoints)?;
            Some(Arc::new(
                up_transport_vsomeip::UPTransportVsomeip::new_with_config_and_transport_config(
                    streamer_uuri.clone(),
                    &vsomeip.remote_authority,
                    std::path::Path::new(&vsomeip.config_file),
                    None,
                    up_transport_vsomeip::TransportConfig::new(assumed_payload_encoding),
                )
                .map_err(|e| {
                    UStatus::fail_with_code(
                        UCode::Internal,
                        format!("Unable to initialize vSomeIP UTransport: {e:?}"),
                    )
                })?,
            ))
        }
    };
    #[cfg(not(feature = "vsomeip-transport"))]
    if config
        .transports
        .vsomeip
        .as_ref()
        .is_some_and(|vsomeip| !vsomeip.endpoints.is_empty())
    {
        return Err(invalid_config(
            "config declares vSomeIP endpoints but this build lacks the \
             vsomeip-transport feature"
                .to_string(),
        ));
    }

    // build the mqtt5 transport only when the selected config uses MQTT endpoints
    let mqtt5_transport: Option<Arc<dyn UTransport>> =
        if config.transports.mqtt.endpoints.is_empty() {
            None
        } else {
            let mqtt_details = config.transports.mqtt.mqtt_details.clone().ok_or_else(|| {
                UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    "MQTT transport details are missing after load_mqtt_details",
                )
            })?;
            let mqtt_client_options = MqttClientOptions {
                broker_uri: format!("{}:{}", mqtt_details.hostname, mqtt_details.port),
                ..Default::default()
            };
            let mqtt_transport_options = Mqtt5TransportOptions {
                mqtt_client_options,
                ..Default::default()
            };
            let mqtt5_transport = Mqtt5Transport::new(
                mqtt_transport_options,
                config.streamer_uuri.authority.clone(),
            )
            .await?;
            mqtt5_transport.connect().await?;
            Some(Arc::new(mqtt5_transport))
        };

    register_zenoh_endpoints(
        &mut endpoints,
        &config.transports.zenoh.endpoints,
        &config.transports.zenoh.config_file,
        &streamer_uuri.to_string(),
        #[cfg(any(
            feature = "experimental-copy-minimized-routing",
            feature = "owned-frame-transport"
        ))]
        &route_wire_formats,
        zenoh_transport,
    )
    .await?;
    if let Some(mqtt5_transport) = mqtt5_transport {
        register_mqtt_endpoints(
            &mut endpoints,
            &config.transports.mqtt.endpoints,
            mqtt5_transport,
        )?;
    }

    #[cfg(feature = "vsomeip-transport")]
    if let (Some(vsomeip), Some(transport)) = (&config.transports.vsomeip, &vsomeip_transport) {
        register_vsomeip_endpoints(&mut endpoints, &vsomeip.endpoints, transport.clone())?;
    }
    if let Some(iceoryx2_config) = &config.transports.iceoryx2 {
        #[cfg(any(
            all(
                feature = "experimental-copy-minimized-routing",
                feature = "iceoryx2-zero-copy"
            ),
            feature = "iceoryx2-owned-frame"
        ))]
        {
            register_iceoryx2_endpoints(
                &mut endpoints,
                &iceoryx2_config.endpoints,
                &route_wire_formats,
                Iceoryx2PubSub::new(),
            )?;
        }
        #[cfg(not(any(
            all(
                feature = "experimental-copy-minimized-routing",
                feature = "iceoryx2-zero-copy"
            ),
            feature = "iceoryx2-owned-frame"
        )))]
        {
            let _ = iceoryx2_config;
            return Err(invalid_config(
                "iceoryx2 transport config requires copy-minimized features experimental-copy-minimized-routing and iceoryx2-zero-copy or owned-frame feature iceoryx2-owned-frame",
            ));
        }
    }
    if let Some(lola_config) = &config.transports.lola {
        #[cfg(any(
            all(
                feature = "experimental-copy-minimized-routing",
                feature = "lola-transport"
            ),
            feature = "lola-owned-frame"
        ))]
        {
            validate_lola_mw_com_manifests(&lola_config.endpoints, &config_dir)?;
            register_lola_endpoints(
                &mut endpoints,
                &lola_config.endpoints,
                &route_wire_formats,
                &config_dir,
            )?;
        }
        #[cfg(not(any(
            all(
                feature = "experimental-copy-minimized-routing",
                feature = "lola-transport"
            ),
            feature = "lola-owned-frame"
        )))]
        {
            let _ = lola_config;
            return Err(invalid_config(
                "LoLa transport config requires configurable-streamer feature lola-transport or lola-owned-frame",
            ));
        }
    }

    wire_forwarding_rules(
        &mut streamer,
        &endpoints,
        &config.transports.zenoh.endpoints,
    )
    .await?;
    wire_forwarding_rules(&mut streamer, &endpoints, &config.transports.mqtt.endpoints).await?;
    if let Some(iceoryx2_config) = &config.transports.iceoryx2 {
        wire_forwarding_rules(&mut streamer, &endpoints, &iceoryx2_config.endpoints).await?;
    }
    if let Some(lola_config) = &config.transports.lola {
        wire_forwarding_rules(&mut streamer, &endpoints, &lola_config.endpoints).await?;
    }
    #[cfg(feature = "vsomeip-transport")]
    if let Some(vsomeip_config) = &config.transports.vsomeip {
        wire_forwarding_rules(&mut streamer, &endpoints, &vsomeip_config.endpoints).await?;
    }

    println!("READY streamer_initialized");
    info!("Streamer initialized; waiting for shutdown signal");
    wait_for_shutdown_signal().await?;
    info!("Shutdown signal received; exiting");

    Ok(())
}
