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

//! Route diagnostics that describe forwarding copy semantics without changing routing.

use crate::endpoint::Endpoint;
#[cfg(feature = "owned-frame-transport")]
use crate::endpoint::OwnedFrameEndpoint;

/// Public route identity attached to Streamer diagnostics.
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

impl DataPlaneRoute {
    pub(crate) fn from_parts(
        ingress_name: &str,
        ingress_authority: &str,
        egress_name: &str,
        egress_authority: &str,
    ) -> Self {
        Self {
            ingress_name: ingress_name.to_string(),
            ingress_authority: ingress_authority.to_string(),
            egress_name: egress_name.to_string(),
            egress_authority: egress_authority.to_string(),
        }
    }

    pub(crate) fn from_endpoints(ingress: &Endpoint, egress: &Endpoint) -> Self {
        Self::from_parts(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        )
    }

    #[cfg(feature = "owned-frame-transport")]
    pub(crate) fn from_owned_endpoints(
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Self {
        Self {
            ingress_name: ingress.name.clone(),
            ingress_authority: ingress.authority.clone(),
            egress_name: egress.name.clone(),
            egress_authority: egress.authority.clone(),
        }
    }

    pub(crate) fn sort_key(&self) -> (&str, &str, &str, &str) {
        (
            self.ingress_authority.as_str(),
            self.ingress_name.as_str(),
            self.egress_authority.as_str(),
            self.egress_name.as_str(),
        )
    }
}

/// Classification for how a Streamer route forwards frames.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteKind {
    /// Compatibility route through the regular `UTransport`/`UMessage` API.
    UTransportCompatibility,
    /// Compatibility route through the experimental owned-frame API.
    OwnedFrameCompatibility,
    /// Route crosses a transport-local adapter boundary that copies payload bytes.
    AdapterBacked,
    /// Future copy-minimized route category; this phase does not implement it.
    CopyMinimized,
}

impl RouteKind {
    /// Returns the explicit copy-semantics label for this route kind.
    pub fn copy_semantics(self) -> RouteCopySemantics {
        match self {
            RouteKind::UTransportCompatibility | RouteKind::OwnedFrameCompatibility => {
                RouteCopySemantics::OwnedOrMessageCopying
            }
            RouteKind::AdapterBacked => RouteCopySemantics::AdapterBoundaryCopying,
            RouteKind::CopyMinimized => RouteCopySemantics::CopyMinimizedOneCopy,
        }
    }
}

/// Public description of a route's copy semantics.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteCopySemantics {
    /// The route is compatibility routing and makes no zero-copy preservation claim.
    OwnedOrMessageCopying,
    /// A transport adapter boundary copies payload bytes.
    AdapterBoundaryCopying,
    /// A route copies receive-lease bytes directly into a transmit loan once.
    CopyMinimizedOneCopy,
}

/// Public route diagnostic snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteDiagnostic {
    /// Route identity.
    pub route: DataPlaneRoute,
    /// Route forwarding classification.
    pub route_kind: RouteKind,
    /// Explicit copy semantics for this route.
    pub copy_semantics: RouteCopySemantics,
}

impl RouteDiagnostic {
    pub(crate) fn utransport_compatibility(ingress: &Endpoint, egress: &Endpoint) -> Self {
        let route_kind = RouteKind::UTransportCompatibility;
        Self {
            route: DataPlaneRoute::from_endpoints(ingress, egress),
            route_kind,
            copy_semantics: route_kind.copy_semantics(),
        }
    }

    #[cfg(feature = "owned-frame-transport")]
    pub(crate) fn owned_frame_compatibility(
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Self {
        let route_kind = RouteKind::OwnedFrameCompatibility;
        Self {
            route: DataPlaneRoute::from_owned_endpoints(ingress, egress),
            route_kind,
            copy_semantics: route_kind.copy_semantics(),
        }
    }

    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub(crate) fn copy_minimized(
        ingress_name: &str,
        ingress_authority: &str,
        egress_name: &str,
        egress_authority: &str,
    ) -> Self {
        let route_kind = RouteKind::CopyMinimized;
        Self {
            route: DataPlaneRoute::from_parts(
                ingress_name,
                ingress_authority,
                egress_name,
                egress_authority,
            ),
            route_kind,
            copy_semantics: route_kind.copy_semantics(),
        }
    }
}
