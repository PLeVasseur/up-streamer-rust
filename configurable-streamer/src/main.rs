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

use crate::config::{Config, EndpointConfig, RoutingMode, SubscriptionProviderMode};
use clap::Parser;
use std::io::Read;
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "lola-transport"
))]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{collections::HashMap, fs::File};
use tracing::info;
use up_rust::core::usubscription::USubscription;
use up_rust::{UCode, UStatus, UTransport};
#[cfg(feature = "experimental-copy-minimized-routing")]
use up_streamer::{CopyMinimizedRouteOptions, ZeroCopyFrameEndpoint};
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
    zero_copy: Option<ZeroCopyEndpoint>,
}

#[cfg(feature = "experimental-copy-minimized-routing")]
#[derive(Clone)]
enum ZeroCopyEndpoint {
    #[cfg(feature = "zenoh-zero-copy")]
    Zenoh(ZeroCopyFrameEndpoint<UPTransportZenoh>),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2(ZeroCopyFrameEndpoint<Iceoryx2PubSub>),
    #[cfg(feature = "lola-transport")]
    Lola(ZeroCopyFrameEndpoint<UTransportLola>),
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
fn zero_copy_endpoint<'a>(
    endpoint: &'a ConfiguredEndpoint,
    endpoint_name: &str,
) -> Result<&'a ZeroCopyEndpoint, UStatus> {
    endpoint.zero_copy.as_ref().ok_or_else(|| {
        invalid_config(format!(
            "endpoint {endpoint_name} is not available for copy_minimized routing; enable the matching zero-copy transport feature"
        ))
    })
}

