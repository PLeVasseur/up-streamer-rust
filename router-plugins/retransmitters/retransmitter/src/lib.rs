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
use uprotocol_sdk::uprotocol::{UAttributes, /*UMessage,*/ UPayload, UStatus, UUri};

#[async_trait]
pub trait Retransmitter {
    async fn retransmit(
        &self,
        destination: UUri,
        payload: UPayload,
        attributes: UAttributes,
    ) -> Result<(), UStatus>;
    // async fn retransmit(&self, destination: UUri, message: UMessage) -> UStatus;
}
