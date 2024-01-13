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
use zenoh::prelude::r#async::AsyncResolve;
use zenoh::prelude::*;

#[async_std::main]
async fn main() {
    println!("Hello, uStreamer!");

    // Initialize the Zenoh runtime
    let locator = vec![String::from("tcp/127.0.0.1:17449")];

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Router))
        .expect("Unable to configure as Router");
    config
        .listen
        .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
        .unwrap();
    let Ok(session) = zenoh::open(config).res().await else {
        println!("Failed to open Zenoh Router session");
        return;
    };

    let session_arc = Arc::new(session);
    let session_arc_clone_callback = session_arc.clone();
    let session_arc_clone_mainthread = session_arc.clone();

    // Define a callback function to process incoming messages
    let callback = move |sample: Sample| {
        let key_expr = sample.key_expr.clone();
        let payload = sample.value.payload.clone();

        // Check if the key expression starts with "@"
        if key_expr.starts_with('@') {
            println!("Ignoring message with key expression: '{}'", key_expr);
            return; // Skip processing this message
        }

        if key_expr.ends_with("retransmit") {
            println!("Ignoring message with key expression: '{}'", key_expr);
            return; // Skip processing this message
        }

        let Some(attachment) = sample.attachment() else {
            println!(
                "Message missing attachment, skip key expression: '{}'",
                key_expr
            );
            return;
        };

        let attachment_clone = attachment.clone();

        println!("Received on '{}': '{:?}'", &key_expr, &payload);

        let session_clone = session_arc_clone_callback.clone();

        let retransmit_key_expr = key_expr
            .join("retransmit")
            .expect("unable to append retransmit");

        let Ok(encoding) = sample.encoding.suffix().parse::<i32>() else {
            println!("Unable to get encoding for key expression: '{}'", key_expr);
            return;
        };

        task::spawn(async move {
            let session_clone = session_clone.clone();
            let putbuilder = session_clone
                .put(retransmit_key_expr, payload)
                .encoding(Encoding::WithSuffix(
                    KnownEncoding::AppCustom,
                    encoding.to_string().into(),
                ))
                .with_attachment(attachment_clone);

            if let Err(e) = putbuilder.res().await {
                eprintln!("Failed to send message: {:?}", e);
            }
        });
    };

    // Attach the callback function to a subscriber that listens to all paths
    let _subscriber = session_arc_clone_mainthread
        .declare_subscriber("**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
        .callback_mut(callback)
        .res()
        .await;

    // Infinite loop in main thread, just letting the uStreamer listen and retransmit
    loop {
        task::sleep(Duration::from_secs(10)).await;
    }
}
