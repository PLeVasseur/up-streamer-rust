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
use zenoh::queryable::Query;

// macro_rules! insert_json5 {
//     ($config: expr, $args: expr, $key: expr, if $name: expr) => {
//         if $args.occurrences_of($name) > 0 {
//             $config.insert_json5($key, "true").unwrap();
//         }
//     };
//     ($config: expr, $args: expr, $key: expr, if $name: expr, $($t: tt)*) => {
//         if $args.occurrences_of($name) > 0 {
//             $config.insert_json5($key, &serde_json::to_string(&$args.value_of($name).unwrap()$($t)*).unwrap()).unwrap();
//         }
//     };
//     ($config: expr, $args: expr, $key: expr, for $name: expr, $($t: tt)*) => {
//         if let Some(value) = $args.values_of($name) {
//             $config.insert_json5($key, &serde_json::to_string(&value$($t)*).unwrap()).unwrap();
//         }
//     };
// }
//
// #[async_std::main]
// async fn main() {
//     env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("z=info")).init();
//     // log::info!("zenoh-bridge-dds {}", *zenoh_plugin_dds::LONG_VERSION);
//
//     // let (config) = parse_args();
//     // let zenoh_transport_router_plugin = config.plugin("rest").is_some();
//     let zenoh_transport_router_plugin = true;
//
//     let mut config = zenoh::config::Config::default();
//     config
//         .set_mode(Some(WhatAmI::Peer))
//         .expect("Unable to configure as Peer");
//
//     // if "zenoh_transport_router" plugin conf is not present, add it (empty to use default config)
//     if config.plugin("zenoh_transport_router").is_none() {
//         config
//             .insert_json5("plugins/zenoh_transport_router", "{}")
//             .unwrap();
//     }
//
//     // create a zenoh Runtime (to share with router-plugins)
//     let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();
//
//     // start transport-router-zenoh-plugin plugin
//     if zenoh_transport_router_plugin {
//         use zenoh_plugin_trait::Plugin;
//         zenoh_transport_router::ZenohTransportRouter::start("zenoh_transport_router", &runtime)
//             .unwrap();
//     }
//
//     async_std::future::pending::<()>().await;
// }

