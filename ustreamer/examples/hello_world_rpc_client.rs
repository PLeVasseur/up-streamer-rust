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
use prost::Message;
use std::time::Duration;
use uprotocol_sdk::rpc::RpcClient;
use uprotocol_sdk::transport::builder::UAttributesBuilder;
use uprotocol_sdk::uprotocol::{u_payload, Data, UPriority, Uuid};
use uprotocol_sdk::uprotocol::{UEntity, UPayload, UUri};
use uprotocol_sdk::uri::builder::resourcebuilder::UResourceBuilder;
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::Config;
use zenoh::prelude::WhatAmI;

#[async_std::main]
async fn main() {
    println!("uProtocol RPC client example");

    // let locator = vec![String::from("tcp/127.0.0.1:17449")];

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Peer failed");
    // config
    //     .connect
    //     .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
    //     .unwrap();
    let ulink = ULinkZenoh::new_from_config(config).await.unwrap();

    let request_resource =
        UResourceBuilder::for_rpc_request(Some("get_hello".to_string()), Some(1));
    let hello_world_request_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "hello_world_service".to_string(),
            id: Option::Some(111),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(request_resource),
    };

    let attributes = UAttributesBuilder::request(
        UPriority::UpriorityCs4,
        hello_world_request_uuri.clone(),
        100,
    )
    .with_reqid(Uuid {
        msb: 0x0000000000018000u64,
        lsb: 0x8000000000000000u64,
    })
    .build();

    println!("hello_world_request_uuri: {:?}", hello_world_request_uuri);

    let mut hello_attempt = 0;

    loop {
        task::sleep(Duration::from_secs(1)).await;
        println!("Attempting send of hello world...");

        let hello_request = HelloRequest {
            name: format!("Please tell me hello {}", hello_attempt).to_string(),
        };
        let mut hello_request_buf = Vec::new();
        hello_request
            .encode(&mut hello_request_buf)
            .expect("Failed to encode");
        let hello_request_payload = UPayload {
            length: Some(hello_request_buf.len() as i32),
            format: 0,
            data: Some(u_payload::Data::Value(hello_request_buf)),
        };

        match ulink
            .invoke_method(
                hello_world_request_uuri.clone(),
                hello_request_payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(payload) => {
                let data = match payload.data {
                    Some(data) => data,
                    None => {
                        println!("Empty data payload!");
                        return;
                    }
                };

                if let Data::Value(buf) = data {
                    let hello_response = match HelloResponse::decode(&*buf) {
                        Ok(hello_response) => hello_response,
                        Err(_) => {
                            println!("Failed to decode HelloResponse!");
                            return;
                        }
                    };

                    println!("The HelloResponse was: {}", hello_response.message);
                }
            }
            Err(e) => {
                println!("invoke_method failed: {:?}", e)
            }
        }

        hello_attempt += 1;
    }
}