fn ensure_copy_minimized_endpoint(
    endpoint: &ConfiguredEndpoint,
    endpoint_name: &str,
) -> Result<(), UStatus> {
    #[cfg(feature = "experimental-copy-minimized-routing")]
    {
        if endpoint.zero_copy.is_some() {
            return Ok(());
        }
        return Err(invalid_config(format!(
            "endpoint {endpoint_name} uses copy_minimized routing but no zero-copy endpoint is compiled for its transport"
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
fn zenoh_zero_copy_endpoint(
    endpoint_config: &EndpointConfig,
    transport: Arc<UPTransportZenoh>,
) -> Option<ZeroCopyEndpoint> {
    Some(ZeroCopyEndpoint::Zenoh(ZeroCopyFrameEndpoint::new(
        &endpoint_config.endpoint,
        &endpoint_config.authority,
        transport,
    )))
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    not(feature = "zenoh-zero-copy")
))]
fn zenoh_zero_copy_endpoint(
    _endpoint_config: &EndpointConfig,
    _transport: Arc<UPTransportZenoh>,
) -> Option<ZeroCopyEndpoint> {
    None
}

fn register_zenoh_endpoints(
    endpoints: &mut HashMap<String, ConfiguredEndpoint>,
    endpoint_configs: &[EndpointConfig],
    transport: Arc<UPTransportZenoh>,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        let standard_transport: Arc<dyn UTransport> = transport.clone();
        let endpoint = ConfiguredEndpoint {
            standard: Some(Endpoint::new(
                &endpoint_config.endpoint,
                &endpoint_config.authority,
                standard_transport,
            )),
            #[cfg(feature = "experimental-copy-minimized-routing")]
            zero_copy: zenoh_zero_copy_endpoint(endpoint_config, transport.clone()),
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
                zero_copy: None,
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
    transport: Arc<Iceoryx2PubSub>,
) -> Result<(), UStatus> {
    for endpoint_config in endpoint_configs {
        let standard_transport: Arc<dyn UTransport> = transport.clone();
        let endpoint = ConfiguredEndpoint {
            standard: Some(Endpoint::new(
                &endpoint_config.endpoint,
                &endpoint_config.authority,
                standard_transport,
            )),
            zero_copy: Some(ZeroCopyEndpoint::Iceoryx2(ZeroCopyFrameEndpoint::new(
                &endpoint_config.endpoint,
                &endpoint_config.authority,
                transport.clone(),
            ))),
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
        insert_configured_endpoint(
            endpoints,
            endpoint_config,
            ConfiguredEndpoint {
                standard: None,
                zero_copy: Some(ZeroCopyEndpoint::Lola(ZeroCopyFrameEndpoint::new(
                    &endpoint_config.endpoint,
                    &endpoint_config.authority,
                    transport,
                ))),
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
        for forwarding_target in &endpoint_config.forwarding {
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
) -> Result<(), UStatus> {
    let options = CopyMinimizedRouteOptions {
        payload_alignment: payload_alignment.unwrap_or(1),
    };
    add_configured_copy_minimized_route(
        streamer,
        zero_copy_endpoint(left_endpoint, left_name)?,
        zero_copy_endpoint(right_endpoint, right_name)?,
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
) -> Result<(), UStatus> {
    Err(invalid_config(format!(
        "copy_minimized route {left_name}->{right_name} requires configurable-streamer feature experimental-copy-minimized-routing"
    )))
}

#[cfg(feature = "experimental-copy-minimized-routing")]
async fn add_configured_copy_minimized_route(
    streamer: &mut UStreamer,
    ingress: &ZeroCopyEndpoint,
    egress: &ZeroCopyEndpoint,
    options: CopyMinimizedRouteOptions,
) -> Result<(), UStatus> {
    #[allow(unreachable_patterns)]
    match (ingress, egress) {
        #[cfg(feature = "zenoh-zero-copy")]
        (ZeroCopyEndpoint::Zenoh(left), ZeroCopyEndpoint::Zenoh(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "iceoryx2-zero-copy"))]
        (ZeroCopyEndpoint::Zenoh(left), ZeroCopyEndpoint::Iceoryx2(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "lola-transport"))]
        (ZeroCopyEndpoint::Zenoh(left), ZeroCopyEndpoint::Lola(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "zenoh-zero-copy"))]
        (ZeroCopyEndpoint::Iceoryx2(left), ZeroCopyEndpoint::Zenoh(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (ZeroCopyEndpoint::Iceoryx2(left), ZeroCopyEndpoint::Iceoryx2(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "lola-transport"))]
        (ZeroCopyEndpoint::Iceoryx2(left), ZeroCopyEndpoint::Lola(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "zenoh-zero-copy"))]
        (ZeroCopyEndpoint::Lola(left), ZeroCopyEndpoint::Zenoh(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "iceoryx2-zero-copy"))]
        (ZeroCopyEndpoint::Lola(left), ZeroCopyEndpoint::Iceoryx2(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "lola-transport")]
        (ZeroCopyEndpoint::Lola(left), ZeroCopyEndpoint::Lola(right)) => {
            streamer
                .add_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        _ => Err(invalid_config(
            "copy_minimized route uses an unsupported zero-copy endpoint combination",
        )),
    }
}

async fn wait_for_shutdown_signal() -> Result<(), UStatus> {
    tokio::signal::ctrl_c().await.map_err(|error| {
        UStatus::fail_with_code(
            UCode::Internal,
            format!("Unable to wait for shutdown signal: {error:?}"),
        )
    })
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
    config.transports.mqtt.load_mqtt_details().map_err(|e| {
        UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!("Unable to load MQTT transport details: {e:?}"),
        )
    })?;

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

    // build the zenoh transport
    let zenoh_config =
        ZenohConfig::from_file(config.transports.zenoh.config_file).map_err(|e| {
            UStatus::fail_with_code(
                UCode::InvalidArgument,
                format!("Unable to load Zenoh config file: {e:?}"),
            )
        })?;
    let zenoh_transport = Arc::new(
        UPTransportZenoh::builder(config.streamer_uuri.authority.clone())
            .map_err(|e| {
                UStatus::fail_with_code(
                    UCode::Internal,
                    format!("Unable to create Zenoh transport builder: {e:?}"),
                )
            })?
            .with_config(zenoh_config)
            .build()
            .await
            .map_err(|e| {
                UStatus::fail_with_code(
                    UCode::Internal,
                    format!("Unable to initialize Zenoh UTransport: {e:?}"),
                )
            })?,
    );

    // build the mqtt5 transport
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
    let mqtt5_transport: Arc<dyn UTransport> = Arc::new(mqtt5_transport);

    register_zenoh_endpoints(
        &mut endpoints,
        &config.transports.zenoh.endpoints,
        zenoh_transport,
    )?;
    register_mqtt_endpoints(
        &mut endpoints,
        &config.transports.mqtt.endpoints,
        mqtt5_transport,
    )?;
    if let Some(iceoryx2_config) = &config.transports.iceoryx2 {
        #[cfg(all(
            feature = "experimental-copy-minimized-routing",
            feature = "iceoryx2-zero-copy"
        ))]
        {
            register_iceoryx2_endpoints(
                &mut endpoints,
                &iceoryx2_config.endpoints,
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
            register_lola_endpoints(&mut endpoints, &lola_config.endpoints, &config_dir)?;
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
