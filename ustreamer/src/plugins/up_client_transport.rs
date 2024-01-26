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
use std::cell::RefCell;

use async_std::channel::{Receiver, Sender};
use async_std::pin::Pin;
use async_std::prelude::Future;
use async_std::sync::{Arc, Mutex};
use async_std::task;
use async_trait::async_trait;
use log::*;
use lru::LruCache;
use std::convert::TryFrom;
use uprotocol_sdk::rpc::{RpcClient, RpcServer};
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::UPriority::UpriorityCs4;
use uprotocol_sdk::uprotocol::{
    Remote, UAttributes, UAuthority, UEntity, UMessage, UMessageType, UStatus, UUri,
};
use uprotocol_sdk::uri::validator::ValidationError;
use uprotocol_sdk::uuid::builder::UUIDv8Builder;
use uuid::Uuid as UuidForHashing;
// TODO: If we want to make this a stand-alone plugin that can compile to a dylib (.so), then we need to
//  use ZenohPlugin
//  use zenoh::plugins::{ZenohPlugin};
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_result::{Error, ZResult};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct UpClientTransportPlugin;

// TODO: If we want to make this a stand-alone plugin that can compile to a dylib (.so), then we need to
//  declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
//  zenoh_plugin_trait::declare_plugin!(UpClientTransportPlugin);

pub trait UpClientTransport: UTransport + RpcServer + RpcClient {}

impl<T> UpClientTransport for T where T: UTransport + RpcServer + RpcClient {}

#[async_trait]
pub trait UpClientTransportFactory: Send + Sync {
    fn transport_type(&self) -> &'static TransportType;

    fn create_up_client(
        &self,
    ) -> Box<
        dyn FnOnce()
            -> Pin<Box<dyn Future<Output = Arc<Mutex<Box<dyn UpClientTransport>>>> + Send>>,
    >;

    async fn create_and_setup(
        &self,
        udevice_authority: UAuthority,
        egress_queue_sender: Sender<UMessageWithRouting>,
        ingress_queue_sender: Sender<UMessageWithRouting>,
        transmit_request_queue_receiver: Receiver<UMessageWithRouting>,
        transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
    ) {
        trace!("entered create_and_setup");

        let up_client = task::block_on({
            // Call the boxed function to get the future
            let up_client_future = (self.create_up_client())();
            // Await the future to get the Arc<Mutex<Box<dyn UpClientFull>>>
            up_client_future
        });

        trace!("after creating up_client");

        let transport_type_clone_1 = self.transport_type().clone();
        let transport_type_clone_2 = self.transport_type().clone();

        setup_remote_listeners(
            &up_client,
            transport_type_clone_1,
            udevice_authority,
            egress_queue_sender.clone(),
            ingress_queue_sender,
        );

        trace!("after setup_remote_listeners");

        task::spawn_local(async move {
            start_transmit_queue_receiver(
                up_client,
                transport_type_clone_2,
                transmit_request_queue_receiver,
                egress_queue_sender.clone(),
                transmit_cache,
            );

            trace!("after transmit_queue_receiver");

            async_std::future::pending::<()>().await;
        });
    }
}

