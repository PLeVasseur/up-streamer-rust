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

mod plugins;

use crate::plugins::egress_router::{EgressRouter, EgressRouterStartArgs};
use crate::plugins::ingress_router::{IngressRouter, IngressRouterStartArgs};
use crate::plugins::types::*;
use crate::plugins::up_client_full::{
    UpClientFull, UpClientFullFactory, UpClientPlugin, UpClientPluginStartArgs,
};
use std::cell::RefCell;

use async_std::channel::{self, Receiver, Sender};
use async_std::sync::{Arc, Mutex};
use log::{debug, error, info, trace, warn};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use uprotocol_rust_transport_mqtt::UTransportMqtt;
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{Remote, UAuthority, UMessage, Uuid};
use uprotocol_sdk::uuid::builder::UUIDv8Builder;
use uprotocol_zenoh_rust::ULinkZenoh;
use uuid::Uuid as UuidForHashing;
use zenoh::scouting::WhatAmI;

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("Starting uStreamer!");

    let uuid_builder = Arc::new(UUIDv8Builder::new());

    // TODO: Add configuration of local UAuthority
    let ustreamer_device_ip: Vec<u8> = vec![192, 168, 3, 100];
    let ustreamer_device_authority: UAuthority = UAuthority {
        remote: Some(Remote::Ip(ustreamer_device_ip)),
    };

    // TODO: Add configuration of host transport
    const HOST_TRANSPORT: TransportType = TransportType::UpClientZenoh;

    // TODO: Add configuration of UAuthority => TransportType
    let uapp_authority = UAuthority {
        remote: Some(Remote::Ip(vec![192, 168, 3, 100])),
    };
    let mdevice_authority = UAuthority {
        remote: Some(Remote::Ip(vec![192, 168, 3, 1])),
    };
    let cloud_authority = UAuthority {
        remote: Some(Remote::Ip(vec![192, 168, 3, 200])),
    };
    let authority_transport_mapping = Arc::new(Mutex::new(HashMap::from([
        (
            HashableAuthority(uapp_authority.clone()),
            TransportType::UpClientZenoh,
        ),
        (
            HashableAuthority(cloud_authority.clone()),
            TransportType::UpClientMqtt,
        ),
        (
            HashableAuthority(mdevice_authority.clone()),
            TransportType::UpClientSommr,
        ),
    ])));

    // TODO: Should make the transmit_cache configurable
    let mut transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>> =
        Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(1000).unwrap())));

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");

    let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();

    // TODO: Add ability to configure this
    const INGRESS_QUEUE_CAPACITY: usize = 100;
    let (ingress_queue_sender, ingress_queue_receiver) =
        channel::bounded::<UMessageWithRouting>(INGRESS_QUEUE_CAPACITY);

    // TODO: Add ability to configure this
    const EGRESS_QUEUE_CAPACITY: usize = 100;
    let (egress_queue_sender, egress_queue_receiver) =
        channel::bounded::<UMessageWithRouting>(EGRESS_QUEUE_CAPACITY);

    // TODO: Modularize this s.t. we can choose which transports should be started

    // TODO: Add ability to configure this
    const ZENOH_UP_CLIENT_QUEUE_CAPACITY: usize = 100;
    let (zenoh_transmit_request_queue_sender, zenoh_transmit_request_queue_receiver) =
        channel::bounded::<UMessageWithRouting>(ZENOH_UP_CLIENT_QUEUE_CAPACITY);

    // TODO: Add ability to configure this
    const SOMMR_UP_CLIENT_QUEUE_CAPACITY: usize = 100;
    let (sommr_transmit_request_queue_sender, sommr_transmit_request_queue_receiver) =
        channel::bounded::<UMessageWithRouting>(SOMMR_UP_CLIENT_QUEUE_CAPACITY);

    // TODO: Add ability to configure this
    const MQTT_UP_CLIENT_QUEUE_CAPACITY: usize = 100;
    let (mqtt_transmit_request_queue_sender, mqtt_transmit_request_queue_receiver) =
        channel::bounded::<UMessageWithRouting>(MQTT_UP_CLIENT_QUEUE_CAPACITY);

    let up_client_zenoh_factory: RefCell<Option<Box<dyn UpClientFullFactory>>> =
        RefCell::new(Some(Box::new(ULinkZenohFactory {})));
    let up_client_zenoh_start_args = UpClientPluginStartArgs {
        host_transport: HOST_TRANSPORT,
        transport_type: TransportType::UpClientZenoh,
        up_client_factory: up_client_zenoh_factory,
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        ingress_queue_sender: ingress_queue_sender.clone(),
        transmit_request_queue_receiver: zenoh_transmit_request_queue_receiver.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        UpClientPlugin::start("up_client_zenoh", &up_client_zenoh_start_args)
            .expect("Failed to start up_client_zenoh plugin");
    }

    let up_client_sommr_factory: RefCell<Option<Box<dyn UpClientFullFactory>>> =
        RefCell::new(Some(Box::new(UTransportSommrFactory {})));
    let up_client_sommr_start_args = UpClientPluginStartArgs {
        host_transport: HOST_TRANSPORT,
        transport_type: TransportType::UpClientSommr,
        up_client_factory: up_client_sommr_factory,
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        ingress_queue_sender: ingress_queue_sender.clone(),
        transmit_request_queue_receiver: sommr_transmit_request_queue_receiver.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        UpClientPlugin::start("up_client_sommr", &up_client_sommr_start_args)
            .expect("Failed to start up_client_sommr plugin");
    }

    let up_client_mqtt_factory: RefCell<Option<Box<dyn UpClientFullFactory>>> =
        RefCell::new(Some(Box::new(UTransportMqttFactory {})));
    let up_client_mqtt_start_args = UpClientPluginStartArgs {
        host_transport: HOST_TRANSPORT,
        transport_type: TransportType::UpClientMqtt,
        up_client_factory: up_client_mqtt_factory,
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        ingress_queue_sender: ingress_queue_sender.clone(),
        transmit_request_queue_receiver: mqtt_transmit_request_queue_receiver.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        UpClientPlugin::start("up_client_mqtt", &up_client_mqtt_start_args)
            .expect("Failed to start up_client_mqtt plugin");
    }

    let transmit_queue_senders_tagged = Arc::new(Mutex::new(HashMap::from([
        (
            TransportType::UpClientZenoh,
            zenoh_transmit_request_queue_sender.clone(),
        ),
        (
            TransportType::UpClientMqtt,
            mqtt_transmit_request_queue_sender.clone(),
        ),
        (
            TransportType::UpClientSommr,
            sommr_transmit_request_queue_sender.clone(),
        ),
    ])));

    let ingress_queue_start_args = IngressRouterStartArgs {
        host_transport: HOST_TRANSPORT,
        uuid_builder: uuid_builder.clone(),
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        ingress_queue_sender: ingress_queue_sender.clone(),
        ingress_queue_receiver: ingress_queue_receiver.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        transmit_request_senders: transmit_queue_senders_tagged.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        IngressRouter::start("ingress_router", &ingress_queue_start_args)
            .expect("Failed to start IngressRouter");
    }

    trace!("uStreamer: started IngressRouter");

    let egress_queue_start_args = EgressRouterStartArgs {
        host_transport: HOST_TRANSPORT,
        authority_transport_mapping: authority_transport_mapping.clone(),
        uuid_builder: uuid_builder.clone(),
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        egress_queue_receiver: egress_queue_receiver.clone(),
        transmit_request_senders: transmit_queue_senders_tagged.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        EgressRouter::start("egress_router", &egress_queue_start_args).unwrap();
    }

    trace!("uStreamer: started EgressRouter");

    async_std::future::pending::<()>().await;
}
