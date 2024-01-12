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

use example_proto::proto::example::hello_world::v1::*;
use example_proto::proto::google::r#type::*;

#[async_std::main]
async fn main() {
    // Your example code goes here
    println!("This is an example sender for uStreamer.");

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

    let mut second_timer = Timer {
        time: Some(TimeOfDay {
            hours: 10,
            minutes: 44,
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
        println!("Attempting send of timer_second...");
        if let Some(ref mut time) = second_timer.time {
            println!(
                "Time: H: {} M: {} S: {} NS: {}",
                time.hours, time.minutes, time.seconds, time.nanos
            );
        }

        // Create a buffer to hold the serialized data
        let mut second_timer_buf = Vec::new();

        // Serialize the struct into the buffer
        second_timer
            .encode(&mut second_timer_buf)
            .expect("Failed to encode");

        let payload = UPayload {
            length: Some(second_timer_buf.len() as i32),
            format: 0,
            data: Some(u_payload::Data::Value(second_timer_buf)),
        };

        match ulink
            .send(
                timer_second_uuri.clone(),
                payload.clone(),
                attributes.clone(),
            )
            .await
        {
            Ok(_) => {
                println!("Sending timer_second succeeded")
            }
            Err(status) => {
                println!("Seconding timer_second failed: {:?}", status)
            }
        }

        if let Some(ref mut time) = second_timer.time {
            time.seconds += 1;
        }
    }
}