fn setup_remote_listeners(
    up_client: &Arc<Mutex<Box<dyn UpClientTransport>>>,
    transport_type: TransportType,
    udevice_authority: UAuthority,
    egress_queue_sender: Sender<UMessageWithRouting>,
    ingress_queue_sender: Sender<UMessageWithRouting>,
) {
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

    let transport_type_clone = transport_type.clone();
    let listener_closure = move |result: Result<UMessage, UStatus>| {
        let transport_type_clone = transport_type_clone.clone();
        register_all_remote_listener(
            result,
            transport_type_clone,
            udevice_authority.clone(),
            egress_queue_sender.clone(),
            ingress_queue_sender.clone(),
        );
    };

    trace!("up_client_full: {:?}", &transport_type);

    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let listener_closure_register_listener = listener_closure.clone();
    let uuri_for_all_remote_register_listener = uuri_for_all_remote.clone();
    task::block_on(async move {
        let _registered_all_remote_listener_key = {
            let up_client_lock = up_client.lock().await;
            let up_client_register_listener_result = up_client_lock.register_listener(
                uuri_for_all_remote_register_listener,
                Box::new(listener_closure_register_listener),
            );

            let registration_result = up_client_register_listener_result.await;

            if registration_result.is_err() {
                error!(
                    "Failed to register listener: {:?}",
                    registration_result.unwrap_err().get_code()
                );
                return;
            }

            registration_result.unwrap()
        };
    });

    trace!("register_listener");

    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let listener_closure_register_rpc_listener = listener_closure.clone();
    let uuri_for_all_remote_register_rpc_listener = uuri_for_all_remote.clone();
    task::block_on(async move {
        let _registered_all_remote_listener_key = {
            let up_client_lock = up_client.lock().await;
            let up_client_register_rpc_listener_result = up_client_lock.register_rpc_listener(
                uuri_for_all_remote_register_rpc_listener,
                Box::new(listener_closure_register_rpc_listener),
            );

            let registration_result = up_client_register_rpc_listener_result.await;

            if registration_result.is_err() {
                error!(
                    "Failed to register listener: {:?}",
                    registration_result.unwrap_err().get_code()
                );
                return;
            }

            registration_result.unwrap()
        };
    });
}

fn start_transmit_queue_receiver(
    up_client: Arc<Mutex<Box<dyn UpClientTransport>>>,
    transport_type: TransportType,
    transmit_request_queue_receiver: Receiver<UMessageWithRouting>,
    egress_queue_sender: Sender<UMessageWithRouting>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
) {
    task::spawn_local(async move {
        transmit_queue_request_consumer(
            transport_type.clone(),
            transmit_request_queue_receiver.clone(),
            egress_queue_sender.clone(),
            transmit_cache.clone(),
            up_client,
        )
        .await;
    });
}

pub struct UpClientTransportPluginStartArgs {
    pub host_transport: TransportType,
    pub up_client_factory: RefCell<Option<Box<dyn UpClientTransportFactory>>>,
    pub runtime: Runtime,
    pub udevice_authority: UAuthority,
    pub egress_queue_sender: Sender<UMessageWithRouting>,
    pub ingress_queue_sender: Sender<UMessageWithRouting>,
    pub transmit_request_queue_receiver: Receiver<UMessageWithRouting>,
    pub transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
// zenoh_plugin_trait::declare_plugin!(EgressRouter);

impl Plugin for UpClientTransportPlugin {
    type StartArgs = UpClientTransportPluginStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "up_client_plugin";

