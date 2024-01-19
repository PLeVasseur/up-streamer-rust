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

use async_std::task::{self};
use example_proto::proto::example::hello_world::v1::{HelloRequest, HelloResponse};
use log::{debug, error, info, trace};
use prost::Message;
use std::sync::Arc;
use std::time;
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::transport::builder::UAttributesBuilder;
use uprotocol_sdk::uprotocol::{u_payload, Remote, UAuthority, UPriority};
use uprotocol_sdk::{
    transport::datamodel::UTransport,
    uprotocol::{Data, UEntity, UMessage, UPayload, UStatus, UUri},
    uri::builder::resourcebuilder::UResourceBuilder,
};
use zenoh::config::{Config, WhatAmI};

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("mdevice RPC example");

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Client failed");
    let mdevice_rpc_server = Arc::new(UTransportSommr::new_from_config(config).await.unwrap());

    let uapp_ip = vec![192, 168, 3, 100];
    let mdevice_ip = vec![192, 168, 3, 1];

    // create uuri
    let uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Ip(mdevice_ip.clone())),
        }),
        entity: Some(UEntity {
            name: "hello_world_service".to_string(),
            version_major: Some(1),
            ..Default::default()
        }),
        resource: Some(UResourceBuilder::for_rpc_request(
            Some("get_hello".to_string()),
            Some(1),
        )),
    };

    let mdevice_rpc_server_for_callback = mdevice_rpc_server.clone();

    let callback = move |result: Result<UMessage, UStatus>| {
        trace!("callback: entered");

        match result {
            Ok(message) => {
                let Some(mut hello_response_destination) = message.source else {
                    error!("Unable to get destination UUri");
                    return;
                };

                debug!(
                    "hello_response_destination: {:?}",
                    hello_response_destination
                );

                if let Some(authority) = hello_response_destination.authority {
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

                hello_response_destination.authority = Some(UAuthority {
                    remote: Some(Remote::Ip(uapp_ip.clone())),
                });

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

                let mut hello_response = HelloResponse {
                    message: "".to_string(),
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

                    hello_response.message = format!("Hello there, {}!", hello_request.name);
                }

                let Some(hello_request_attributes) = message.attributes else {
                    error!("Unable to get attributes");
                    return;
                };

                let Some(hello_request_reqid) = hello_request_attributes.id else {
                    error!("Unable to get id to use as reqid");
                    return;
                };

                let hello_response_attributes = UAttributesBuilder::response(
                    UPriority::UpriorityCs4,
                    hello_response_destination.clone(),
                    hello_request_reqid,
                )
                .build();

                let mut hello_response_buf = Vec::new();
                hello_response
                    .encode(&mut hello_response_buf)
                    .expect("Failed to encode");

                let hello_response_payload = UPayload {
                    length: Some(hello_response_buf.len() as i32),
                    format: 0,
                    data: Some(u_payload::Data::Value(hello_response_buf)),
                };

                let mdevice_rpc_server_within_async_spawn = mdevice_rpc_server_for_callback.clone();
                task::spawn(async move {
                    match mdevice_rpc_server_within_async_spawn
                        .send(
                            hello_response_destination.clone(),
                            hello_response_payload,
                            hello_response_attributes,
                        )
                        .await
                    {
                        Ok(_) => {
                            info!("Sending HelloResponse succeeded");
                        }
                        Err(status) => {
                            error!("Sending HelloResponse failed: {:?}", status)
                        }
                    }
                });
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

    info!("Register the listener...");
    mdevice_rpc_server
        .register_listener(uuri, Box::new(callback))
        .await
        .unwrap();

    loop {
        task::sleep(time::Duration::from_millis(10000)).await;
        // async_std::future::pending::<()>().await;
    }
}
