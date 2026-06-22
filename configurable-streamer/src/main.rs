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
#[cfg(feature = "experimental-copy-minimized-routing")]
use configurable_streamer_wire_support::{RouteWireEndpoint, RouteWireFormat};
use std::collections::HashMap;
#[cfg(feature = "experimental-copy-minimized-routing")]
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
))]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;
use up_rust::core::usubscription::USubscription;
use up_rust::{UCode, UStatus, UTransport, UUri};
#[cfg(feature = "experimental-copy-minimized-routing")]
use up_streamer::CopyMinimizedRouteOptions;
use up_streamer::{Endpoint, UStreamer};
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "iceoryx2-zero-copy"
))]
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
))]
use up_transport_lola_rust::{LolaTransportConfig, UTransportLola};
use up_transport_mqtt5::{Mqtt5Transport, Mqtt5TransportOptions, MqttClientOptions};
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "zenoh-zero-copy"
))]
use up_transport_zenoh::ZenohZeroCopyCore;
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};
use usubscription_static_file::USubscriptionStaticFile;

#[derive(Parser)]
#[command()]
struct StreamerArgs {
    #[arg(short, long, value_name = "FILE")]
    config: String,
}

#[derive(Clone)]
struct ConfiguredEndpoint {
    standard: Option<Endpoint>,
    #[cfg(feature = "experimental-copy-minimized-routing")]
    route_wire_endpoints: HashMap<RouteWireFormat, RouteWireEndpoint>,
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
    wire_format: Option<&'a str>,
}

fn endpoint_routes(endpoint_config: &EndpointConfig) -> Vec<PendingRoute<'_>> {
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
            wire_format: wire_format.as_deref(),
        });
    }
    routes
}

#[cfg(feature = "experimental-copy-minimized-routing")]
fn required_route_wire_format(
    endpoint_name: &str,
    target_name: &str,
    configured: Option<&str>,
) -> Result<RouteWireFormat, UStatus> {
    let configured = configured.ok_or_else(|| {
        invalid_config(format!(
            "copy_minimized route {endpoint_name}->{target_name} requires wire_format"
        ))
    })?;
    RouteWireFormat::parse(configured)
}