// Temporarily comment out -- try to bring in the plugin
#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("Hello, uStreamer!");

    // Initialize the Zenoh runtime
    // let locator = vec![String::from("tcp/127.0.0.1:17449")];

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Router");
    let Ok(session) = zenoh::open(config.clone()).res().await else {
        println!("Failed to open Zenoh Router session");
        return;
    };
    let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();

    let ulink_zenoh = ULinkZenoh::new_from_runtime(runtime.clone()).await.unwrap();
    let utransport_sommr = UTransportSommr::new_from_runtime(runtime.clone()).await.unwrap();

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
    let _session_arc_clone_subscriber_callback = session_arc.clone();
    let _session_arc_clone_queryable_callback = session_arc.clone();
    let session_arc_clone_mainthread = session_arc.clone();

    // Define a callback function to process incoming messages
    let zenoh_sub_callback = move |sample: Sample| {
        println!("Zenoh up/ subscriber callback");

        let key_expr = sample.key_expr.clone();
        let payload = sample.value.payload.clone();

        // // Check if the key expression starts with "@"
        // if key_expr.starts_with('@') {
        //     println!("Ignoring message with key expression: '{}'", key_expr);
        //     return; // Skip processing this message
        // }
        //
        // println!("zenoh_sub_callback: after key_expr @ check");
        //
        // // TODO: Need to check this will still work after the move to micro form
        // //  Perhaps they'll just append all the numbers together with some . or /
        // //  So my guess is it'd be best to just add a number here, let's say 535
        // //  --This mechanism is only needed now because we're listening in on and transmitting
        // //  over the same transport and can be removed when we're retransmitting over SOME/IP
        // // if key_expr.ends_with("535") {
        // //     println!("Ignoring message with key expression: '{}'", key_expr);
        // //     return; // Skip processing this message
        // // }
        //
        // let Some(attachment) = sample.attachment() else {
        //     println!(
        //         "Message missing attachment, skip key expression: '{}'",
        //         key_expr
        //     );
        //     return;
        // };
        //
        // println!("zenoh_sub_callback: got attachment");
        //
        // let attachment_clone = attachment.clone();

        println!("Received on '{}': '{:?}'", &key_expr, &payload);

        // let session_clone = session_arc_clone_subscriber_callback.clone();
        //
        // // let retransmit_key_expr = key_expr.concat("535").expect("unable to append retransmit");
        //
        // let Ok(encoding) = sample.encoding.suffix().parse::<i32>() else {
        //     println!("Unable to get encoding for key expression: '{}'", &key_expr);
        //     return;
        // };
        //
        // println!("zenoh_sub_callback: got encoding");
        //
        // task::spawn(async move {
        //     let session_clone = session_clone.clone();
        //     let putbuilder = session_clone
        //         .put(key_expr, payload)
        //         .encoding(Encoding::WithSuffix(
        //             KnownEncoding::AppCustom,
        //             encoding.to_string().into(),
        //         ))
        //         .with_attachment(attachment_clone);
        //
        //     if let Err(e) = putbuilder.res().await {
        //         eprintln!("Failed to send message: {:?}", e);
        //     }
        //     println!("zenoh_sub_callback: sent via Zenoh inside async");
        // });
        //
        // println!("zenoh_sub_callback: sent via Zenoh");
    };

    // let get_callback = move |query: Query| {
    //     let key_expr = query.key_expr().clone();
    //
    //     // Check if the key expression starts with "@"
    //     if key_expr.starts_with('@') {
    //         println!("Ignoring message with key expression: '{}'", key_expr);
    //         return;
    //     }
    //
    //     let Some(value) = query.value() else {
    //         println!("query lacked value: {}", query.key_expr());
    //         return;
    //     };
    //
    //     let value_clone = value.clone();
    //
    //     let Some(attachment) = query.attachment() else {
    //         println!("query lacked appropriate attachment: {}", query.key_expr());
    //         return;
    //     };
    //
    //     if key_expr.ends_with("123") {
    //         println!("Ignoring query with key expression: '{}'", key_expr);
    //         return; // Skip processing this message as it's a retransmit
    //     }
    //
    //     let attachment_clone = attachment.clone();
    //
    //     println!("Received on '{}': '{:?}'", &key_expr, &value);
    //
    //     let session_clone = session_arc_clone_queryable_callback.clone();
    //
    //     let retransmit_key_expr = key_expr.concat("123").expect("unable to append retransmit");
    //
    //     task::spawn(async move {
    //         // 1. Extract the relevant info from the query needed for the GetBuilder:
    //         let getbuilder = session_clone
    //             .get(&retransmit_key_expr)
    //             .with_value(value_clone)
    //             .with_attachment(attachment_clone)
    //             .target(QueryTarget::BestMatching)
    //             .timeout(Duration::from_millis(1000));
    //
    //         // 2. Forward the query on
    //         let Ok(replies) = getbuilder.res().await else {
    //             println!("Error while sending Zenoh query");
    //             return;
    //         };
    //
    //         // 3. If the reply was Ok, then we can reply to to the original query
    //         // TODO: Because of this back and forth, had to increase the timeout over on invoke_method
    //         match replies.recv_async().await {
    //             Ok(reply) => {
    //                 println!("Got reply back from server");
    //
    //                 let mut sample_res = reply.sample;
    //
    //                 if let Ok(ref mut res) = sample_res {
    //                     res.key_expr = key_expr;
    //                 } else {
    //                     // Handle the error case
    //                 }
    //
    //                 let sample = match &sample_res {
    //                     Ok(sample) => sample,
    //                     Err(e) => {
    //                         println!("No sample returned: {:?}", e);
    //                         return;
    //                     }
    //                 };
    //
    //                 let Some(reply_attachment) = &sample.attachment else {
    //                     println!("Unable to retrieve reply attachment");
    //                     return;
    //                 };
    //
    //                 let reply_attachment_clone = reply_attachment.clone();
    //
    //                 // Send data
    //                 let result = query // original query from client
    //                     .reply(sample_res) // sample we got back from the server
    //                     .with_attachment(reply_attachment_clone);
    //
    //                 match result {
    //                     Ok(reply_builder_res) => match reply_builder_res.res().await {
    //                         Ok(_) => {
    //                             println!("Got reply");
    //                         }
    //                         Err(e) => {
    //                             println!("Error: didn't get reply: {:?}", e);
    //                         }
    //                     },
    //                     Err(_) => {
    //                         println!("Error: Unable to add attachment");
    //                     }
    //                 }
    //             }
    //             Err(e) => {
    //                 // 4. If the reply was not Ok... then just print an error and return
    //                 println!("Error while receiving Zenoh reply: {:?}", e);
    //             }
    //         };
    //     });
    // };

    // Declare a Queryable
    // let _queryable = session_arc_clone_mainthread
    //     .declare_queryable("**")
    //     // .declare_queryable("up/**")
    //     .callback_mut(get_callback)
    //     .res()
    //     .await;

    // Attach the callback function to a subscriber that listens to all paths
    let _subscriber = session_arc_clone_mainthread
        .declare_subscriber("**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
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
