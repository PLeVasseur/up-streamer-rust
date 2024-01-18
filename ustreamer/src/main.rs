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

use async_std::task;
use std::sync::Arc;
use std::time::Duration;
use uprotocol_sdk::uprotocol::{Remote, UAuthority, UEntity, UMessage, UStatus, UUri};
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_zenoh_rust::ULinkZenoh;
use uprotocol_rust_transport_sommr::UTransportSommr;
use zenoh::prelude::r#async::AsyncResolve;
use zenoh::prelude::*;

// Temporarily comment out -- try to bring in the plugin
#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("Hello, uStreamer!");

    let mut raw_zenoh_config = zenoh::config::Config::default();
    raw_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Router");
    let Ok(session) = zenoh::open(raw_zenoh_config.clone()).res().await else {
        println!("Failed to open Zenoh Router session");
        return;
    };

    let mut sommr_zenoh_config = zenoh::config::Config::default();
    sommr_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Router");

    let mut ulink_zenoh_config = zenoh::config::Config::default();
    ulink_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Router");
    // let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();
    // let ulink_zenoh = ULinkZenoh::new_from_runtime(runtime.clone()).await.unwrap();
    // let utransport_sommr = UTransportSommr::new_from_runtime(runtime.clone()).await.unwrap();

    let ulink_zenoh = ULinkZenoh::new_from_config(ulink_zenoh_config.clone()).await.unwrap();
    let utransport_sommr = UTransportSommr::new_from_config(sommr_zenoh_config.clone()).await.unwrap();
    let uuri_for_all_remote = UUri{
        authority: Some(UAuthority{ remote: Some(Remote::Name("*".to_string())) }),
        entity: Some(UEntity{
            name: "*".to_string(),
            id: None,
            version_major: None,
            version_minor: None,
        }),
        resource: None,
    };

    let ulink_zenoh_arc = Arc::new(ulink_zenoh);

    let sommr_callback = move | result: Result<UMessage, UStatus> | {
        println!("entered sommr_callback");

        let Ok(msg) = result else {
            println!("received error");
            return;
        };

        println!("sommr_callback: got msg");

        let Some(source) = msg.source else {
            println!("no source");
            return;
        };

        println!("sommr_callback: got source");

        let Some(payload) = msg.payload else {
            println!("no payload");
            return;
        };

        println!("sommr_callback: got payload");

        let Some(attributes) = msg.attributes else {
            println!("no attributes");
            return;
        };

        println!("sommr_callback: got attributes");

        println!("Source: {}", &source);

        let ulink_zenoh_clone = ulink_zenoh_arc.clone();
        task::spawn(async move {
            match ulink_zenoh_clone
                .send(
                    source,
                    payload,
                    attributes
                )
                .await
            {
                Ok(_) => {
                    println!("Forwarding message succeeded");
                }
                Err(status) => {
                    println!("Forwarding message failed: {:?}", status)
                }
            }

            println!("sommr_callback: ulink_zenoh_clone.send() within async");
        });
        println!("sommr_callback: after ulink_zenoh_clone.send()");
    };

    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let _registered_all_remote_sommr_key = {
        match utransport_sommr
            .register_listener(uuri_for_all_remote, Box::new(sommr_callback))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                println!(
                    "Failed to register sommr_remote_listener: {:?} {}",
                    status.get_code(),
                    status.message()
                );
                return;
            }
        }
    };

    let session_arc = Arc::new(session);
    let session_arc_clone_mainthread = session_arc.clone();

    // Define a callback function to process incoming messages
    let zenoh_sub_callback = move |sample: Sample| {
        println!("Zenoh up/ subscriber callback");

        let key_expr = sample.key_expr.clone();
        let payload = sample.value.payload.clone();

        println!("Received on '{}': '{:?}'", &key_expr, &payload);
    };

    // Attach the callback function to a subscriber that listens to all paths
    let _subscriber = session_arc_clone_mainthread
        .declare_subscriber("up/**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
        // .declare_subscriber("up/**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
        .callback_mut(zenoh_sub_callback)
        .res()
        .await;

    // Infinite loop in main thread, just letting the uStreamer listen and retransmit
    loop {
        task::sleep(Duration::from_millis(1000)).await;
        // async_std::future::pending::<()>().await;
    }
}
