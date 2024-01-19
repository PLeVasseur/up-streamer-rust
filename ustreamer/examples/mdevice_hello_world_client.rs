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
use uprotocol_sdk::uprotocol::{Data, Remote, u_payload, UAttributes, UAuthority, UMessage, UStatus};
use uprotocol_sdk::{
    transport::datamodel::UTransport,
    uprotocol::{UEntity, UMessageType, UPayload, UResource, UUri},
};
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

    let uuri = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Ip(mdevice_ip.clone())),
        }),
        entity: Some(UEntity {
            name: "mapp".to_string(),
            version_major: Some(1),
            ..Default::default()
        }),
        resource: Some(UResourceBuilder::for_rpc_response()),
    };

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
        .register_listener(uuri, Box::new(callback))
        .await
        .unwrap();

    loop {
        task::sleep(Duration::from_secs(1)).await;

    }
}
