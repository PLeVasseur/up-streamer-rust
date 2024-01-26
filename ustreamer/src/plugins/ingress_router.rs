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
use uprotocol_sdk::uprotocol::UAuthority;
use uuid::Uuid as UuidForHashing;
// TODO: If we want to make this a stand-alone plugin that can compile to a dylib (.so), then we need to
//  use ZenohPlugin
//  use zenoh::plugins::{ZenohPlugin};
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_result::ZResult;

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct IngressRouter {}

// TODO: If we want to make this a stand-alone plugin that can compile to a dylib (.so), then we need to
//  declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
//  zenoh_plugin_trait::declare_plugin!(EgressRouter);

pub struct IngressRouterStartArgs {
    pub host_transport: TransportType,
    pub runtime: Runtime,
    pub udevice_authority: UAuthority,
    pub ingress_queue_sender: Sender<UMessageWithRouting>,
    pub ingress_queue_receiver: Receiver<UMessageWithRouting>,
    pub egress_queue_sender: Sender<UMessageWithRouting>,
    pub transmit_request_senders: Arc<Mutex<HashMap<TransportType, Sender<UMessageWithRouting>>>>,
    pub transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
}

impl Plugin for IngressRouter {
    type StartArgs = IngressRouterStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "ingress_router";

    // The first operation called by zenohd on the plugin
    // TODO: Think about how we can use _name
    //  My first thought is to use this as a prepend, e.g. up-admin/provided_name/foo
    //  To allow us to control configuration at run-time
    fn start(_name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        trace!("entered up_client_full: start");
        let host_transport_clone = start_args.host_transport.clone();
        let udevice_authority = start_args.udevice_authority.clone();
        let ingress_queue_sender_clone = start_args.ingress_queue_sender.clone();
        let ingress_queue_receiver_clone = start_args.ingress_queue_receiver.clone();
        let egress_queue_sender_clone = start_args.egress_queue_sender.clone();
        let transmit_request_senders_clone = start_args.transmit_request_senders.clone();
        let transmit_cache_clone = start_args.transmit_cache.clone();
        async_std::task::spawn(run(
            host_transport_clone,
            udevice_authority,
            ingress_queue_sender_clone,
            ingress_queue_receiver_clone,
            egress_queue_sender_clone,
            transmit_request_senders_clone,
            transmit_cache_clone,
        ));

        // let ingress_queue_sender_plugin_clone = start_args.ingress_queue_sender.clone();
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                _runtime: start_args.runtime.clone(),
                _udevice_authority: start_args.udevice_authority.clone(),
            },
        )))))
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    // TODO: Eventually, we can use this to receive admin messages addressed to us for live configuration
    //  over up-admin/provided_name/foo
    _runtime: Runtime,
    // TODO: Unclear if udevice_authority needed
    _udevice_authority: UAuthority,
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

async fn ingress_queue_consumer(
    host_transport: TransportType,
    transport_request_senders: Arc<Mutex<HashMap<TransportType, Sender<UMessageWithRouting>>>>,
    // TODO: Unclear if egress_queue_sender needed here
    _egress_queue_sender: Sender<UMessageWithRouting>,
    ingress_queue_receiver: Receiver<UMessageWithRouting>,
) {
    debug!("host_transport: {:?}", host_transport);

    while let Ok(msg) = ingress_queue_receiver.recv().await {
        trace!("Ingress Queue: Received msg: {:?}", msg);

        let send_message = |msg: UMessageWithRouting| async {
            // TODO: Generally remove all the .unwrap() and handle errors properly

            if let Some(sender) = transport_request_senders.lock().await.get(&host_transport) {
                match sender.send(msg).await {
                    Ok(_) => info!("Sent message to host transmit queue."),
                    Err(_) => error!("Unable to route message to host transmit queue."),
                }
            } else {
                error!(
                    "No sender matching the host transport is registered. host_transport: {:?}",
                    &host_transport
                );
            }
        };

        match &msg.src {
            TransportType::Multicast => {
                error!("Not possible!");
                continue;
            }
            TransportType::UpClientZenoh => {
                if host_transport == TransportType::UpClientZenoh {
                    info!("Source is Zenoh and so is the host, should be routed automatically no need for bridging.");
                    continue;
                }
                send_message(msg).await;
            }
            _ => send_message(msg).await,
        }
    }
    error!("Something bad happened with the sender to the Ingress Queue, have to stop listening.");
}

async fn run(
    host_transport: TransportType,
    // TODO: Unclear if udevice_authority needed
    _udevice_authority: UAuthority,
    // TODO: Unclear if ingress_queue_sender needed
    _ingress_queue_sender: Sender<UMessageWithRouting>,
    ingress_queue_receiver: Receiver<UMessageWithRouting>,
    egress_queue_sender: Sender<UMessageWithRouting>,
    transmit_request_senders: Arc<Mutex<HashMap<TransportType, Sender<UMessageWithRouting>>>>,
    // TODO: Unclear if transmit_cache needed
    _transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
) {
    let _ = env_logger::try_init();

    debug!("host_transport: {:?}", &host_transport);

    let egress_queue_sender_clone = egress_queue_sender.clone();
    let host_transport_clone = host_transport.clone();
    let transmit_request_senders_clone = transmit_request_senders.clone();
    task::spawn(ingress_queue_consumer(
        host_transport_clone,
        transmit_request_senders_clone,
        egress_queue_sender_clone,
        ingress_queue_receiver,
    ));

    async_std::future::pending::<()>().await;
}
