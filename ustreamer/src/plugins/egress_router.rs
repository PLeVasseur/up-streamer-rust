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

#![recursion_limit = "256"]

use crate::plugins::types::*;

use async_std::channel::{self, Receiver, SendError, Sender};
use async_std::sync::{Arc, Mutex};
use async_std::task;
use futures::select;
use log::{debug, error, info, trace, warn};
use lru::LruCache;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{
    Remote, UAuthority, UEntity, UMessage, UMessageType, UStatus, UUri,
};
use uprotocol_sdk::uuid::builder::UUIDv8Builder;
use uprotocol_zenoh_rust::ULinkZenoh;
use uuid::Uuid as UuidForHashing;
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_core::zlock;
use zenoh_result::{bail, ZResult};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct EgressRouter {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
// zenoh_plugin_trait::declare_plugin!(EgressRouter);

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

        // TODO: Should be some mechanism by which we have registered UAuthority with a certain TransportType
        //  If we can find it, great! Send over that
        //  If we cannot, then ideally we should call uDiscovery
        //  Perhaps for now we can stub in a way to communicate over Zenoh to the uStreamer that we should add
        //   a value to the registry of UAuthority => TransportType

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

        //
        // match UMessageType::try_from(attributes.r#type) {
        //     Ok(UMessageType::UmessageTypePublish) => {
        //         trace!("UMessageTypePublish being routed externally");
        //
        //         for transport in &transports {
        //             match transport
        //                 .up_client
        //                 .send(source.clone(), payload.clone(), attributes.clone())
        //                 .await
        //             {
        //                 // TODO: Would be good to be able to log _which_ transport is succeeding or failing
        //                 Ok(_) => {
        //                     trace!(
        //                         "Forwarding message externally succeeded on transport: {}",
        //                         &transport.tag.to_string()
        //                     );
        //                     let mut transmit_cache = local_transmit_cache.lock().unwrap();
        //                     transmit_cache.put(UuidForHashing::from(id), true);
        //                 }
        //                 Err(status) => {
        //                     error!("Forwarding message externally failed on transport: {} details: {:?}", &transport.tag.to_string(), status)
        //                 }
        //             }
        //         }
        //     }
        //     Ok(UMessageType::UmessageTypeRequest) => {
        //         trace!("UMessageTypeRequest being routed externally");
        //
        //         // TODO: Need to make the send() here on each transport
        //         //  Need to keep track of the id (which will then become the reqid on the Response we get back)
        //         //   so that we can response appropriately
        //
        //         warn!("CE Egress Queue external Request not implemented yet");
        //     }
        //     Ok(UMessageType::UmessageTypeResponse) => {
        //         trace!("UMessageTypeResponse being routed externally");
        //
        //         debug!("source: {:?}\nattributes: {:?}", &source, &attributes);
        //
        //         {
        //             let mut transmit_cache = local_transmit_cache.lock().unwrap();
        //             transmit_cache.put(UuidForHashing::from(id), true);
        //         }
        //
        //         // TODO: Should ask @Steven Hartley about this:
        //         //  I'd like to know if we should attempt to retransmit on every transport that has failed
        //         for transport in &transports {
        //             match transport
        //                 .up_client
        //                 .send(source.clone(), payload.clone(), attributes.clone())
        //                 .await
        //             {
        //                 // TODO: Would be good to be able to log _which_ transport is succeeding or failing
        //                 Ok(_) => {
        //                     trace!(
        //                         "Forwarding response externally succeeded on transport: {}",
        //                         &transport.tag.to_string()
        //                     );
        //                 }
        //                 Err(status) => {
        //                     error!("Forwarding response externally failed on transport: {} details: {:?}", &transport.tag.to_string(), status)
        //                 }
        //             }
        //         }
        //     }
        //     Err(_) => {}
        //     _ => {}
        // }
    }

    error!("Something bad happened with the sender to the Egress Queue, have to stop listening.");
}

// fn transport_listener(
//     result: Result<UMessage, UStatus>,
//     udevice_authority: UAuthority,
//     egress_sender: Sender<UMessageWithRouting>,
//     egress_receiver: Receiver<UMessageWithRouting>,
//     transports: TransportVec,
// ) {
//     match result {
//         Ok(message) => {
//             let attributes = match &message.attributes {
//                 Some(attributes) => attributes,
//                 None => {
//                     info!("CE is missing an authority. No need to be routed.");
//                     return;
//                 }
//             };
//
//             let sink = match &attributes.sink {
//                 Some(sink) => sink,
//                 None => {
//                     info!("CE has attributes, but no authority. No need to be routed.");
//                     return;
//                 }
//             };
//
//             let authority = match &sink.authority {
//                 Some(authority) => authority,
//                 None => {
//                     info!("CE has sink, but no authority. No need to be routed.");
//                     return;
//                 }
//             };
//
//             debug!(
//                 "udevice_authority: {:?} Message sink authority: {:?}",
//                 &udevice_authority, &authority
//             );
//
//             if *authority != udevice_authority {
//                 info!("CE intended to be sent to another uDevice. Sending to Egress Queue.");
//                 let egress_sender_clone = egress_sender.clone();
//                 task::spawn(async move {
//                     egress_sender_clone.send(message).await.unwrap();
//                 });
//             }
//         }
//         Err(status) => {
//             error!(
//                 "transport_listener returned UStatus: {:?} msg: {}",
//                 status.get_code(),
//                 status.message()
//             );
//         }
//     }
// }

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

    // let uuri_for_all_remote = UUri {
    //     authority: Some(UAuthority {
    //         remote: Some(Remote::Name("*".to_string())),
    //     }),
    //     entity: Some(UEntity {
    //         name: "*".to_string(),
    //         id: None,
    //         version_major: None,
    //         version_minor: None,
    //     }),
    //     resource: None,
    // };
    // task::spawn(async move {
    //     let listener_closure = move |result: Result<UMessage, UStatus>| {
    //         transport_listener(
    //             result,
    //             udevice_authority.clone(),
    //             egress_queue_sender.clone(),
    //             egress_queue_receiver.clone(),
    //             transports.clone(),
    //         );
    //     };
    //
    //     // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    //     let _registered_all_remote_listener_key = {
    //         match up_client_zenoh
    //             .register_listener(uuri_for_all_remote, Box::new(listener_closure))
    //             .await
    //         {
    //             Ok(registered_key) => registered_key,
    //             Err(status) => {
    //                 error!("Failed to register listener: {:?}", status.get_code());
    //                 return;
    //             }
    //         }
    //     };
    // });

    async_std::future::pending::<()>().await;
}
