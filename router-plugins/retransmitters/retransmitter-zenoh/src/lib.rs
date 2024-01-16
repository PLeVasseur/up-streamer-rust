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
use log::debug;
use retransmitter::Retransmitter;
use uprotocol_sdk::uprotocol::{UCode, UMessage, UStatus, UUri};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;

pub struct RetransmitterZenoh {
    resource_append: Option<u8>, // used when operating in "dummy" mode to append this to the resource
    up_zenoh: ULinkZenoh,
}

impl RetransmitterZenoh {
    pub async fn new(runtime: Runtime, resource_append: Option<u8>) -> RetransmitterZenoh {
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
    async fn retransmit(&self, destination: UUri, message: UMessage) -> UStatus {
        let key_expr = match ULinkZenoh::to_zenoh_key_string(&destination) {
            Ok(ke) => ke,
            Err(e) => {
                return UStatus::fail_with_code(
                    UCode::Internal,
                    &*format!("Unable to convert UUri to Zenoh key expression: {:?}", e),
                );
            }
        };

        // Check if the key expression starts with "@", i.e is internal message to Zenoh
        if key_expr.starts_with('@') {
            debug!(
                "Ignoring Zenoh internal message with key expression: '{}'",
                &key_expr
            );
            return UStatus::ok();
        }

        // Check if we have received a message we already retransmitted
        if let Some(resource_append) = self.resource_append {
            if key_expr.ends_with(&resource_append.to_string()) {
                debug!(
                    "Ignoring already retransmitted message with key expression: '{}'",
                    &key_expr
                );
                return UStatus::ok();
            }
        }

        let retransmit_key_expr = {
            if let Some(resource_append) = self.resource_append {
                key_expr.clone().push_str(&resource_append.to_string());
            }
            key_expr.clone()
        };

        // TODO: Implement checking the UAuthority here, when we want to use this with remote uDevices
        if destination.authority.is_none() {
            debug!(
                "Only retransmit messages onto other protocols to other uDevices: '{}'",
                &key_expr
            );
            return UStatus::ok();
        }

        UStatus::ok()
    }
}
