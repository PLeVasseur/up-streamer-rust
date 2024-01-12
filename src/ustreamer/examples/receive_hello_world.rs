extern crate example_proto;
extern crate uprotocol_sdk;
extern crate uprotocol_zenoh_rust;

use async_std::task::{self, block_on};
use prost::Message;
use std::sync::{Arc, Mutex};
use std::time;
use uprotocol_sdk::{
    rpc::{RpcClient, RpcServer},
    transport::builder::UAttributesBuilder,
    transport::datamodel::UTransport,
    uprotocol::{
        Data, UCode, UEntity, UMessage, UMessageType, UPayload, UPayloadFormat, UPriority,
        UResource, UStatus, UUri, Uuid,
    },
    uri::builder::resourcebuilder::UResourceBuilder,
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::Config;

use example_proto::proto::example::hello_world::v1::*;
use example_proto::proto::google::r#type::*;

fn second_timer_listener(result: Result<UMessage, UStatus>) {
    match result {
        Ok(message) => {
            println!("second_timer_listener returned UMessage");

            let payload = match message.payload {
                Some(payload) => payload,
                None => return,
            };

            let data = match payload.data {
                Some(data) => data,
                None => return,
            };

            if let Data::Value(buf) = data {
                println!("We got some data");

                let timer = match Timer::decode(&*buf) {
                    Ok(timer) => timer,
                    Err(_) => return,
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

// fn second_timer_listener(result: Result<UMessage, UStatus>) {
//     match result {
//         Ok(message) => {
//             println!("second_timer_listener returned UMessage");
//
//             if let Some(payload) = message.payload {
//                 if let Some(data) = payload.data {
//                     if let Data::Value(buf) = data {
//                         println!("We got some data");
//
//                         if let Ok(timer) = Timer::decode(&*buf) {
//
//                             if let Some(time) = timer.time {
//                                 println!("Time: H: {} M: {} S: {} NS: {}", time.hours, time.minutes, time.seconds, time.nanos);
//                             }
//
//                         }
//                     }
//                 }
//             }
//         }
//         Err(status) => {
//             println!(
//                 "second_timer_listener returned UStatus: {:?}",
//                 status.get_code()
//             );
//         }
//     }
// }

#[async_std::main]
async fn main() {
    // Your example code goes here
    println!("This is an example client for uStreamer.");

    let ulink = ULinkZenoh::new(Config::default()).await.unwrap();
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
            id: Some(1),
        }),
    };

    let registered_second_timer_key = {
        match ulink
            .register_listener(timer_second_uuri, Box::new(second_timer_listener))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                println!(
                    "Failed to register second_timer_listener: {:?}",
                    status.get_code()
                );
                return;
            }
        }
    };

    // let _to: Timer = Timer { time: None };

    loop {}
}
