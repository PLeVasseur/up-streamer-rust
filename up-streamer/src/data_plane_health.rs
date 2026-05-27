/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
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

//! Public data-plane health metadata for route dispatch failures.

use std::time::SystemTime;

/// Public route identity attached to data-plane health failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataPlaneRoute {
    /// Ingress endpoint name.
    pub ingress_name: String,
    /// Ingress uProtocol authority.
    pub ingress_authority: String,
    /// Egress endpoint name.
    pub egress_name: String,
    /// Egress uProtocol authority.
    pub egress_authority: String,
}

/// Data-plane failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataPlaneFailureKind {
    /// Sending a routed frame to the egress endpoint failed.
    EgressSend,
    /// The ingress listener could not enqueue a received frame because the route
    /// queue was closed.
    IngressQueueClosed,
    /// Route refresh could not unregister an old ingress listener registration.
    RouteRewireUnregister,
}

/// Details for the most recent data-plane failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataPlaneFailure {
    /// Failure category.
    pub kind: DataPlaneFailureKind,
    /// Route on which the failure happened.
    pub route: DataPlaneRoute,
    /// Diagnostic error message.
    pub message: String,
}

/// Data-plane health snapshot for streamer route dispatch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DataPlaneHealth {
    /// Count of egress send failures observed by route workers.
    pub egress_send_failures: u64,
    /// Count of frames dropped because an ingress route queue was closed.
    pub ingress_queue_failures: u64,
    /// Count of old listener unregister failures during route refresh.
    pub route_rewire_unregister_failures: u64,
    /// Time at which the most recent data-plane failure was recorded.
    pub last_failure_at: Option<SystemTime>,
    /// Most recent data-plane failure details.
    pub last_failure: Option<DataPlaneFailure>,
}

impl DataPlaneHealth {
    pub(crate) fn record(
        &mut self,
        kind: DataPlaneFailureKind,
        route: DataPlaneRoute,
        message: String,
    ) {
        match kind {
            DataPlaneFailureKind::EgressSend => {
                self.egress_send_failures = self.egress_send_failures.saturating_add(1);
            }
            DataPlaneFailureKind::IngressQueueClosed => {
                self.ingress_queue_failures = self.ingress_queue_failures.saturating_add(1);
            }
            DataPlaneFailureKind::RouteRewireUnregister => {
                self.route_rewire_unregister_failures =
                    self.route_rewire_unregister_failures.saturating_add(1);
            }
        }
        self.last_failure_at = Some(SystemTime::now());
        self.last_failure = Some(DataPlaneFailure {
            kind,
            route,
            message,
        });
    }
}