#[cfg(feature = "experimental-copy-minimized-routing")]
fn collect_route_wire_formats(
    endpoint_configs: &[&[EndpointConfig]],
) -> Result<HashMap<String, HashSet<RouteWireFormat>>, UStatus> {
    let mut route_wire_formats: HashMap<String, HashSet<RouteWireFormat>> = HashMap::new();
    for configs in endpoint_configs {
        for endpoint_config in *configs {
            if endpoint_config.routing_mode != RoutingMode::CopyMinimized {
                continue;
            }
            for route in endpoint_routes(endpoint_config) {
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

fn ensure_copy_minimized_endpoint(
    endpoint: &ConfiguredEndpoint,
    endpoint_name: &str,
) -> Result<(), UStatus> {
    #[cfg(feature = "experimental-copy-minimized-routing")]
    {
        if !endpoint.route_wire_endpoints.is_empty() {
            return Ok(());
        }
        return Err(invalid_config(format!(
            "endpoint {endpoint_name} uses copy_minimized routing but no route wire endpoint is available"
        )));
    }

    #[cfg(not(feature = "experimental-copy-minimized-routing"))]
    {
        let _ = endpoint;
        Err(invalid_config(format!(
            "endpoint {endpoint_name} uses copy_minimized routing but configurable-streamer was not built with experimental-copy-minimized-routing"
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
) -> Result<HashMap<RouteWireFormat, RouteWireEndpoint>, UStatus> {
    let mut endpoints = HashMap::new();
    if let Some(route_wire_formats) = route_wire_formats {
        for route_wire_format in route_wire_formats {
            let zenoh_config = ZenohConfig::from_file(config_file).map_err(|e| {
                UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    format!("Unable to load Zenoh config file for route wire endpoint: {e:?}"),
                )
            })?;
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
) -> Result<HashMap<RouteWireFormat, RouteWireEndpoint>, UStatus> {
    if route_wire_formats.is_some_and(|formats| !formats.is_empty()) {
        return Err(invalid_config(
            "Zenoh route wire formats require configurable-streamer feature zenoh-zero-copy",
        ));
    }
    Ok(HashMap::new())
}

async fn register_zenoh_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    config_file: &str,
    streamer_uri: &str,
    #[cfg(feature = "experimental-copy-minimized-routing")] route_wire_formats: &HashMap<
        String,
        HashSet<RouteWireFormat>,
    >,
    transport: Option<Arc<UPTransportZenoh>>,
) -> Result<(), UStatus> {
    #[cfg(not(feature = "experimental-copy-minimized-routing"))]
    {
        let _ = (config_file, streamer_uri);
    }

    for endpoint_config in endpoint_configs {
        let standard = if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            None
        } else {
            let transport = transport.as_ref().ok_or_else(|| {
                invalid_config(format!(
                    "Zenoh endpoint {} requires regular UTransport routing but no Zenoh UTransport was initialized",
                    endpoint_config.endpoint
                ))
            })?;
            let standard_transport: Arc<dyn UTransport> = transport.clone();
            Some(Endpoint::new(
                &endpoint_config.endpoint,
                &endpoint_config.authority,
                standard_transport,
            ))
        };
        #[cfg(feature = "experimental-copy-minimized-routing")]
        let endpoint_route_wire_formats = route_wire_formats.get(&endpoint_config.endpoint);
        let endpoint = ConfiguredEndpoint {
            standard,
            #[cfg(feature = "experimental-copy-minimized-routing")]
            route_wire_endpoints: zenoh_route_wire_endpoints(
                endpoint_config,
                config_file,
                streamer_uri,
                endpoint_route_wire_formats,
            )
            .await?,
        };
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            ensure_copy_minimized_endpoint(&endpoint, &endpoint_config.endpoint)?;
        }
        insert_configured_endpoint(endpoints, endpoint_config, endpoint)?;
    }

    Ok(())
}

fn register_mqtt_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    transport: Arc<dyn UTransport>,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            return Err(invalid_config(format!(
                "MQTT endpoint {} cannot use copy_minimized routing",
                endpoint_config.endpoint
            )));
        }
        insert_configured_endpoint(
            endpoints,
            endpoint_config,
            ConfiguredEndpoint {
                standard: Some(Endpoint::new(
                    &endpoint_config.endpoint,
                    &endpoint_config.authority,
                    transport.clone(),
                )),
                #[cfg(feature = "experimental-copy-minimized-routing")]
                route_wire_endpoints: HashMap::new(),
            },
        )?;
    }

    Ok(())
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "iceoryx2-zero-copy"
))]
fn register_iceoryx2_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    route_wire_formats: &HashMap<String, HashSet<RouteWireFormat>>,
    transport: Iceoryx2PubSub,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        let mut route_wire_endpoints = HashMap::new();
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
        let endpoint = ConfiguredEndpoint {
            standard: None,
            route_wire_endpoints,
        };
        if endpoint_config.routing_mode == RoutingMode::CopyMinimized {
            ensure_copy_minimized_endpoint(&endpoint, &endpoint_config.endpoint)?;
        }
        insert_configured_endpoint(endpoints, endpoint_config, endpoint)?;
    }

    Ok(())
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
))]
fn register_lola_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    route_wire_formats: &HashMap<String, HashSet<RouteWireFormat>>,
    config_dir: &Path,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        if endpoint_config.routing_mode != RoutingMode::CopyMinimized {
            return Err(invalid_config(format!(
                "LoLa endpoint {} must use copy_minimized routing",
                endpoint_config.endpoint
            )));
        }
        let transport = UTransportLola::build(lola_transport_config(endpoint_config, config_dir)?)?;
        let mut route_wire_endpoints = HashMap::new();
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
        insert_configured_endpoint(
            endpoints,
            endpoint_config,
            ConfiguredEndpoint {
                standard: None,
                route_wire_endpoints,
            },
        )?;
    }

    Ok(())
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
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
        mw_com_config_path: endpoint_config
            .lola_mw_com_config_file
            .as_deref()
            .map(|path| resolve_config_relative_path(config_dir, path)),
    })
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
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

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
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

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
))]
fn config_parent(config_file: &str) -> PathBuf {
    Path::new(config_file)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
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
        for route in endpoint_routes(endpoint_config) {
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

            match endpoint_config.routing_mode {
                RoutingMode::Owned => {
                    streamer
                        .add_route_ref(
                            standard_endpoint(left_endpoint, &endpoint_config.endpoint)?,
                            standard_endpoint(right_endpoint, forwarding_target)?,
                        )
                        .await?;
                }
                RoutingMode::CopyMinimized => {
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
            }
        }
    }

    Ok(())
}

