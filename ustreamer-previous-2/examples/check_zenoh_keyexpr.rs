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

extern crate example_proto;
extern crate prost;
extern crate uprotocol_sdk;
extern crate uprotocol_zenoh_rust;

use async_std::task::{self};
use example_proto::proto::example::hello_world::v1::*;
use log::{debug, trace};
use prost::Message;
use std::time;
use std::time::Duration;
use uprotocol_sdk::rpc::RpcClient;
use uprotocol_sdk::rpc::RpcServer;
use uprotocol_sdk::transport::builder::UAttributesBuilder;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{u_payload, Data, Remote, UAuthority, UPriority, Uuid};
use uprotocol_sdk::uprotocol::{UEntity, UMessage, UPayload, UStatus, UUri};
use uprotocol_sdk::uri::builder::resourcebuilder::UResourceBuilder;
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::Config;
use zenoh::prelude::WhatAmI;

#[async_std::main]
async fn main() {
    println!("Zenoh key expr example");

    let mdevice_ip = vec![192, 168, 3, 1];
    let uapp_ip = vec![192, 169, 3, 100];

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Peer failed");
    let rpc_server = ULinkZenoh::new_from_config(config).await.unwrap();

    let response_resource = UResourceBuilder::for_rpc_response();
    let local_authority_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "hello_world_service".to_string(),
            id: Option::Some(111),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(response_resource.clone()),
    };

    let remote_authority_uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Name("remote".to_string())),
        }),
        entity: Option::from(UEntity {
            name: "hello_world_service".to_string(),
            id: Option::Some(111),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(response_resource.clone()),
    };

    let local_authority_with_entity_no_resource_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "hello_world_service".to_string(),
            id: Option::Some(111),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: None,
    };

    let remote_authority_no_entity_no_resource_uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Name("remote".to_string())),
        }),
        entity: None,
        resource: None,
    };

    let callback = |res: Result<UMessage, UStatus>| {};

    println!("Register the local authority listener...");
    rpc_server
        .register_rpc_listener(local_authority_uuri, Box::new(callback))
        .await
        .unwrap();

    println!("Register the remote authority listener...");
    rpc_server
        .register_rpc_listener(remote_authority_uuri, Box::new(callback))
        .await
        .unwrap();

    println!("Register the local authority listener with a UEntity and no UResource...");
    rpc_server
        .register_listener(
            local_authority_with_entity_no_resource_uuri,
            Box::new(callback),
        )
        .await
        .unwrap();

    println!("Register the remote authority listener with no UEntity and no UResource...");
    rpc_server
        .register_listener(
            remote_authority_no_entity_no_resource_uuri,
            Box::new(callback),
        )
        .await
        .unwrap();

    async_std::future::pending::<()>().await;
}
