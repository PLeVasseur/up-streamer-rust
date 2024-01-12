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
extern crate uprotocol_sdk;
extern crate uprotocol_zenoh_rust;

use async_std::task;
use prost::Message;
use std::time::Duration;
use uprotocol_sdk::{
    transport::datamodel::UTransport,
    uprotocol::{Data, UEntity, UMessage, UResource, UStatus, UUri},
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::Config;

use example_proto::proto::example::hello_world::v1::*;

fn timer_listener(result: Result<UMessage, UStatus>) {
    match result {
        Ok(message) => {
            let payload = match message.payload {
                Some(payload) => payload,
                None => {
                    println!("No payload attached!");
                    return;
                }
            };

            let data = match payload.data {
                Some(data) => data,
                None => {
                    println!("Empty data payload!");
                    return;
                }
            };

            if let Data::Value(buf) = data {
                let timer = match Timer::decode(&*buf) {
                    Ok(timer) => timer,
                    Err(_) => {
                        println!("Failed to decode Timer!");
                        return;
                    }
                };

                if let Some(time) = timer.time {
                    println!(
                        "Time: H: {} M: {} S: {} NS: {}",
                        time.hours, time.minutes, time.seconds, time.nanos
                    );
                }
            }
        }
        Err(status) => {
            println!(
                "second_timer_listener returned UStatus: {:?}",
                status.get_code()
            );
        }
    }
}

#[async_std::main]
async fn main() {
    // Your example code goes here
    println!("This is an example client for uStreamer.");

    let ulink = ULinkZenoh::new(Config::default()).await.unwrap();
    let timer_hour_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "timer_service".to_string(),
            id: Option::Some(123),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(UResource {
            name: "timer".to_string(),
            instance: None,
            message: Some("hour".to_string()),
            id: Some(1),
        }),
    };
    let timer_minute_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "timer_service".to_string(),
            id: Option::Some(123),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(UResource {
            name: "timer".to_string(),
            instance: None,
            message: Some("minute".to_string()),
            id: Some(2),
        }),
    };
    let timer_second_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "timer_service".to_string(),
            id: Option::Some(123),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(UResource {
            name: "timer".to_string(),
            instance: None,
            message: Some("second".to_string()),
            id: Some(3),
        }),
    };
    let timer_nanosecond_uuri = UUri {
        authority: None,
        entity: Option::from(UEntity {
            name: "timer_service".to_string(),
            id: Option::Some(123),
            version_major: Some(1),
            version_minor: None,
        }),
        resource: Option::from(UResource {
            name: "timer".to_string(),
            instance: None,
            message: Some("nanosecond".to_string()),
            id: Some(4),
        }),
    };

    // You might normally keep track of the registered listener's key so you can remove it later
    let _registered_hour_timer_key = {
        match ulink
            .register_listener(timer_hour_uuri, Box::new(timer_listener))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                println!(
                    "Failed to register timer_hour listener: {:?}",
                    status.get_code()
                );
                return;
            }
        }
    };
    // You might normally keep track of the registered listener's key so you can remove it later
    let _registered_minute_timer_key = {
        match ulink
            .register_listener(timer_minute_uuri, Box::new(timer_listener))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                println!(
                    "Failed to register timer_minute listener: {:?}",
                    status.get_code()
                );
                return;
            }
        }
    };
    // You might normally keep track of the registered listener's key so you can remove it later
    let _registered_second_timer_key = {
        match ulink
            .register_listener(timer_second_uuri, Box::new(timer_listener))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                println!(
                    "Failed to register timer_second listener: {:?}",
                    status.get_code()
                );
                return;
            }
        }
    };
    // You might normally keep track of the registered listener's key so you can remove it later
    let _registered_second_timer_key = {
        match ulink
            .register_listener(timer_nanosecond_uuri, Box::new(timer_listener))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                println!(
                    "Failed to register timer_nanosecond listener: {:?}",
                    status.get_code()
                );
                return;
            }
        }
    };

    // Infinite loop in main thread, just letting the listeners listen
    loop {
        task::sleep(Duration::from_secs(10)).await;
    }
}
