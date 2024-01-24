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
use async_std::task;
use futures::select;
use log::{debug, error, info, trace, warn};
use lru::LruCache;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, Mutex,
};
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
    pub up_client: RefCell<Option<Box<dyn UpClientFull>>>,
    pub runtime: Runtime,
    pub udevice_authority: UAuthority,
    pub egress_queue_sender: Sender<UMessage>,
    pub ingress_queue_sender: Sender<UMessage>,
    pub transmit_request_queue_receiver: Receiver<UMessage>,
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

        let runtime_clone = start_args.runtime.clone();
        let udevice_authority_clone = start_args.udevice_authority.clone();
        let egress_queue_sender_clone = start_args.egress_queue_sender.clone();
        let ingress_queue_sender_clone = start_args.ingress_queue_sender.clone();
        let transmit_request_queue_receiver_clone =
            start_args.transmit_request_queue_receiver.clone();
        let transmit_cache_clone = start_args.transmit_cache.clone();
        async_std::task::spawn(run(
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

async fn transmit_queue_request_consumer<T>(
    mut receiver: Receiver<UMessage>,
    transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>>,
    up_client: T,
) where
    T: UTransport + RpcClient + RpcServer,
{
    let local_transmit_cache = transmit_cache.clone();

    while let Ok(msg) = receiver.recv().await {
        trace!("Transmit Request Queue: Received msg: {:?}", msg);

        // TODO: How do we get access to the UpClientPlugin? Just take ownership?

        // TODO: Unpack the UAttributes and then match over the UMessageType
        //  match &type {
        //    UmessageTypePublish => {}
        //    UmessageTypeResponse => {}
        //    UmessageTypeRequest => {}
        //  }
    }
}

fn register_all_remote_listener(
    result: Result<UMessage, UStatus>,
    udevice_authority: UAuthority,
    egress_sender: Sender<UMessage>,
    ingress_sender: Sender<UMessage>,
) {
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
                    egress_sender_clone.send(message).await.unwrap();
                });
            } else {
                info!("CE intended for this uDevice. Sending to Ingress Queue.");
                let ingress_sender_clone = ingress_sender.clone();
                task::spawn(async move {
                    ingress_sender_clone.send(message).await.unwrap();
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
    runtime: Runtime,
    udevice_authority: UAuthority,
    egress_queue_sender: Sender<UMessage>,
    ingress_queue_sender: Sender<UMessage>,
    transmit_request_queue_receiver: Receiver<UMessage>,
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
    task::spawn(async move {
        let listener_closure = move |result: Result<UMessage, UStatus>| {
            register_all_remote_listener(
                result,
                udevice_authority.clone(),
                egress_queue_sender.clone(),
                ingress_queue_sender.clone(),
            );
        };

        // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
        let _registered_all_remote_listener_key = {
            match up_client
                .register_listener(uuri_for_all_remote, Box::new(listener_closure))
                .await
            {
                Ok(registered_key) => registered_key,
                Err(status) => {
                    error!("Failed to register listener: {:?}", status.get_code());
                    return;
                }
            }
        };
    });
    //
    // task::spawn(egress_queue_consumer(
    //     egress_queue_receiver.clone(),
    //     transports.clone(),
    //     transmit_cache.clone(),
    // ));
}