    // The first operation called by zenohd on the plugin
    // TODO: Think about how we can use _name
    //  My first thought is to use this as a prepend, e.g. up-admin/provided_name/foo
    //  To allow us to control configuration at run-time
    fn start(_name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        trace!("entered up_client_full: start");

        let Some(up_client_factory) = start_args.up_client_factory.borrow_mut().take() else {
            error!("No uP Client Factory provided");
            return Err(Error::try_from(ValidationError::new("No uP Client provided")).unwrap());
        };

        trace!("up_client_full: start: after obtaining factory");

        let host_transport_clone = start_args.host_transport.clone();
        let udevice_authority_clone = start_args.udevice_authority.clone();
        let egress_queue_sender_clone = start_args.egress_queue_sender.clone();
        let ingress_queue_sender_clone = start_args.ingress_queue_sender.clone();
        let transmit_request_queue_receiver_clone =
            start_args.transmit_request_queue_receiver.clone();
        let transmit_cache_clone = start_args.transmit_cache.clone();
        trace!("up_client_full: start: before running run");
        async_std::task::spawn(async move {
            trace!("inside of spawn, before calling run");

            run(
                host_transport_clone,
                udevice_authority_clone,
                egress_queue_sender_clone,
                ingress_queue_sender_clone,
                transmit_request_queue_receiver_clone,
                transmit_cache_clone,
                up_client_factory,
            )
            .await;

            trace!("inside of spawn, after calling run");
        });

        // let ingress_queue_sender_plugin_clone = start_args.ingress_queue_sender.clone();
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                _runtime: start_args.runtime.clone(),
            },
        )))))
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    // TODO: Eventually, we can use this to receive admin messages addressed to us for live configuration
    //  over up-admin/provided_name/foo
    _runtime: Runtime,
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
    receiver: Receiver<UMessageWithRouting>,
    egress_queue_sender: Sender<UMessageWithRouting>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
    up_client: Arc<Mutex<Box<dyn UpClientTransport>>>,
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
                error!("CE pulled from Transmit Request Queue has no source UUri");
                continue;
            }
            Some(source) => source,
        };

        let payload = match &msg.payload {
            None => {
                error!("CE pulled from Transmit Request Queue has no source UUri");
                continue;
            }
            Some(payload) => payload,
        };

        let attributes = match &msg.attributes {
            None => {
                error!("CE pulled from Transmit Request Queue has no UAttributes");
                continue;
            }
            Some(attributes) => attributes.clone(),
        };

        let id = match &attributes.id {
            None => {
                error!("CE pulled from Transmit Request Queue hsa no id in UAttributes");
                continue;
            }
            Some(id) => id.clone(),
        };

        let uuid_for_hashing = UuidForHashing::from(id.clone());

        if local_transmit_cache
            .lock()
            .await
            .get(&uuid_for_hashing)
            .is_some()
        {
            info!("Already forwarded CE with Uuid: {}", &uuid_for_hashing);
            continue;
        }

        trace!("Passed initial sanity checks of source, payload, attributes existing");

        match UMessageType::try_from(attributes.r#type) {
            Ok(UMessageType::UmessageTypePublish) => {
                trace!("UMessageTypePublish being routed internally");

                let up_client_lock = up_client.lock().await;
                let up_client_send =
                    up_client_lock.send(source.clone(), payload.clone(), attributes.clone());

                local_transmit_cache
                    .lock()
                    .await
                    .put(uuid_for_hashing.clone(), true);

                let transmit_cache_within_send = local_transmit_cache.clone();
                match up_client_send.await {
                    Ok(_) => {
                        trace!("Forwarding message over {:?} successfully", &transport_type);
                    }
                    Err(status) => {
                        error!(
                            "Forwarding message internally over {:?} failed: {:?}",
                            &transport_type, status
                        );

                        transmit_cache_within_send
                            .lock()
                            .await
                            .pop(&uuid_for_hashing);

                        // TODO: Put message back in egress queue
                    }
                }
            }
            Ok(UMessageType::UmessageTypeRequest) => {
                trace!("UMessageTypeRequest being routed internally");

                let response_source = match attributes.sink {
                    None => {
                        error!("CE with UmessageTypeRequest heard by our uDevice doesn't have sink. Should be caught in placement into Ingress Queue");
                        return;
                    }
                    Some(ref response_source) => response_source.clone(),
                };

                debug!("response_source: {:?}", &response_source);

                let response_reqid = match attributes.id {
                    None => {
                        error!("CE with UmessageTypeRequest heard by our uDevice doesn't have id. Should be caught in placement into Ingress Queue");
                        return;
                    }
                    Some(ref response_reqid) => response_reqid.clone(),
                };

                debug!("response_reqid: {:?}", &response_reqid);

                // TODO: Note that since we only get a UPayload back from calling invoke_method
                //  we do not have a corresponding `id` for the response
                //  Unclear if that's a huge deal. For now can simply stamp with the id from uStreamer
                let id = UUIDv8Builder::new().build();

                debug!("id for response: {:?}", &id);

                // TODO: BLOCKER: Currently we don't have UAttributes being returned from invoke_method(), so we fill in what
                //  we can. This is a problem I noted to @Steven Hartley and will likely drive some change to get
                //  attributes back from invoke_method as well
                let response_attributes = UAttributes {
                    id: Some(id),
                    r#type: i32::from(UMessageType::UmessageTypeResponse),
                    sink: Some(source.clone()),
                    priority: i32::from(UpriorityCs4), // TODO: What should the priority be?
                    ttl: None,                         // TODO: What should the ttl be?
                    permission_level: None,
                    commstatus: None,
                    reqid: Some(response_reqid.clone()),
                    token: None,
                };

                debug!("response_attributes: {:?}", &response_attributes);

                let payload_clone = payload.clone();
                let attributes_clone = attributes.clone();
                let egress_queue_sender_clone = egress_queue_sender.clone();
                let response_attributes_clone = response_attributes.clone();
                trace!("before call invoke_method");

                let up_client_lock = up_client.lock().await;
                // Note that in order to be "seen" by the RpcServer::register_rpc_listener() on our device
                // we need to use the sink we were given as the topic
                let up_client_invoke_method = up_client_lock.invoke_method(
                    response_source.clone(),
                    payload_clone,
                    attributes_clone,
                );

                // TODO: We don't want to "extraneously" react upon the response coming back
                let reqid_for_hashing = UuidForHashing::from(response_reqid.clone());
                local_transmit_cache
                    .lock()
                    .await
                    .put(reqid_for_hashing.clone(), true);

                match up_client_invoke_method.await {
                    Ok(payload) => {
                        trace!("Received result back from RpcServer");

                        let response_msg = UMessage {
                            source: Some(response_source.clone()),
                            attributes: Some(response_attributes_clone),
                            payload: Some(payload),
                        };

                        let response_message = UMessageWithRouting {
                            msg: response_msg,
                            src: transport_type.clone(),
                            dst: TransportType::Multicast,
                        };

                        egress_queue_sender_clone
                            .send(response_message)
                            .await
                            .unwrap();

                        trace!("Sent response_msg to Egress Queue");
                    }
                    Err(e) => {
                        println!("invoke_method failed: {:?}", e);
                        continue;
                    }
                }

                trace!("After invoke_method");
            }

            Ok(UMessageType::UmessageTypeResponse) => {
                trace!("UMessageTypeResponse being routed externally");

                debug!("source: {:?}\nattributes: {:?}", &source, &attributes);

                let up_client_lock = up_client.lock().await;
                let up_client_send =
                    up_client_lock.send(source.clone(), payload.clone(), attributes.clone());

                local_transmit_cache
                    .lock()
                    .await
                    .put(uuid_for_hashing.clone(), true);

                let transmit_cache_within_send = local_transmit_cache.clone();
                match up_client_send.await {
                    Ok(_) => {
                        trace!("Forwarding message over {:?} successfully", &transport_type);
                    }
                    Err(status) => {
                        error!(
                            "Forwarding message over {:?} failed: {:?}",
                            &transport_type, status
                        );

                        transmit_cache_within_send
                            .lock()
                            .await
                            .pop(&uuid_for_hashing);

                        // TODO: Put message back in egress queue
                    }
                }
            }
            Err(e) => {
                error!(
                    "Error in converting UAttributes.type to a UMessageType: {:?}",
                    e
                );
                continue;
            }
            _ => {
                warn!("Unsupported UMessageType");
                continue;
            }
        }
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
    // TODO: Unclear if host_transport needed
    _host_transport: TransportType,
    udevice_authority: UAuthority,
    egress_queue_sender: Sender<UMessageWithRouting>,
    ingress_queue_sender: Sender<UMessageWithRouting>,
    transmit_request_queue_receiver: Receiver<UMessageWithRouting>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
    up_client_factory: Box<dyn UpClientTransportFactory>,
) {
    let _ = env_logger::try_init();

    trace!("before create_and_setup");

    up_client_factory
        .create_and_setup(
            udevice_authority.clone(),
            egress_queue_sender.clone(),
            ingress_queue_sender.clone(),
            transmit_request_queue_receiver.clone(),
            transmit_cache.clone(),
        )
        .await;

    trace!("after create_and_setup");

    async_std::future::pending::<()>().await;
}
