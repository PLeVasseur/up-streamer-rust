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
use std::cell::RefCell;

use async_std::channel::{self, Receiver, Sender};
use async_std::sync::{Arc, Mutex};
use async_std::task;
use futures::select;
use log::{debug, error, info, trace, warn};
use lru::LruCache;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use uprotocol_sdk::rpc::{RpcClient, RpcServer};
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{
    Remote, UAuthority, UEntity, UMessage, UMessageType, UStatus, UUri,
};
use uprotocol_sdk::uri::validator::ValidationError;
use uprotocol_sdk::uuid::builder::UUIDv8Builder;
use uprotocol_zenoh_rust::ULinkZenoh;
use uuid::Uuid as UuidForHashing;
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_core::zlock;
use zenoh_result::{bail, Error, ZResult};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct UpClientPlugin;

pub trait UpClientFull: UTransport + RpcServer + RpcClient {}

impl<T> UpClientFull for T where T: UTransport + RpcServer + RpcClient {}

pub struct UpClientPluginStartArgs {
    pub host_transport: TransportType,
    pub transport_type: TransportType,
    pub up_client: RefCell<Option<Box<dyn UpClientFull>>>,
    pub runtime: Runtime,
    pub udevice_authority: UAuthority,
    pub egress_queue_sender: Sender<UMessageWithRouting>,
    pub ingress_queue_sender: Sender<UMessageWithRouting>,
    pub transmit_request_queue_receiver: Receiver<UMessageWithRouting>,
    pub transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
// zenoh_plugin_trait::declare_plugin!(EgressRouter);

impl Plugin for UpClientPlugin {
    type StartArgs = UpClientPluginStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "up_client_plugin";

    // The first operation called by zenohd on the plugin

