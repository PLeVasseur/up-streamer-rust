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

use async_std::task::{self, block_on};
use example_proto::proto::example::hello_world::v1::HelloResponse;
use prost::Message;
use std::sync::{Arc, Mutex};
use std::time;
use uprotocol_sdk::{
    rpc::RpcServer,
    transport::datamodel::UTransport,
    uprotocol::{Data, Remote, UAuthority, UEntity, UMessage, UMessageType, UPayload, UPayloadFormat, UStatus, UUri},
    uri::builder::resourcebuilder::UResourceBuilder,
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::{Config, WhatAmI};

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("uProtocol RPC server example");

    let uapp_ip = vec![192, 168, 3, 100];
    let mdevice_ip = vec![192, 168, 3, 1];

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Client failed");
    let rpc_server = Arc::new(Mutex::new(
        ULinkZenoh::new_from_config(config).await.unwrap(),
    ));

    // create uuri
    let uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Ip(uapp_ip.clone())),
        }),
        entity: Some(UEntity {
            name: "hello_world_service".to_string(),
            id: Some(111),
            version_major: Some(1),
            ..Default::default()
        }),
        resource: Some(UResourceBuilder::for_rpc_request(
            Some("get_hello".to_string()),
            Some(1),
        ))
    };

    let rpc_server_callback = rpc_server.clone();
    let callback = move |result: Result<UMessage, UStatus>| {
        match result {
            Ok(msg) => {
                let UMessage {
                    source,
                    attributes,
                    payload,
                } = msg;
                // Get the UUri
                let uuri = source.unwrap();
                // Build the payload to send back
                if let Data::Value(v) = payload.unwrap().data.unwrap() {
                    let value = v.into_iter().map(|c| c as char).collect::<String>();
                    println!("Receive {} from {}", value, uuri);
                }
                let hello_response = HelloResponse {
                    message: "Hello there!".to_string(),
                };
                let mut hello_response_buf = Vec::new();
                hello_response
                    .encode(&mut hello_response_buf)
                    .expect("Failed to encode");
                let upayload = UPayload {
                    length: Some(hello_response_buf.len() as i32),
                    format: UPayloadFormat::UpayloadFormatProtobuf as i32,
                    data: Some(Data::Value(hello_response_buf)),
                };
                // Set the attributes type to Response
                let mut uattributes = attributes.unwrap();
                uattributes.set_type(UMessageType::UmessageTypeResponse);
                // Send back result
                block_on(
                    rpc_server_callback
                        .lock()
                        .unwrap()
                        .send(uuri, upayload, uattributes),
                )
                .unwrap();
            }
            Err(ustatus) => {
                println!("Internal Error: {:?}", ustatus);
            }
        }
    };

    println!("Register the listener...");
    rpc_server
        .lock()
        .unwrap()
        .register_rpc_listener(uuri, Box::new(callback))
        .await
        .unwrap();

    loop {
        task::sleep(time::Duration::from_millis(10000)).await;
    }
}
