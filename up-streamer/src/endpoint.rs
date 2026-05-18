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

/// Indicates whether a streamer endpoint delegates to an owned transport or
/// adapts a zero-copy transport through an owned-frame copy boundary.
pub use up_rust::transport::UOwnedFrameEndpointMode as TransportMode;

/// Named streamer endpoint with a uProtocol authority and transport facade.
///
/// `OwnedFrameEndpoint` is intentionally an owned-frame abstraction. When it is
/// built from a zero-copy transport, the underlying
/// [`up_rust::transport::UOwnedFrameEndpoint`] copies receive leases into owned
/// frames and copies owned egress frames into transmit loans.
#[derive(Clone)]
pub struct OwnedFrameEndpoint {
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: UOwnedFrameEndpoint,
}

impl OwnedFrameEndpoint {
    /// Creates a streamer endpoint backed by an [`up_rust::UOwnedTransport`].
    ///
    /// Sends and listener registration remain on the owned-frame path.
    pub fn from_owned(name: &str, authority: &str, transport: Arc<dyn UOwnedTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport: UOwnedFrameEndpoint::from_owned(transport),
        }
    }

    /// Creates a streamer endpoint backed by a true zero-copy transport.
    ///
    /// This constructor adapts the transport to the streamer's owned-frame router.
    /// It is useful for bridging shared-memory transports, but it is a copy
    /// boundary rather than end-to-end zero-copy forwarding.
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

    /// Human-readable endpoint name used in diagnostics and route keys.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// uProtocol authority represented by this endpoint.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Returns whether this endpoint is owned-backed or zero-copy-adapted.
    pub fn mode(&self) -> TransportMode {
        self.transport.mode()
    }
}
