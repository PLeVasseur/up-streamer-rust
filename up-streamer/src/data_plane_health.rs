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

use crate::TransportMode;

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

/// Public classification for how a streamer route forwards frames.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteKind {
    /// The route uses owned transports on both sides.
    OwnedToOwned,
    /// The route receives owned frames and copies into a zero-copy egress adapter.
    OwnedToZeroCopyAdapter,
    /// The route copies zero-copy ingress leases into owned frames before egress.
    ZeroCopyAdapterToOwned,
    /// The route uses owned-frame copying adapters on both zero-copy endpoints.
    ZeroCopyAdapterToZeroCopyAdapter,
    /// The route copies ingress zero-copy lease slices directly into egress transmit loans.
    CopyMinimizedZeroCopyToZeroCopy,
}

/// Public description of the copy semantics for a streamer route.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteCopySemantics {
    /// The streamer routes owned frames and makes no zero-copy claim for this route.
    OwnedNoStreamerCopyClaim,
    /// The route crosses an owned-frame adapter that copies at a zero-copy endpoint boundary.
    CopyingAdapterBoundary,
    /// The route copies an ingress receive lease payload directly into an egress transmit loan.
    CopyMinimizedLeaseToLoanOneCopy,
}

/// Public route diagnostic snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteDiagnostic {
    /// Route identity.
    pub route: DataPlaneRoute,
    /// Ingress endpoint capability mode.
    pub ingress_mode: TransportMode,
    /// Egress endpoint capability mode.
    pub egress_mode: TransportMode,
    /// Route forwarding classification.
    pub route_kind: RouteKind,
    /// Explicit copy semantics for this route.
    pub copy_semantics: RouteCopySemantics,
    /// Ingress queue behavior configured for this route.
    pub queue_policy: RouteQueuePolicy,
}

/// Behavior when an ingress listener receives a frame while the route queue is full.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RouteQueuePolicy {
    /// Preserve current behavior by awaiting queue capacity and applying backpressure.
    #[default]
    Backpressure,
    /// Drop the frame immediately, log, and report data-plane health degradation.
    DropAndReport,
}

/// Options for owned-frame streamer routes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RouteOptions {
    /// Ingress queue behavior for this route.
    pub queue_policy: RouteQueuePolicy,
}

/// Options for experimental copy-minimized routes.
#[cfg(feature = "experimental-loaned-frame")]
#[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyMinimizedRouteOptions {
    /// Alignment requested from the zero-copy egress transmit loan.
    pub alignment: usize,
    /// Ingress queue behavior for this route.
    pub queue_policy: RouteQueuePolicy,
}

#[cfg(feature = "experimental-loaned-frame")]
impl Default for CopyMinimizedRouteOptions {
    fn default() -> Self {
        Self {
            alignment: 1,
            queue_policy: RouteQueuePolicy::Backpressure,
        }
    }
}

/// Data-plane failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataPlaneFailureKind {
    /// Sending a routed frame to the egress endpoint failed.
    EgressSend,
    /// The ingress listener could not enqueue a received frame because the route
    /// queue was closed.
    IngressQueueClosed,
    /// The ingress listener dropped a received frame because the route queue was full.
    IngressQueueFull,
    /// Route refresh could not unregister an old ingress listener registration.
    RouteRewireUnregister,
    /// A copy-minimized route rejected payload metadata or egress loan layout before send.
    CopyMinimizedPayloadLayout,
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
    /// Count of frames dropped by explicit drop-and-report queue policy.
    pub ingress_queue_full_drops: u64,
    /// Count of old listener unregister failures during route refresh.
    pub route_rewire_unregister_failures: u64,
    /// Count of copy-minimized frames rejected before egress send for payload layout reasons.
    pub copy_minimized_payload_layout_failures: u64,
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
            DataPlaneFailureKind::IngressQueueFull => {
                self.ingress_queue_full_drops = self.ingress_queue_full_drops.saturating_add(1);
            }
            DataPlaneFailureKind::RouteRewireUnregister => {
                self.route_rewire_unregister_failures =
                    self.route_rewire_unregister_failures.saturating_add(1);
            }
            DataPlaneFailureKind::CopyMinimizedPayloadLayout => {
                self.copy_minimized_payload_layout_failures = self
                    .copy_minimized_payload_layout_failures
                    .saturating_add(1);
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