    // TODO: Bit of a problem. If UTransport, RpcClient, RpcServer are not Send + Sync, then I cannot
    //  clone them to get them "into" the main part of the plugin, the run function
    // Perhaps I should break with the idea of using Zenoh plugin? Hmmm...
    fn start(name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        let Some(up_client) = start_args.up_client.borrow_mut().take() else {
            error!("No uP Client provided");
            return Err(Error::try_from(ValidationError::new("No uP Client provided")).unwrap());
        };

        let host_transport_clone = start_args.host_transport.clone();
        let transport_type_clone = start_args.transport_type.clone();
        let runtime_clone = start_args.runtime.clone();
        let udevice_authority_clone = start_args.udevice_authority.clone();
        let egress_queue_sender_clone = start_args.egress_queue_sender.clone();
        let ingress_queue_sender_clone = start_args.ingress_queue_sender.clone();
        let transmit_request_queue_receiver_clone =
            start_args.transmit_request_queue_receiver.clone();
        let transmit_cache_clone = start_args.transmit_cache.clone();
        async_std::task::spawn(run(
            host_transport_clone,
            transport_type_clone,
            runtime_clone,
            udevice_authority_clone,
            egress_queue_sender_clone,
            ingress_queue_sender_clone,
            transmit_request_queue_receiver_clone,
            transmit_cache_clone,
            up_client,
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

async fn transmit_queue_request_consumer(
    transport_type: TransportType,
    mut receiver: Receiver<UMessageWithRouting>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
    up_client: Box<dyn UpClientFull>,
) {
    let local_transmit_cache = transmit_cache.clone();

    trace!("Entered Transmit Request Queue Consumer");

    while let Ok(message) = receiver.recv().await {
        trace!(
            "Transmit Request Queue: {:?}: Received msg: {:?}",
            &transport_type,
            &message
        );

        let msg = message.msg;

        let source = match &msg.source {
            None => {
                error!("CE pulled from Ingress Queue has no source UUri");
                return;
            }
            Some(source) => source,
        };

        let payload = match &msg.payload {
            None => {
                error!("CE pulled from Ingress Queue has no source UUri");
                return;
            }
            Some(payload) => payload,
        };

        let attributes = match &msg.attributes {
            None => {
                error!("CE pulled from Ingress Queue has no UAttributes");
                return;
            }
            Some(attributes) => attributes.clone(),
        };

        trace!("Passed initial sanity checks of source, payload, attributes existing");

        match UMessageType::try_from(attributes.r#type) {
            Ok(UMessageType::UmessageTypePublish) => {
                trace!("UMessageTypePublish being routed internally");

                match up_client
                    .send(source.clone(), payload.clone(), attributes.clone())
                    .await
                {
                    Ok(_) => {
                        trace!("Forwarding message over {:?} successfully", &transport_type);
                    }
                    Err(status) => {
                        error!(
                            "Forwarding message internally over {:?} failed: {:?}",
                            &transport_type, status
                        )
                    }
                }
            }
            // Ok(UMessageType::UmessageTypeRequest) => {
            //     trace!("UMessageTypeRequest being routed internally");
            //
            //     let response_source = match attributes.sink {
            //         None => {
            //             error!("CE with UmessageTypeRequest heard by our uDevice doesn't have sink. Should be caught in placement into Ingress Queue");
            //             return;
            //         }
            //         Some(ref response_source) => response_source.clone(),
            //     };
            //
            //     debug!("response_source: {:?}", &response_source);
            //
            //     let response_reqid = match attributes.id {
            //         None => {
            //             error!("CE with UmessageTypeRequest heard by our uDevice doesn't have id. Should be caught in placement into Ingress Queue");
            //             return;
            //         }
            //         Some(ref response_reqid) => response_reqid.clone(),
            //     };
            //
            //     debug!("response_reqid: {:?}", &response_reqid);
            //
            //     // TODO: Note that since we only get a UPayload back from calling invoke_method
            //     //  we do not have a corresponding `id` for the response
            //     //  Unclear if that's a huge deal. For now can simply stamp with the id from uStreamer
            //     let id = uuid_builder.build();
            //
            //     debug!("id for response: {:?}", &id);
            //
            //     // TODO: BLOCKER: Currently we don't have UAttributes being returned from invoke_method(), so we fill in what
            //     //  we can. This is a problem I noted to @Steven Hartley and will likely drive some change to get
            //     //  attributes back from invoke_method as well
            //     let response_attributes = UAttributes {
            //         id: Some(id),
            //         r#type: i32::from(UMessageType::UmessageTypeResponse),
            //         sink: Some(source.clone()),
            //         priority: i32::from(UpriorityCs4), // TODO: What should the priority be?
            //         ttl: None,                         // TODO: What should the ttl be?
            //         permission_level: None,
            //         commstatus: None,
            //         reqid: Some(response_reqid.clone()),
            //         token: None,
            //     };
            //
            //     debug!("response_attributes: {:?}", &response_attributes);
            //
            //     let up_client_zenoh_clone = up_client_zenoh.clone();
            //     let source_clone = source.clone();
            //     let payload_clone = payload.clone();
            //     let attributes_clone = attributes.clone();
            //     let egress_queue_sender_clone = egress_queue_sender.clone();
            //     let response_attributes_clone = response_attributes.clone();
            //     task::spawn(async move {
            //         trace!("Inside of async closure to call invoke_method");
            //
            //         match up_client_zenoh_clone
            //             // Note that in order to be "seen" by the RpcServer::register_rpc_listener() on our device
            //             // we need to use the sink we were given as the topic
            //             .invoke_method(
            //                 response_source.clone(),
            //                 payload_clone.clone(),
            //                 attributes_clone.clone(),
            //             )
            //             .await
            //         {
            //             Ok(payload) => {
            //                 trace!("Received result back from RpcServer");
            //
            //                 let response_msg = UMessage {
            //                     source: Some(response_source.clone()),
            //                     attributes: Some(response_attributes_clone),
            //                     payload: Some(payload),
            //                 };
            //
            //                 egress_queue_sender_clone.send(response_msg).await.unwrap();
            //
            //                 trace!("Sent response_msg to Egress Queue");
            //             }
            //             Err(e) => {
            //                 println!("invoke_method failed: {:?}", e)
            //             }
            //         }
            //
            //         trace!("After invoke_method");
            //     });
            //
            //     // warn!("CE Ingress Queue -> uDevice internal Request not implemented yet");
            //     // return;
            // }
            // Ok(UMessageType::UmessageTypeResponse) => {
            //     trace!("UMessageTypeResponse being routed internally");
            //
            //     // TODO: if Response, then...
            //     //  => Need to consider how to get ahold of our raw Zenoh session
            //     //     so that we can look up the Zenoh Query to reply back on
            //
            //     warn!("CE Ingress Queue -> uDevice internal Request not implemented yet");
            //     return;
            // }
            Err(_) => {}
            _ => {}
        }

        // TODO: How do we get access to the UpClientPlugin? Just take ownership?

        // TODO: Unpack the UAttributes and then match over the UMessageType
        //  match &type {
        //    UmessageTypePublish => {}
        //    UmessageTypeResponse => {}
        //    UmessageTypeRequest => {}
        //  }
    }

    error!("Erroneously exited Transmit Request Queue Consumer");
}

fn register_all_remote_listener(
    result: Result<UMessage, UStatus>,
    transport_type: TransportType,
    udevice_authority: UAuthority,
    egress_sender: Sender<UMessageWithRouting>,
    ingress_sender: Sender<UMessageWithRouting>,
) {
    trace!("register_all_remote_listener");

    match result {
        Ok(message) => {
            let attributes = match &message.attributes {
                Some(attributes) => attributes,
                None => {
                    info!("CE is missing an authority. No need to be routed.");
                    return;
                }
            };

            let sink = match &attributes.sink {
                Some(sink) => sink,
                None => {
                    info!("CE has attributes, but no authority. No need to be routed.");
                    return;
                }
            };

            let authority = match &sink.authority {
                Some(authority) => authority,
                None => {
                    info!("CE has sink, but no authority. No need to be routed.");
                    return;
                }
            };

            debug!(
                "udevice_authority: {:?} Message sink authority: {:?}",
                &udevice_authority, &authority
            );

            if *authority != udevice_authority {
                info!("CE intended to be sent to another uDevice. Sending to Egress Queue.");
                let egress_sender_clone = egress_sender.clone();
                task::spawn(async move {
                    let routing_message = UMessageWithRouting {
                        msg: message,
                        src: transport_type,
                        dst: TransportType::Multicast,
                    };
                    egress_sender_clone.send(routing_message).await.unwrap();
                });
            } else {
                info!("CE intended for this uDevice. Sending to Ingress Queue.");
                let ingress_sender_clone = ingress_sender.clone();
                task::spawn(async move {
                    let routing_message = UMessageWithRouting {
                        msg: message,
                        src: transport_type,
                        dst: TransportType::Multicast,
                    };
                    ingress_sender_clone.send(routing_message).await.unwrap();
                });
            }
        }
        Err(status) => {
            error!(
                "transport_listener returned UStatus: {:?} msg: {}",
                status.get_code(),
                status.message()
            );
        }
    }
}

async fn run(
    host_transport: TransportType,
    transport_type: TransportType,
    runtime: Runtime,
    udevice_authority: UAuthority,
    egress_queue_sender: Sender<UMessageWithRouting>,
    ingress_queue_sender: Sender<UMessageWithRouting>,
    transmit_request_queue_receiver: Receiver<UMessageWithRouting>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
    up_client: Box<dyn UpClientFull>,
) {
    let _ = env_logger::try_init();

    let uuri_for_all_remote = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Name("*".to_string())),
        }),
        entity: Some(UEntity {
            name: "*".to_string(),
            id: None,
            version_major: None,
            version_minor: None,
        }),
        resource: None,
    };
    // task::spawn(async move {
    let transport_type_clone = transport_type.clone();
    let listener_closure = move |result: Result<UMessage, UStatus>| {
        let transport_type_clone_clone = transport_type_clone.clone();
        register_all_remote_listener(
            result,
            transport_type_clone_clone,
            udevice_authority.clone(),
            egress_queue_sender.clone(),
            ingress_queue_sender.clone(),
        );
    };

    trace!("up_client_full: {:?}", &transport_type);

    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let _registered_all_remote_listener_key = {
        match up_client
            .register_listener(
                uuri_for_all_remote.clone(),
                Box::new(listener_closure.clone()),
            )
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                error!("Failed to register listener: {:?}", status.get_code());
                return;
            }
        }
    };

    trace!("register_listener");

    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let _registered_all_remote_listener_key = {
        match up_client
            .register_rpc_listener(
                uuri_for_all_remote.clone(),
                Box::new(listener_closure.clone()),
            )
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                error!("Failed to register listener: {:?}", status.get_code());
                return;
            }
        }
    };

    trace!("register_rpc_listener");

    task::spawn(transmit_queue_request_consumer(
        transport_type.clone(),
        transmit_request_queue_receiver.clone(),
        transmit_cache.clone(),
        up_client,
    ));

    async_std::future::pending::<()>().await;
}
