/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
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

use std::sync::Arc;

use up_rust::{transport::UOwnedFrameEndpoint, zero_copy::UZeroCopyTransport, UOwnedTransport};

pub use up_rust::transport::UOwnedFrameEndpointMode as TransportMode;

#[derive(Clone)]
pub struct OwnedFrameEndpoint {
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: UOwnedFrameEndpoint,
}

impl OwnedFrameEndpoint {
    pub fn from_owned(name: &str, authority: &str, transport: Arc<dyn UOwnedTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport: UOwnedFrameEndpoint::from_owned(transport),
        }
    }

    pub fn from_zero_copy<T>(name: &str, authority: &str, transport: Arc<T>) -> Self
    where
        T: UZeroCopyTransport + Send + Sync + 'static,
    {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport: UOwnedFrameEndpoint::from_zero_copy(transport),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub fn mode(&self) -> TransportMode {
        self.transport.mode()
    }
}
