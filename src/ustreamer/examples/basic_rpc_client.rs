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

use std::str::FromStr;
use uprotocol_sdk::{
    rpc::RpcClient,
    transport::builder::UAttributesBuilder,
    uprotocol::{Data, UEntity, UPayload, UPayloadFormat, UPriority, UUri, Uuid},
    uri::builder::resourcebuilder::UResourceBuilder,
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::{Config, ModeDependentValue, WhatAmI, WhatAmIMatcher};

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("uProtocol RPC client example");

    let locator = vec![String::from("tcp/127.0.0.1:17449")];

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Client))
        .expect("Setting as Client failed");
    config
        .connect
        .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
        .unwrap();
    config.scouting.multicast.set_enabled(Some(false)).unwrap();
    config.scouting.gossip.set_enabled(Some(false)).unwrap();
    config
        .routing
        .router
        .set_peers_failover_brokering(Some(false))
        .unwrap();
    let rpc_client = ULinkZenoh::new(config).await.unwrap();

    // create uuri
    let uuri = UUri {
        entity: Some(UEntity {
            name: "test_rpc.app".to_string(),
            version_major: Some(1),
            ..Default::default()
        }),
        resource: Some(UResourceBuilder::for_rpc_request(
            Some("getTime".to_string()),
            None,
        )),
        ..Default::default()
    };

    // create uattributes
    // TODO: Check TTL (Should TTL map to Zenoh's timeout?)
    // TODO: It's a little strange to create UUID by users
    let attributes = UAttributesBuilder::request(UPriority::UpriorityCs4, uuri.clone(), 100)
        .with_reqid(Uuid {
            msb: 0x0000000000018000u64,
            lsb: 0x8000000000000000u64,
        })
        .build();

    // create uPayload
    let data = String::from("GetCurrentTime");
    let payload = UPayload {
        length: Some(0),
        format: UPayloadFormat::UpayloadFormatText as i32,
        data: Some(Data::Value(data.as_bytes().to_vec())),
    };

    // invoke RPC method
    println!("Send request to {}", uuri.to_string());
    let result = rpc_client.invoke_method(uuri, payload, attributes).await;

    // process the result
    if let Data::Value(v) = result.unwrap().data.unwrap() {
        let value = v.into_iter().map(|c| c as char).collect::<String>();
        println!("Receive {}", value);
    } else {
        println!("Failed to get result from invoke_method.");
    }
}