#[cfg(feature = "experimental-copy-minimized-routing")]
async fn wire_copy_minimized_route(
    streamer: &mut UStreamer,
    left_endpoint: &ConfiguredEndpoint,
    right_endpoint: &ConfiguredEndpoint,
    left_name: &str,
    right_name: &str,
    payload_alignment: Option<usize>,
    route_wire_format: Option<&str>,
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
    _route_wire_format: Option<&str>,
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "copy_minimized route {left_name}->{right_name} requires configurable-streamer feature experimental-copy-minimized-routing"
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
        let result = required_route_wire_format("ingress", "egress", Some("unsupported"));

        assert!(result.is_err());
    }
}

#[tokio::main]
async fn main() -> Result<(), UStatus> {
    let _ = tracing_subscriber::fmt::try_init();

    info!("Started up-linux-streamer-configurable");

    // Get the config file.
    let args = StreamerArgs::parse();
    #[cfg(all(
        feature = "experimental-copy-minimized-routing",
        feature = "lola-transport"
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
    #[cfg(feature = "experimental-copy-minimized-routing")]
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
    let zenoh_transport = if config
        .transports
        .zenoh
        .endpoints
        .iter()
        .any(|endpoint| endpoint.routing_mode != RoutingMode::CopyMinimized)
    {
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
        #[cfg(feature = "experimental-copy-minimized-routing")]
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
    if let Some(iceoryx2_config) = &config.transports.iceoryx2 {
        #[cfg(all(
            feature = "experimental-copy-minimized-routing",
            feature = "iceoryx2-zero-copy"
        ))]
        {
            register_iceoryx2_endpoints(
                &mut endpoints,
                &iceoryx2_config.endpoints,
                &route_wire_formats,
                Iceoryx2PubSub::new(),
            )?;
        }
        #[cfg(not(all(
            feature = "experimental-copy-minimized-routing",
            feature = "iceoryx2-zero-copy"
        )))]
        {
            let _ = iceoryx2_config;
            return Err(invalid_config(
                "iceoryx2 transport config requires configurable-streamer features experimental-copy-minimized-routing and iceoryx2-zero-copy",
            ));
        }
    }
    if let Some(lola_config) = &config.transports.lola {
        #[cfg(all(
            feature = "experimental-copy-minimized-routing",
            feature = "lola-transport"
        ))]
        {
            register_lola_endpoints(
                &mut endpoints,
                &lola_config.endpoints,
                &route_wire_formats,
                &config_dir,
            )?;
        }
        #[cfg(not(all(
            feature = "experimental-copy-minimized-routing",
            feature = "lola-transport"
        )))]
        {
            let _ = lola_config;
            return Err(invalid_config(
                "LoLa transport config requires configurable-streamer features experimental-copy-minimized-routing and lola-transport",
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

    println!("READY streamer_initialized");
    info!("Streamer initialized; waiting for shutdown signal");
    wait_for_shutdown_signal().await?;
    info!("Shutdown signal received; exiting");

    Ok(())
}
