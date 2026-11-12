// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Configuration for one-copy forwarding into setup-reserved native egress loans.

/// Options for feature-gated copy-minimized Streamer routes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyMinimizedRouteOptions {
    /// Required egress payload alignment, bounded by the endpoint's reserved catalog.
    pub payload_alignment: usize,
}

impl Default for CopyMinimizedRouteOptions {
    fn default() -> Self {
        Self {
            payload_alignment: 1,
        }
    }
}
