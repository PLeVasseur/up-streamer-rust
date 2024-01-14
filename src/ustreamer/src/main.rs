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
use prost::Message;
use std::sync::Arc;
use std::time::Duration;
use uprotocol_sdk::rpc::RpcMapperError;
use uprotocol_sdk::uprotocol::{
    Data, UAttributes, UCode, UMessage, UPayload, UPayloadFormat, UStatus,
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::buffers::ZBuf;
use zenoh::prelude::r#async::AsyncResolve;
use zenoh::prelude::*;
use zenoh::queryable::Query;

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("Hello, uStreamer!");

    // Initialize the Zenoh runtime
    let locator = vec![String::from("tcp/127.0.0.1:17449")];

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        // .set_mode(Some(WhatAmI::Router))
        .expect("Unable to configure as Router");
    config
        .listen
        .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
        .unwrap();
    config
        .routing
        .router
        .set_peers_failover_brokering(Some(false))
        .unwrap();
    config.scouting.multicast.set_enabled(Some(false)).unwrap();
    config.scouting.gossip.set_enabled(Some(false)).unwrap();
    let Ok(session) = zenoh::open(config).res().await else {
        println!("Failed to open Zenoh Router session");
        return;
    };

    let session_arc = Arc::new(session);
    let session_arc_clone_subscriber_callback = session_arc.clone();
    let session_arc_clone_queryable_callback = session_arc.clone();
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

        // TODO: Need to check this will still work after the move to micro form
        //  Perhaps they'll just append all the numbers together with some . or /
        //  So my guess is it'd be best to just add a number here, let's say 535
        //  --This mechanism is only needed now because we're listening in on and transmitting
        //  over the same transport and can be removed when we're retransmitting over SOME/IP
        if key_expr.ends_with("535") {
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

        let session_clone = session_arc_clone_subscriber_callback.clone();

        let retransmit_key_expr = key_expr.concat("535").expect("unable to append retransmit");

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

    let get_callback = move |query: Query| {
        let key_expr = query.key_expr().clone();

        // Check if the key expression starts with "@"
        if key_expr.starts_with('@') {
            println!("Ignoring message with key expression: '{}'", key_expr);
            return;
        }

        let Some(value) = query.value() else {
            println!("query lacked value: {}", query.key_expr());
            return;
        };

        let value_clone = value.clone();

        let Some(attachment) = query.attachment() else {
            println!("query lacked appropriate attachment: {}", query.key_expr());
            return;
        };

        if key_expr.ends_with("123") {
            println!("Ignoring query with key expression: '{}'", key_expr);
            return; // Skip processing this message as it's a retransmit
        }

        let attachment_clone = attachment.clone();

        println!("Received on '{}': '{:?}'", &key_expr, &value);

        let session_clone = session_arc_clone_queryable_callback.clone();

        let retransmit_key_expr = key_expr.concat("123").expect("unable to append retransmit");

        task::spawn(async move {
            // TODO:
            //  1. Extract the relevant info from the query needed for the GetBuilder:
            // // Send data
            // // TODO: Query should support .encoding
            // // TODO: Adjust the timeout

            let getbuilder = session_clone
                .get(&retransmit_key_expr)
                .with_value(value_clone)
                .with_attachment(attachment_clone)
                .target(QueryTarget::BestMatching)
                .timeout(Duration::from_millis(1000));

            // 2. Forward the query on
            // Send the query

            let Ok(replies) = getbuilder.res().await else {
                println!("Error while sending Zenoh query");
                return;
            };

            // 3. If the reply was Ok, then we can reply... something like this

            let reply = match replies.recv_async().await {
                Ok(reply) => {
                    println!("Got reply back from server");

                    let mut sample_res = reply.sample;

                    if let Ok(ref mut res) = sample_res {
                        res.key_expr = key_expr;
                    } else {
                        // Handle the error case
                    }

                    let ke = sample_res.clone().unwrap().key_expr;

                    let sample = match &sample_res {
                        Ok(sample) => sample,
                        Err(e) => {
                            println!("No sample returned: {:?}", e);
                            return;
                        }
                    };

                    let Some(reply_attachment) = &sample.attachment else {
                        println!("Unable to retrieve reply attachment");
                        return;
                    };

                    let reply_attachment_clone = reply_attachment.clone();

                    // Send data
                    let result = query // original query from client
                        .reply(sample_res) // sample we got back from the server
                        .with_attachment(reply_attachment_clone);

                    match result {
                        Ok(reply_builder_res) => match reply_builder_res.res().await {
                            Ok(_) => {
                                println!("Got reply");
                                return;
                            }
                            Err(e) => {
                                println!("Error: didn't get reply: {:?}", e);
                                return;
                            }
                        },
                        Err(_) => {
                            println!("Error: Unable to add attachment");
                            return;
                        }
                    }
                }
                Err(e) => {
                    // 4. If the reply was not Ok... then we have to define how we'd return the error we got s.t.
                    //    the client is able to fire off its logic for handling errors. I think the only logic for handling
                    //    errors if something bad happens in an RpcServer is just to print out the below... so...
                    //    for the simple case, I think... we'd just return, meaning we don't respond wihin the TTL, and
                    //    I think the connection will just die and then the below kind of code would get fired under
                    //    invoke_method()
                    // Print out the error
                    println!("Error while receiving Zenoh reply: {:?}", e);
                    return;
                }
            };
        });
    };

    // Declare a Queryable
    let _queryable = session_arc_clone_mainthread
        .declare_queryable("**")
        .callback_mut(get_callback)
        .res()
        .await;

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
