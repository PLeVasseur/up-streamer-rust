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

use async_trait::async_trait;
use log::{debug, error, info};
use retransmitter::Retransmitter;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{
    UAttributes, UCode, UMessage, UMessageType, UPayload, UStatus, UUri,
};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;

pub struct RetransmitterZenoh {
    resource_append: Option<u16>, // used when operating in "dummy" mode to append this to the resource
    up_zenoh: ULinkZenoh,
}

impl RetransmitterZenoh {
    pub async fn new(runtime: Runtime, resource_append: Option<u16>) -> RetransmitterZenoh {
        // TODO: Add error handling here and change signature to possibly error
        let up_zenoh = ULinkZenoh::new_from_runtime(runtime).await.unwrap();

        RetransmitterZenoh {
            resource_append,
            up_zenoh: up_zenoh,
        }
    }
}

#[async_trait]
impl Retransmitter for RetransmitterZenoh {
    async fn retransmit(
        &self,
        destination: UUri,
        payload: UPayload,
        attributes: UAttributes,
    ) -> Result<(), UStatus> {
        let key_expr = match ULinkZenoh::to_zenoh_key_string(&destination) {
            Ok(ke) => ke,
            Err(e) => {
                return Err(UStatus::fail_with_code(
                    UCode::Internal,
                    &*format!("Unable to convert UUri to Zenoh key expression: {:?}", e),
                ))
            }
        };

        // Check if we have received a message we already retransmitted
        if let Some(resource_append) = self.resource_append {
            if key_expr.ends_with(&resource_append.to_string()) {
                // TODO: Troubleshoot why e.g. info!() don't appear to be honored from within this file
                println!(
                    "Ignoring already retransmitted message with key expression: '{}'",
                    &key_expr
                );
                return Ok(());
            }
        }

        let retransmit_key_expr = {
            if let Some(resource_append) = self.resource_append {
                key_expr.clone() + &resource_append.to_string()
            } else {
                key_expr.clone()
            }
        };

        // TODO: Implement checking the UAuthority here, when we want to use this with remote uDevices
        // if destination.authority.is_none() {
        //     debug!(
        //         "Only retransmit messages onto other protocols to other uDevices: '{}'",
        //         &key_expr
        //     );
        //     return UStatus::ok();
        // }

        // TODO: Do we need to check this here, if it's enforced at the API level?
        // let Some(payload) = message.payload else {
        //     error!("No payload retrieved from message with key expression: '{}", &key_expr);
        //     return UStatus::fail_with_code(
        //         UCode::InvalidArgument,
        //         &*format!("No payload retrieved from message with key expression: {:?}", key_expr),
        //     );
        // };

        // TODO: Do we need to check this here, if it's enforced at the API level?
        // let Some(attributes) = message.attributes else {
        //     error!("No attributes retrieved from message with key expression: '{}", &key_expr);
        //     return UStatus::fail_with_code(
        //         UCode::InvalidArgument,
        //         &*format!("No attributes retrieved from message with key expression: {:?}", key_expr),
        //     );
        // };

        let mut retransmit_destination = destination;

        if let Some(append) = &self.resource_append {
            let resource = retransmit_destination.resource.as_mut().ok_or_else(|| {
                println!("UResource was none!");
                UStatus::fail_with_code(
                    UCode::Internal,
                    &format!("UResource was none for Zenoh key expression: {}", key_expr),
                )
            })?;

            let message = resource.message.as_mut().ok_or_else(|| {
                println!("UResource.message was none!");
                UStatus::fail_with_code(
                    UCode::Internal,
                    &format!(
                        "UResource.message was none for Zenoh key expression: {}",
                        key_expr
                    ),
                )
            })?;

            message.push_str(&append.to_string());
        }

        let mut retransmit_attributes = attributes;

        if let Some(append) = &self.resource_append {
            let sink = retransmit_attributes.sink.as_mut().ok_or_else(|| {
                println!("UResource was none!");
                UStatus::fail_with_code(
                    UCode::Internal,
                    &format!("UResource was none for Zenoh key expression: {}", key_expr),
                )
            })?;

            let resource = sink.resource.as_mut().ok_or_else(|| {
                println!("UResource was none!");
                UStatus::fail_with_code(
                    UCode::Internal,
                    &format!("UResource was none for Zenoh key expression: {}", key_expr),
                )
            })?;

            let message = resource.message.as_mut().ok_or_else(|| {
                println!("UResource.message was none!");
                UStatus::fail_with_code(
                    UCode::Internal,
                    &format!(
                        "UResource.message was none for Zenoh key expression: {}",
                        key_expr
                    ),
                )
            })?;

            message.push_str(&append.to_string());
        }

        match UMessageType::try_from(retransmit_attributes.r#type) {
            Ok(UMessageType::UmessageTypePublish) => {
                if let Err(e) = self
                    .up_zenoh
                    .send(retransmit_destination, payload, retransmit_attributes)
                    .await
                {
                    error!("UTransport::send() failed: {:?}", e);
                    return Err(UStatus::fail_with_code(
                        UCode::Internal,
                        &*format!("Send failed: {:?}", e),
                    ));
                }
            }
            // Ok(UMessageType::UmessageTypeResponse) => {
            //     self.send_response(&zenoh_key, payload, attributes).await
            // }
            _ => {
                return Err(UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    "Wrong Message type in UAttributes",
                ));
            }
        }

        Ok(())
    }
}
