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
use example_proto::proto::example::hello_world::v1::{HelloRequest, HelloResponse};
use log::error;
use prost::Message;
use std::sync::{Arc, Mutex};
use std::time;
use uprotocol_sdk::{
    rpc::RpcServer,
    transport::datamodel::UTransport,
    uprotocol::{Data, UEntity, UMessage, UMessageType, UPayload, UPayloadFormat, UStatus, UUri},
    uri::builder::resourcebuilder::UResourceBuilder,
};
use uprotocol_rust_transport_sommr::UTransportSommr;
use zenoh::config::{Config, WhatAmI};

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("mdevice RPC example");

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Client failed");
    let mdevice_rpc_server = Arc::new(Mutex::new(
        UTransportSommr::new_from_config(config).await.unwrap(),
    ));

    // create uuri
    let uuri = UUri {
        entity: Some(UEntity {
            name: "hello_world_service".to_string(),
            version_major: Some(1),
            ..Default::default()
        }),
        resource: Some(UResourceBuilder::for_rpc_request(
            Some("get_hello".to_string()),
            Some(1),
        )),
        ..Default::default()
    };

    let callback = move |result: Result<UMessage, UStatus>| {
        match result {
            Ok(message) => {
                let payload = match message.payload {
                    Some(payload) => payload,
                    None => {
                        error!("No payload attached!");
                        return;
                    }
                };

                let data = match payload.data {
                    Some(data) => data,
                    None => {
                        error!("Empty data payload!");
                        return;
                    }
                };

                if let Data::Value(buf) = data {
                    let hello_request = match HelloRequest::decode(&*buf) {
                        Ok(hello_request) => hello_request,
                        Err(_) => {
                            error!("Failed to decode HelloRequest!");
                            return;
                        }
                    };

                    println!("Received HelloRequest: {}", hello_request.name);
                }
            }
            Err(status) => {
                error!(
                    "HelloRequest returned UStatus: {:?} msg: {}",
                    status.get_code(),
                    status.message()
                );
            }
        }
    };

    println!("Register the listener...");
    mdevice_rpc_server
        .lock()
        .unwrap()
        .register_listener(uuri, Box::new(callback))
        .await
        .unwrap();

    loop {
        task::sleep(time::Duration::from_millis(10000)).await;
        // async_std::future::pending::<()>().await;
    }
}
