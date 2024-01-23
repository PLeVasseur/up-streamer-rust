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

use async_std::channel::{self, Receiver, Sender};
use log::{debug, error, info, trace, warn};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
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

    // TODO: Should make the transmit_cache configurable
    let mut transmit_cache: Arc<Mutex<LruCache<UuidForHashing, bool>>> =
        Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(1000).unwrap())));

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");

    let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();

    // TODO: Add configuration of which up-clients to start
    let mut up_client_zenoh_config = zenoh::config::Config::default();
    up_client_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");
    let up_client_zenoh = Arc::new(
        ULinkZenoh::new_from_config(up_client_zenoh_config)
            .await
            .unwrap(),
    );

    let mut up_client_sommr_config = zenoh::config::Config::default();
    up_client_sommr_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");
    let up_client_sommr = Arc::new(
        UTransportSommr::new_from_config(up_client_sommr_config)
            .await
            .unwrap(),
    );

    let mut up_client_mqtt_config = zenoh::config::Config::default();
    up_client_mqtt_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");
    let up_client_mqtt = Arc::new(
        UTransportMqtt::new_from_config(up_client_mqtt_config)
            .await
            .unwrap(),
    );

    // TODO: Should consider removing up_client_zenoh_transport from the transports the ingress listener listens in on
    //  as in theory we'd already have received it to any clients apps?
    //  Feels kinda... wrong. I would prefer there be a way to "force" going through the uStreamer, even for Zenoh
    //  as it now appears like from further reading that uP-L1 transport must return to use whether we succeed or fail
    //  Get some feedback on this from @Steven Hartley
    // let up_client_zenoh_transport: Arc<dyn UTransport> =
    //     up_client_zenoh.clone() as Arc<dyn UTransport>;
    let up_client_sommr_transport_tagged: Arc<dyn UTransport> =
        up_client_sommr.clone() as Arc<dyn UTransport>;
    let up_client_mqtt_transport_tagged: Arc<dyn UTransport> =
        up_client_mqtt.clone() as Arc<dyn UTransport>;
    let up_clients_tagged: TransportVec = vec![
        TaggedTransport {
            up_client: up_client_sommr_transport_tagged,
            tag: TransportType::UpClientSommr,
        },
        TaggedTransport {
            up_client: up_client_mqtt_transport_tagged,
            tag: TransportType::UpClientMqtt,
        },
    ];

    // TODO: Add ability to configure this
    const INGRESS_QUEUE_CAPACITY: usize = 5;
    let (ingress_queue_sender, ingress_queue_receiver) =
        channel::bounded::<UMessage>(INGRESS_QUEUE_CAPACITY);

    // TODO: Add ability to configure this
    const EGRESS_QUEUE_CAPACITY: usize = 5;
    let (egress_queue_sender, egress_queue_receiver) =
        channel::bounded::<UMessage>(EGRESS_QUEUE_CAPACITY);

    let ingress_queue_start_args = IngressRouterStartArgs {
        uuid_builder: uuid_builder.clone(),
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        ingress_queue_sender: ingress_queue_sender.clone(),
        ingress_queue_receiver: ingress_queue_receiver.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        up_client_zenoh: up_client_zenoh.clone(),
        transports: up_clients_tagged.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        IngressRouter::start("ingress_router", &ingress_queue_start_args)
            .expect("Failed to start IngressRouter");
    }

    trace!("uStreamer: started IngressRouter");

    let egress_queue_start_args = EgressRouterStartArgs {
        uuid_builder: uuid_builder.clone(),
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        egress_queue_receiver: egress_queue_receiver.clone(),
        up_client_zenoh: up_client_zenoh.clone(),
        transports: up_clients_tagged.clone(),
        transmit_cache: transmit_cache.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        EgressRouter::start("egress_router", &egress_queue_start_args).unwrap();
    }

    trace!("uStreamer: started EgressRouter");

    async_std::future::pending::<()>().await;
}
