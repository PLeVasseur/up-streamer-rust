/********************************************************************************
 * Copyright (c) 2023 Contributors to the Eclipse Foundation
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

// TODO: Would be used if we made this compile to its own dylib as a stand-alone Zenoh Plugin
//  #![recursion_limit = "256"]

use crate::plugins::types::*;

use async_std::channel::{Receiver, Sender};
use async_std::sync::{Arc, Mutex};
use async_std::task;
use log::*;
use lru::LruCache;
use std::collections::HashMap;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::UAuthority;
use uprotocol_sdk::uuid::builder::UUIDv8Builder;
use uuid::Uuid as UuidForHashing;
// TODO: If we want to make this a stand-alone plugin that can compile to a dylib (.so), then we need to
//  use ZenohPlugin
//  use zenoh::plugins::{ZenohPlugin};
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_result::ZResult;

pub struct EgressRouter {}

// TODO: If we want to make this a stand-alone plugin that can compile to a dylib (.so), then we need to
//  declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
//  zenoh_plugin_trait::declare_plugin!(EgressRouter);

pub struct EgressRouterStartArgs {
    pub host_transport: TransportType,
    pub authority_transport_mapping: Arc<Mutex<HashMap<HashableAuthority, TransportType>>>,
    pub uuid_builder: Arc<UUIDv8Builder>,
    pub runtime: Runtime,
    pub udevice_authority: UAuthority,
    pub egress_queue_sender: Sender<UMessageWithRouting>,
    pub egress_queue_receiver: Receiver<UMessageWithRouting>,
    pub transmit_request_senders: Arc<Mutex<HashMap<TransportType, Sender<UMessageWithRouting>>>>,
    pub transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
}

impl Plugin for EgressRouter {
    type StartArgs = EgressRouterStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "egress_router";

    // The first operation called by zenohd on the plugin
    fn start(name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        let runtime_clone = start_args.runtime.clone();
        let udevice_authority_clone = start_args.udevice_authority.clone();
        let authority_transport_mapping_clone = start_args.authority_transport_mapping.clone();
        let egress_queue_sender_clone = start_args.egress_queue_sender.clone();
        let egress_queue_receiver_clone = start_args.egress_queue_receiver.clone();
        let transmit_request_senders_clone = start_args.transmit_request_senders.clone();
        let transmit_cache_clone = start_args.transmit_cache.clone();
        async_std::task::spawn(run(
            runtime_clone,
            udevice_authority_clone,
            authority_transport_mapping_clone,
            egress_queue_sender_clone,
            egress_queue_receiver_clone,
            transmit_request_senders_clone,
            transmit_cache_clone,
        ));

        // let ingress_queue_sender_plugin_clone = start_args.ingress_queue_sender.clone();
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                runtime: start_args.runtime.clone(),
            },
        )))))
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    runtime: Runtime,
}
// The RunningPlugin struct implementing the RunningPluginTrait trait
#[derive(Clone)]
struct RunningPlugin(Arc<Mutex<RunningPluginInner>>);
impl RunningPluginTrait for RunningPlugin {
    // Operation returning a ValidationFunction(path, old, new)-> ZResult<Option<serde_json::Map<String, serde_json::Value>>>
    // this function will be called each time the plugin's config is changed via the zenohd admin space
    fn config_checker(&self) -> ValidationFunction {
        todo!()
    }

    // Function called on any query on admin space that matches this plugin's sub-part of the admin space.
    // Thus the plugin can reply its contribution to the global admin space of this zenohd.
    fn adminspace_getter<'a>(
        &'a self,
        _selector: &'a Selector<'a>,
        _plugin_status_key: &str,
    ) -> ZResult<Vec<zenoh::plugins::Response>> {
        todo!()
    }
}

async fn egress_queue_consumer(
    mut receiver: Receiver<UMessageWithRouting>,
    authority_transport_mapping: Arc<Mutex<HashMap<HashableAuthority, TransportType>>>,
    transmit_request_senders: Arc<Mutex<HashMap<TransportType, Sender<UMessageWithRouting>>>>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
) {
    let local_transmit_cache = transmit_cache.clone();
    let local_authority_transport_mapping = authority_transport_mapping.clone();
    while let Ok(message) = receiver.recv().await {
        trace!("Egress Queue: Received msg: {:?}", message);

        let msg = &message.msg;

        let source = match &msg.source {
            None => {
                error!("CE pulled from Egress Queue has no source UUri");
                continue;
            }
            Some(source) => source,
        };

        let payload = match &msg.payload {
            None => {
                error!("CE pulled from Egress Queue has no source UUri");
                continue;
            }
            Some(payload) => payload,
        };

        let attributes = match &msg.attributes {
            None => {
                error!("CE pulled from Egress Queue has no UAttributes");
                continue;
            }
            Some(attributes) => attributes,
        };

        let id = match &attributes.id {
            None => {
                error!("CE pulled from Egress Queue does not have an id (UUID)");
                continue;
            }
            Some(id) => id,
        };

        let sink = match &attributes.sink {
            None => {
                error!("CE pulled from Egress Queue does not have a sink UUri");
                continue;
            }
            Some(sink) => sink,
        };

        let authority = match &sink.authority {
            None => {
                error!("CE pulled from Egress Queue has a sink UUri without a UAuthority");
                continue;
            }
            Some(authority) => authority,
        };

        match local_authority_transport_mapping
            .lock()
            .await
            .get(&HashableAuthority(authority.clone()))
        {
            None => {
                error!("CE pulled from Egress Queue has a sink UUri whose UAuthority does not match any known mapping to a transport. UAuthority: {:?}", authority);
                continue;
            }
            Some(transport) => match transmit_request_senders.lock().await.get(transport) {
                None => {
                    error!("Desired transport to transmit over: {:?} does not have a registered UpClient.", &transport);
                    continue;
                }
                Some(transport_request_sender) => {
                    match transport_request_sender.send(message).await {
                        Ok(_) => {
                            info!(
                                "Forwarded message to Transmit Request Queue for: {:?}",
                                &transport
                            );
                        }
                        Err(e) => {
                            error!("Unable to forward message to Transmit Request Queue for: {:?} with error: {:?}", &transport, e);
                            continue;
                        }
                    }
                }
            },
        }
    }
    error!("Something bad happened with the sender to the Egress Queue, have to stop listening.");
}

async fn run(
    runtime: Runtime,
    udevice_authority: UAuthority,
    authority_transport_mapping: Arc<Mutex<HashMap<HashableAuthority, TransportType>>>,
    egress_queue_sender: Sender<UMessageWithRouting>,
    egress_queue_receiver: Receiver<UMessageWithRouting>,
    transmit_request_senders: Arc<Mutex<HashMap<TransportType, Sender<UMessageWithRouting>>>>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
) {
    let _ = env_logger::try_init();

    task::spawn(egress_queue_consumer(
        egress_queue_receiver.clone(),
        authority_transport_mapping.clone(),
        transmit_request_senders.clone(),
        transmit_cache.clone(),
    ));

    async_std::future::pending::<()>().await;
}
