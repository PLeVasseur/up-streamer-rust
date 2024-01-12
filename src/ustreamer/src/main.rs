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

use zenoh::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use async_std::channel::unbounded;
use async_std::task;
use uprotocol_sdk::uprotocol::{UCode, UStatus};
use zenoh::prelude::r#async::AsyncResolve;

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

    // Define a callback function to process incoming messages
    let callback = move |sample: Sample| {
        println!("Received on '{}': '{:?}'", sample.key_expr, sample.value.payload);
        // Add more logic here to process the message
    };

    // Attach the callback function to a subscriber that listens to all paths
    let _subscriber = session_arc
        .declare_subscriber("**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
        .callback_mut(callback)
        .res()
        .await;

    // Infinite loop in main thread, just letting the listeners listen
    loop {
        task::sleep(Duration::from_secs(10)).await;
    }
}

// DO NOT DELETE THIS GUY -- at the very least it will listen correctly :)
// #[async_std::main]
// async fn main() {
//     println!("Hello, uStreamer!");
//
//     // Initialize the Zenoh runtime
//     let locator = vec![String::from("tcp/127.0.0.1:17449")];
//
//     let mut config = zenoh::config::Config::default();
//     config
//         .set_mode(Some(WhatAmI::Router))
//         .expect("Unable to configure as Router");
//     config
//         .listen
//         .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
//         .unwrap();
//     let Ok(session) = zenoh::open(config).res().await else {
//         println!("Failed to open Zenoh Router session");
//         return;
//     };
//
//     let session_arc = Arc::new(session);
//
//     // Define a callback function to process incoming messages
//     let callback = move |sample: Sample| {
//         println!("Received on '{}': '{:?}'", sample.key_expr, sample.value.payload);
//         // Add more logic here to process the message
//     };
//
//     // Attach the callback function to a subscriber that listens to all paths
//     let _subscriber = session_arc
//         .declare_subscriber("**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
//         .callback_mut(callback)
//         .res()
//         .await;
//
//     // Infinite loop in main thread, just letting the listeners listen
//     loop {
//         task::sleep(Duration::from_secs(10)).await;
//     }
// }
// DO NOT DELETE THIS GUY -- at the very least it will listen correctly :)

// #[async_std::main]
// async fn main() {
//     println!("Hello, uStreamer!");
//
//     // Initialize the Zenoh runtime
//     let locator = vec![String::from("tcp/127.0.0.1:17449")];
//     let mut config = zenoh::config::Config::default();
//     config.set_mode(Some(WhatAmI::Router)).expect("Unable to configure as Router");
//     config.listen.set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect()).unwrap();
//
//     let session = zenoh::open(config).res().await.expect("Failed to open Zenoh Router session");
//
//     // Wrap session in Arc<Mutex<>> for shared ownership
//     let session_arc = Arc::new(session);
//
//     // Clone `session_arc` for use inside the closure
//     let session_arc_clone = Arc::clone(&session_arc);
//
//     // Define a callback to re-transmit incoming messages
//     let callback = move |sample: Sample| {
//         println!("Received on '{}': '{:?}'", sample.key_expr, sample.value.payload);
//
//         // let putbuilder = session_arc_clone
//         //     .put(sample.key_expr, sample.value.payload);
//         //
//         // task::block_on(async {
//         //     putbuilder
//         //         .res()
//         //         .await
//         //         .expect("Unable to send");
//         // });
//     };
//
//     // Declare a subscriber with the callback
//     session_arc.declare_subscriber("**").callback_mut(callback).res().await.expect("Failed to declare subscriber");
//
//     // Infinite loop to keep the main thread running
//     loop {
//         task::sleep(Duration::from_secs(10)).await;
//     }
// }

// #[async_std::main]
// async fn main() {
//     println!("Hello, uStreamer!");
//
//     // Initialize the Zenoh runtime
//     let locator = vec![String::from("tcp/127.0.0.1:17449")];
//
//     let mut config = zenoh::config::Config::default();
//     config
//         .set_mode(Some(WhatAmI::Router))
//         .expect("Unable to configure as Router");
//     config
//         .listen
//         .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
//         .unwrap();
//
//     let Ok(session) = zenoh::open(config).res().await else {
//         println!("Failed to open Zenoh Router session");
//         return;
//     };
//
//     // Wrap session in Arc<Mutex<>> for shared ownership
//     let session_arc = Arc::new(Mutex::new(session));
//
//     // Lock session and create publisher
//     let session_lock = session_arc.lock().unwrap();
//     let publisher = session_lock.declare_publisher("**").res().await.unwrap();
//
//     // Create a channel for passing messages from the callback to the async context
//     let (sender, receiver) = unbounded::<Sample>();
//
//     // Define a callback function to process and re-transmit incoming messages
//     let session_clone = Arc::clone(&session_arc);
//     let callback = move |sample: Sample| {
//         println!("Received on '{}': '{:?}'", sample.key_expr, sample.value.payload);
//         sender.try_send(sample).unwrap();
//     };
//
//     // Lock session and create subscriber
//     let session_lock = session_arc.lock().unwrap();
//     let _subscriber = session_lock
//         .declare_subscriber("**")
//         .callback_mut(callback)
//         .res()
//         .await;
//
//     // Async task to handle the retransmission
//     let session_clone_for_async = Arc::clone(&session_arc);
//     task::spawn(async move {
//         while let Ok(sample) = receiver.recv().await {
//             // Clone the key and payload to avoid borrowing issues
//             let key = sample.key_expr.clone();
//             let payload = sample.value.payload.clone();
//
//             // Perform the put operation within a separate scope
//             {
//                 let session_guard = session_clone_for_async.lock().unwrap();
//                 session_guard.put(key, payload).res().await.unwrap();
//             }
//             // session_guard is dropped here, before the next await
//         }
//     });
//
//     // Infinite loop in main thread, just letting the listeners listen
//     loop {
//         task::sleep(Duration::from_secs(10)).await;
//     }
// }

// below one actually appeared to catch messages

// #[async_std::main]
// async fn main() {
//     println!("Hello, uStreamer!");
//
//     // Initialize the Zenoh runtime
//     let locator = vec![String::from("tcp/127.0.0.1:17449")];
//
//     let mut config = zenoh::config::Config::default();
//     config
//         .set_mode(Some(WhatAmI::Router))
//         .expect("Unable to configure as Router");
//     config
//         .listen
//         .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
//         .unwrap();
//     let Ok(session) = zenoh::open(config).res().await else {
//         println!("Failed to open Zenoh Router session");
//         return;
//     };
//
//     // Create a publisher
//     let publisher = session.declare_publisher("**").res().await.unwrap();
//
//     // Create a channel for passing messages from the callback to the async context
//     let (sender, receiver) = unbounded::<Sample>();
//
//     // Define a callback function to process and re-transmit incoming messages
//     let callback = move |sample: Sample| {
//         println!("Received on '{}': '{:?}'", sample.key_expr, sample.value.payload);
//         sender.try_send(sample).unwrap();
//     };
//
//     // Attach the callback function to a subscriber that listens to all paths
//     let _subscriber = session
//         .declare_subscriber("**") // "*" captures one chunk (i.e. section not containing /), "**" captures all chunks
//         .callback_mut(callback)
//         .res()
//         .await;
//
//     // Async task to handle the retransmission
//     task::spawn(async move {
//         while let Ok(sample) = receiver.recv().await {
//             publisher.put(sample.value.payload);
//         }
//     });
//
//     // Infinite loop in main thread, just letting the listeners listen
//     loop {
//         task::sleep(Duration::from_secs(10)).await;
//     }
// }
