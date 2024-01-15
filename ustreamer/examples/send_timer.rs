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

use async_std::task::{self};
use prost::Message;
use std::time::Duration;
use uprotocol_sdk::uprotocol::{u_payload, UAttributes};
use uprotocol_sdk::{
    transport::datamodel::UTransport,
    uprotocol::{UEntity, UMessageType, UPayload, UResource, UUri},
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::Config;
use zenoh::prelude::WhatAmI;

use example_proto::proto::example::hello_world::v1::*;
use example_proto::proto::google::r#type::*;

fn print_time(timer: &Timer) {
    if let Some(ref time) = timer.time {
        println!(
            "Timer: H: {} M: {} S: {} NS: {}",
            time.hours, time.minutes, time.seconds, time.nanos
        );
    }
}

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();
    // Your example code goes here
    println!("This is an example sender for uStreamer.");

    // let locator = vec![String::from("tcp/127.0.0.1:17449")];

    let mut config = Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Setting as Peer failed");
    // config
    //     .connect
    //     .set_endpoints(locator.iter().map(|x| x.parse().unwrap()).collect())
    //     .unwrap();
    let ulink = ULinkZenoh::new(config).await.unwrap();
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

    let mut hour_timer = Timer {
        time: Some(TimeOfDay {
            hours: 0,
            minutes: 0,
            seconds: 0,
            nanos: 0,
        }),
    };

    let mut minute_timer = Timer {
        time: Some(TimeOfDay {
            hours: 0,
            minutes: 0,
            seconds: 0,
            nanos: 0,
        }),
    };

    let mut second_timer = Timer {
        time: Some(TimeOfDay {
            hours: 0,
            minutes: 0,
            seconds: 0,
            nanos: 0,
        }),
    };

    let mut nanosecond_timer = Timer {
        time: Some(TimeOfDay {
            hours: 0,
            minutes: 0,
            seconds: 0,
            nanos: 0,
        }),
    };

    let attributes = UAttributes {
        id: None,
        r#type: i32::from(UMessageType::UmessageTypePublish),
        sink: None,
        priority: 0,
        ttl: None,
        permission_level: None,
        commstatus: None,
        reqid: None,
        token: None,
    };

    loop {
        task::sleep(Duration::from_secs(1)).await;

        println!("Attempting send of timers...");

        // ---- hour_timer ----
        let mut hour_timer_buf = Vec::new();
        hour_timer
            .encode(&mut hour_timer_buf)
            .expect("Failed to encode");

        let hour_timer_payload = UPayload {
            length: Some(hour_timer_buf.len() as i32),
            format: 0,
            data: Some(u_payload::Data::Value(hour_timer_buf)),
        };

        match ulink
            .send(
                timer_hour_uuri.clone(),
                hour_timer_payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(_) => {
                println!("Sending timer_hour succeeded");
                print_time(&hour_timer);
            }
            Err(status) => {
                println!("Seconding timer_hour failed: {:?}", status)
            }
        }

        if let Some(ref mut time) = hour_timer.time {
            time.hours += 1;
        }

        // ---- minute_timer ----
        let mut minute_timer_buf = Vec::new();
        minute_timer
            .encode(&mut minute_timer_buf)
            .expect("Failed to encode");

        let minute_timer_payload = UPayload {
            length: Some(minute_timer_buf.len() as i32),
            format: 0,
            data: Some(u_payload::Data::Value(minute_timer_buf)),
        };

        match ulink
            .send(
                timer_minute_uuri.clone(),
                minute_timer_payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(_) => {
                println!("Sending timer_minute succeeded");
                print_time(&minute_timer);
            }
            Err(status) => {
                println!("Seconding timer_minute failed: {:?}", status)
            }
        }

        if let Some(ref mut time) = minute_timer.time {
            time.minutes += 1;
        }

        // ---- second_timer ----
        let mut second_timer_buf = Vec::new();
        second_timer
            .encode(&mut second_timer_buf)
            .expect("Failed to encode");

        let second_timer_payload = UPayload {
            length: Some(second_timer_buf.len() as i32),
            format: 0,
            data: Some(u_payload::Data::Value(second_timer_buf)),
        };

        match ulink
            .send(
                timer_second_uuri.clone(),
                second_timer_payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(_) => {
                println!("Sending timer_second succeeded");
                print_time(&second_timer);
            }
            Err(status) => {
                println!("Seconding timer_second failed: {:?}", status)
            }
        }

        if let Some(ref mut time) = second_timer.time {
            time.seconds += 1;
        }

        // ---- nanosecond_timer ----
        let mut nanosecond_timer_buf = Vec::new();
        nanosecond_timer
            .encode(&mut nanosecond_timer_buf)
            .expect("Failed to encode");

        let nanosecond_timer_payload = UPayload {
            length: Some(nanosecond_timer_buf.len() as i32),
            format: 0,
            data: Some(u_payload::Data::Value(nanosecond_timer_buf)),
        };

        match ulink
            .send(
                timer_nanosecond_uuri.clone(),
                nanosecond_timer_payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(_) => {
                println!("Sending timer_nanosecond succeeded");
                print_time(&nanosecond_timer);
            }
            Err(status) => {
                println!("Seconding timer_nanosecond failed: {:?}", status)
            }
        }

        if let Some(ref mut time) = nanosecond_timer.time {
            time.nanos += 1;
        }
    }
}
