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

use std::sync::Arc;
use async_std::task::{self};
use prost::Message;
use std::time::Duration;
use log::{debug, error, info, trace};
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::uprotocol::{Data, Remote, u_payload, UAttributes, UAuthority, UMessage, UPriority, UStatus};
use uprotocol_sdk::{
    transport::datamodel::UTransport,
    uprotocol::{UEntity, UMessageType, UPayload, UResource, UUri},
};
use uprotocol_sdk::transport::builder::UAttributesBuilder;
use uprotocol_sdk::uri::builder::resourcebuilder::UResourceBuilder;
use zenoh::config::Config;
use zenoh::prelude::WhatAmI;

use example_proto::proto::example::hello_world::v1::*;

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();
    // Your example code goes here
    println!("This is an example (stand-in) sender of sommR messages for uStreamer.");

    let uapp_ip = vec![192, 168, 3, 100];
    let mdevice_ip = vec![192, 168, 3, 1];

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Peer failed");
    let utransport_sommr = UTransportSommr::new_from_config(config).await.unwrap();

    let response_resource = UResourceBuilder::for_rpc_response();
    let hello_world_response_uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Ip(mdevice_ip.clone())),
        }),
        entity: Option::from(UEntity {
            name: "ask_for_hello_service".to_string(),
            id: Option::Some(111),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(response_resource),
    };

    let request_resource =
        UResourceBuilder::for_rpc_request(Some("get_hello".to_string()), Some(1));
    let hello_world_request_uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Ip(uapp_ip.clone())),
        }),
        entity: Option::from(UEntity {
            name: "hello_world_service".to_string(),
            id: Option::Some(111),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(request_resource),
    };

    let mut attributes = UAttributesBuilder::request(
        UPriority::UpriorityCs4,
        hello_world_request_uuri.clone(),
        2000,
    )
    .build();

    attributes.sink = Some(hello_world_request_uuri.clone());

    let utransport_sommr_arc = Arc::new(utransport_sommr);

    let callback = move |result: Result<UMessage, UStatus>| {
        trace!("callback: entered");

        match result {
            Ok(message) => {
                let Some(attributes) = message.attributes else {
                    error!("No attributes attached");
                    return;
                };

                let Some(sink) = attributes.sink else {
                    error!("No sink attached");
                    return;
                };

                debug!(
                    "sink: {:?}",
                    &sink
                );

                if let Some(authority) = sink.authority {
                    if let Some(Remote::Ip(ip)) = authority.remote {
                        debug!("ip: {:?}", ip);

                        if ip != mdevice_ip {
                            info!(
                                "Don't react to messages not directed to us. remote ip: {:?}",
                                ip
                            );
                            return;
                        }
                    }
                }

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
                    let hello_response = match HelloResponse::decode(&*buf) {
                        Ok(hello_response) => hello_response,
                        Err(_) => {
                            error!("Failed to decode HelloResponse!");
                            return;
                        }
                    };


                    println!("{}", &hello_response.message);
                }
            }
            Err(status) => {
                error!(
                    "HelloResponse returned UStatus: {:?} msg: {}",
                    status.get_code(),
                    status.message()
                );
            }
        }
    };

    info!("Register the listener...");
    utransport_sommr_arc
        .register_listener(hello_world_response_uuri.clone(), Box::new(callback))
        .await
        .unwrap();

    let mut hello_attempt = 0;

    loop {
        task::sleep(Duration::from_secs(1)).await;

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
            data: Some(Data::Value(hello_request_buf)),
        };

        match utransport_sommr_arc
            .send(
                hello_world_response_uuri.clone(),
                hello_request_payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(_) => {
                println!("Sending HelloRequest succeeded");
            }
            Err(status) => {
                println!("Sending HelloRequest failed: {:?}", status)
            }
        }

        hello_attempt += 1;
    }
}
