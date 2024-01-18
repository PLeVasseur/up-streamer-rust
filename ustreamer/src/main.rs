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
use log::{debug, error, info, trace};
use uprotocol_sdk::uprotocol::{Remote, UAuthority, UEntity, UMessage, UStatus, UUri};
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_zenoh_rust::ULinkZenoh;
use uprotocol_rust_transport_sommr::UTransportSommr;
use zenoh::prelude::r#async::AsyncResolve;
use zenoh::prelude::*;

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    info!("Hello, uStreamer!");

    let mut raw_zenoh_config = zenoh::config::Config::default();
    raw_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Router");
    let Ok(session) = zenoh::open(raw_zenoh_config.clone()).res().await else {
        error!("Failed to open Zenoh Router session");
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
        trace!("entered sommr_callback");

        let Ok(msg) = result else {
            error!("no msg");
            return;
        };

        trace!("sommr_callback: got msg");

        let Some(source) = msg.source else {
            error!("no source");
            return;
        };

        trace!("sommr_callback: got source");

        let Some(payload) = msg.payload else {
            error!("no payload");
            return;
        };

        trace!("sommr_callback: got payload");

        let Some(attributes) = msg.attributes else {
            error!("no attributes");
            return;
        };

        trace!("sommr_callback: got attributes");

        info!("sommr_callback: Source: {}", &source);

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
                    info!("Forwarding message succeeded");
                }
                Err(status) => {
                    error!("Forwarding message failed: {:?}", status)
                }
            }

            trace!("sommr_callback: ulink_zenoh_clone.send() within async");
        });
        trace!("sommr_callback: after ulink_zenoh_clone.send()");
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
    let session_arc_clone_subscriber_callback = session_arc.clone();

    // Define a callback function to process incoming messages
    let zenoh_sub_callback = move |sample: Sample| {
        trace!("zenoh_sub_callback: Zenoh up/ subscriber callback");

        let key_expr = sample.key_expr.clone();
        let payload = sample.value.payload.clone();

        // Check if the key expression starts with "@"
        if key_expr.starts_with('@') {
            debug!("Ignoring message with key expression: '{}'", key_expr);
            return; // Skip processing this message
        }

        trace!("zenoh_sub_callback: after key_expr @ check");

        // TODO: Need to check this will still work after the move to micro form
        //  Perhaps they'll just append all the numbers together with some . or /
        //  So my guess is it'd be best to just add a number here, let's say 535
        //  --This mechanism is only needed now because we're listening in on and transmitting
        //  over the same transport and can be removed when we're retransmitting over SOME/IP
        if key_expr.ends_with("535") {
            debug!("Ignoring message with key expression: '{}'", key_expr);
            return; // Skip processing this message
        }

        let Some(attachment) = sample.attachment() else {
            error!(
                "Message missing attachment, skip key expression: '{}'",
                key_expr
            );
            return;
        };

        trace!("zenoh_sub_callback: got attachment");

        let attachment_clone = attachment.clone();

        trace!("Received on '{}': '{:?}'", &key_expr, &payload);

        let session_clone = session_arc_clone_subscriber_callback.clone();

        let retransmit_key_expr = key_expr.concat("535").expect("unable to append retransmit");

        let Ok(encoding) = sample.encoding.suffix().parse::<i32>() else {
            error!("Unable to get encoding for key expression: '{}'", &key_expr);
            return;
        };

        trace!("zenoh_sub_callback: got encoding");

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
                error!("Failed to send message: {:?}", e);
            }
            trace!("zenoh_sub_callback: sent via Zenoh inside async");
        });

        trace!("zenoh_sub_callback: sent via Zenoh");
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
