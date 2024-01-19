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
use log::{debug, error, info, trace};
use prost::Message;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{
    Data, Remote, UAttributes, UAuthority, UEntity, UMessage, UMessageType, UPayload,
    UPayloadFormat, UStatus, UUri, Uuid,
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::prelude::r#async::AsyncResolve;
use zenoh::prelude::*;
use zenoh::queryable::Query;
use zenoh::sample::AttachmentBuilder;

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("Starting uStreamer!");

    let uapp_ip: Vec<u8> = vec![192, 168, 3, 100];
    let mdevice_ip: Vec<u8> = vec![192, 168, 3, 1];

    let zenoh_queries = Arc::new(Mutex::new(HashMap::<String, (KeyExpr, Query)>::new()));

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

    let ulink_zenoh = ULinkZenoh::new_from_config(ulink_zenoh_config.clone())
        .await
        .unwrap();
    let utransport_sommr = UTransportSommr::new_from_config(sommr_zenoh_config.clone())
        .await
        .unwrap();

    let uuri_for_all_remote = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Name("*".to_string())),
        }),
        entity: Some(UEntity {
            name: "*".to_string(),
            id: None,
            version_major: None,
            version_minor: None,
        }),
        resource: None,
    };

    let ulink_zenoh_arc = Arc::new(ulink_zenoh);
    let utransport_sommr_arc = Arc::new(utransport_sommr);
    let zenoh_queries_sommr_callback = zenoh_queries.clone();

    let uapp_ip_clone = uapp_ip.clone(); // Clone before the first closure
    let utransport_sommr_zenoh_callback = utransport_sommr_arc.clone();

    let zenoh_callback = move |result: Result<UMessage, UStatus>| {
        trace!("entered zenoh_callback");

        let utransport_sommr_clone = utransport_sommr_zenoh_callback.clone();

        let Ok(msg) = result else {
            error!("no msg");
            return;
        };

        trace!("zenoh_callback: got msg");

        let Some(source) = msg.source else {
            error!("no source");
            return;
        };

        trace!("zenoh_callback: got source");

        let Some(payload) = msg.payload else {
            error!("no payload");
            return;
        };

        trace!("zenoh_callback: got payload");

        let Some(attributes) = msg.attributes else {
            error!("no attributes");
            return;
        };

        trace!("zenoh_callback: got attributes");

        debug!("attributes: {:?}", &attributes);

        // Check the message type (Publish/Request/Response)
        // ASSUMPTION: By registering an "all remote" listener, we get only those messages which have UAuthority
        match UMessageType::try_from(attributes.r#type) {
            Ok(UMessageType::UmessageTypePublish) => {
                let block_msg_tx = if let Some(authority) = &source.authority {
                    if let Some(Remote::Ip(ip)) = &authority.remote {
                        debug!("ip: {:?}", &ip);
                        *ip != uapp_ip_clone
                    } else {
                        true
                    }
                } else {
                    true
                };

                debug!("block_msg_tx: {}", &block_msg_tx);

                if block_msg_tx {
                    info!("Message not for send from us, skipping");
                    return;
                }
                info!("zenoh_callback: Source: {}", &source);

                task::spawn(async move {
                    match utransport_sommr_clone
                        .send(source, payload, attributes)
                        .await
                    {
                        Ok(_) => {
                            info!("zenoh_callback: Forwarding message over sommr succeeded");
                        }
                        Err(status) => {
                            error!(
                                "zenoh_callback: Forwarding message over sommr failed: {:?}",
                                status
                            )
                        }
                    }

                    trace!("zenoh_callback: utransport_sommr_clone.send() within async");
                });
                trace!("zenoh_callback: after utransport_clone.send()");
            }
            Ok(UMessageType::UmessageTypeResponse) => {
                // trace!("got response back");
                //
                // // Look up the Zenoh reply using reqid from the message's attributes
                // if let Some(reqid) = attributes.reqid.as_ref() {
                //     if let Some((key_expr, query)) = zenoh_queries_sommr_callback
                //         .lock()
                //         .unwrap()
                //         .remove(&String::from(reqid))
                //     {
                //         // Use the reply to respond to the original Zenoh query
                //         // (You'll need to adjust this according to your application's logic)
                //         trace!(
                //             "for reqid: {} we had query: {:?}",
                //             <&Uuid as Into<String>>::into(reqid),
                //             &query
                //         );
                //
                //         let Some(Data::Value(buf)) = payload.data else {
                //             error!("Invalid data");
                //             return;
                //         };
                //
                //         let mut attr = vec![];
                //         let Ok(()) = attributes.encode(&mut attr) else {
                //             error!("Unable to encode UAttributes");
                //             return;
                //         };
                //
                //         // Add attachment and payload
                //         let mut attachment = AttachmentBuilder::new();
                //         attachment.insert("uattributes", attr.as_slice());
                //         // Send back query
                //         let value = Value::new(buf.into()).encoding(Encoding::WithSuffix(
                //             KnownEncoding::AppCustom,
                //             payload.format.to_string().into(),
                //         ));
                //         let reply = Ok(Sample::new(key_expr, value));
                //
                //         task::spawn(async move {
                //             let Ok(reply_builder) =
                //                 query.reply(reply).with_attachment(attachment.build())
                //                 else {
                //                     error!("Error: Unable to add attachment");
                //                     return;
                //                 };
                //
                //             if let Err(e) = reply_builder.res().await {
                //                 error!("Error: Unable to reply with Zenoh - {:?}", e);
                //                 return;
                //             }
                //
                //             trace!("Replied via Zenoh!");
                //         });
                //     }
                // } else {
                //     error!("Attributes lack reqid");
                // }
            }
            _ => {
                debug!("UMessageType is not UmessageTypeRequest");
            }
        }
    };

    info!("Register the listener for publishes to remote...");
    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let _registered_all_remote_zenoh_key = {
        match ulink_zenoh_arc
            .register_listener(uuri_for_all_remote.clone(), Box::new(zenoh_callback))
            .await
        {
            Ok(registered_key) => registered_key,
            Err(status) => {
                error!(
                    "Failed to register zenoh remote listener callback: {:?} {}",
                    status.get_code(),
                    status.message()
                );
                return;
            }
        }
    };

    let sommr_callback = move |result: Result<UMessage, UStatus>| {
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

        debug!("sommr attributes: {:?}", &attributes);

        // Check the message type (Publish/Request/Response)
        // ASSUMPTION: By registering an "all remote" listener, we get only those messages which have UAuthority
        match UMessageType::try_from(attributes.r#type) {
            Ok(UMessageType::UmessageTypePublish) => {
                let block_msg_rx = if let Some(authority) = &source.authority {
                    if let Some(Remote::Ip(ip)) = &authority.remote {
                        debug!("ip: {:?}", &ip);
                        *ip == uapp_ip.clone()
                    } else {
                        true
                    }
                } else {
                    true
                };

                debug!("block_msg_rx: {}", &block_msg_rx);

                if block_msg_rx {
                    info!("Message not intended for us, skipping");
                    return;
                }
                info!("sommr_callback: Source: {}", &source);

                let ulink_zenoh_clone = ulink_zenoh_arc.clone();
                task::spawn(async move {
                    match ulink_zenoh_clone.send(source, payload, attributes).await {
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
            }
            Ok(UMessageType::UmessageTypeResponse) => {
                trace!("got response back");

                // Look up the Zenoh reply using reqid from the message's attributes
                if let Some(reqid) = attributes.reqid.as_ref() {
                    if let Some((key_expr, query)) = zenoh_queries_sommr_callback
                        .lock()
                        .unwrap()
                        .remove(&String::from(reqid))
                    {
                        // Use the reply to respond to the original Zenoh query
                        // (You'll need to adjust this according to your application's logic)
                        trace!(
                            "for reqid: {} we had query: {:?}",
                            <&Uuid as Into<String>>::into(reqid),
                            &query
                        );

                        let Some(Data::Value(buf)) = payload.data else {
                            error!("Invalid data");
                            return;
                        };

                        let mut attr = vec![];
                        let Ok(()) = attributes.encode(&mut attr) else {
                            error!("Unable to encode UAttributes");
                            return;
                        };

                        // Add attachment and payload
                        let mut attachment = AttachmentBuilder::new();
                        attachment.insert("uattributes", attr.as_slice());
                        // Send back query
                        let value = Value::new(buf.into()).encoding(Encoding::WithSuffix(
                            KnownEncoding::AppCustom,
                            payload.format.to_string().into(),
                        ));
                        let reply = Ok(Sample::new(key_expr, value));

                        task::spawn(async move {
                            let Ok(reply_builder) =
                                query.reply(reply).with_attachment(attachment.build())
                            else {
                                error!("Error: Unable to add attachment");
                                return;
                            };

                            if let Err(e) = reply_builder.res().await {
                                error!("Error: Unable to reply with Zenoh - {:?}", e);
                                return;
                            }

                            trace!("Replied via Zenoh!");
                        });
                    }
                } else {
                    error!("Attributes lack reqid");
                }
            }
            _ => {
                debug!("UMessageType is not UmessageTypeRequest");
            }
        }
    };

    // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
    let _registered_all_remote_sommr_key = {
        match utransport_sommr_arc
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
    let zenoh_queries_get_callback = zenoh_queries.clone();

    let get_callback = move |query: Query| {
        let utransport_sommr_arc = utransport_sommr_arc.clone();
        let key_expr = query.key_expr().clone();

        // Check if the key expression starts with "@"
        if key_expr.starts_with('@') {
            info!("Ignoring message with key expression: '{}'", key_expr);
            return;
        }

        let Some(value) = query.value() else {
            error!("query lacked value: {}", query.key_expr());
            return;
        };

        let Some(attachment) = query.attachment() else {
            error!("query lacked appropriate attachment: {}", query.key_expr());
            return;
        };
        let Some(attribute) = attachment.get(&"uattributes".as_bytes()) else {
            error!("Unable to get uattributes");
            return;
        };
        let u_attribute: UAttributes = if let Ok(attr) = Message::decode(&*attribute) {
            attr
        } else {
            error!("Unable to decode attribute");
            return;
        };

        debug!("up-client-zenoh-rust: attributes: {:?}", &u_attribute);

        // Check the type of UAttributes (Request)
        match UMessageType::try_from(u_attribute.r#type) {
            Ok(UMessageType::UmessageTypeRequest) => {}
            _ => {
                debug!("UMessageType is not UmessageTypeRequest");
                return;
            }
        }

        // Create UPayload
        let u_payload = match query.value() {
            Some(value) => {
                let Ok(encoding) = value.encoding.suffix().parse::<i32>() else {
                    error!("Unable to get payload encoding");
                    return;
                };
                UPayload {
                    length: Some(0),
                    format: encoding,
                    data: Some(Data::Value(value.payload.contiguous().to_vec())),
                }
            }
            None => UPayload {
                length: Some(0),
                format: UPayloadFormat::UpayloadFormatUnspecified as i32,
                data: None,
            },
        };

        let destination = if let Some(sink) = u_attribute.sink.clone() {
            sink
        } else {
            error!("Unable to get destination from UAttributes.sink");
            return;
        };

        // Extract the id from u_attribute and use it as the key for the HashMap
        if let Some(id) = u_attribute.id.as_ref() {
            zenoh_queries_get_callback
                .lock()
                .unwrap()
                .insert(id.clone().into(), (key_expr.clone(), query.clone()));
        } else {
            error!("u_attribute lacks id");
            return;
        }

        debug!(
            "Received on '{}': '{:?}': destination: {:?}",
            &key_expr,
            &value,
            &destination.clone()
        );
        trace!("UAttributes: {:?}", &u_attribute);

        task::spawn(async move {
            // TODO: I think we want to register a listener for what's coming back over the sommR interface here
            //  and then hang out / wait till we get the topic we're expecting
            //  and then at that point return the result over Zenoh

            trace!("destination: {:?}", &destination);

            match utransport_sommr_arc
                .send(destination.clone(), u_payload, u_attribute)
                .await
            {
                Ok(_) => {
                    info!("Forwarding RPC Request over sommR succeeded");
                }
                Err(status) => {
                    error!("Forwarding RPC Request over sommR failed: {:?}", status)
                }
            }
        });
    };

    // Declare a Queryable
    let _queryable = session_arc_clone_mainthread
        .declare_queryable("up/**")
        .callback_mut(get_callback)
        .res()
        .await;

    // Infinite loop in main thread, just letting the uStreamer listen and retransmit
    loop {
        task::sleep(Duration::from_millis(1000)).await;
        // async_std::future::pending::<()>().await;
    }
}
