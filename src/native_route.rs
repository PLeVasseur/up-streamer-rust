// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Complete declared-route native identity policy, independent of physical storage.

use std::{collections::BTreeSet, sync::Arc};
use up_rust::{NativeProfile, NativeProfileAgreement, NativeProfileMode, UCode, UStatus};

/// API class of a declared route member, irrespective of its payload encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeRouteApi {
    /// Classic `UTransport`/`UMessage` carriage, which has no structural-token field.
    Classic,
    /// Validated owned frames, including adapters over encoded-owned cores.
    OwnedFrame,
    /// Validated native-loan frames exposed by a zero-copy transport API.
    NativeLoan,
}

impl NativeRouteApi {
    /// Parses an explicit route-model API class.
    pub fn parse(value: &str) -> Result<Self, UStatus> {
        match value {
            "classic" => Ok(Self::Classic),
            "owned_frame" => Ok(Self::OwnedFrame),
            "native_loan" => Ok(Self::NativeLoan),
            _ => Err(invalid("unknown native route API class")),
        }
    }

    /// Returns the configuration spelling of this API class.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::OwnedFrame => "owned_frame",
            Self::NativeLoan => "native_loan",
        }
    }
}

/// One independently provisioned member of a complete declared native route.
#[derive(Clone, Debug)]
pub struct NativeRouteHop {
    /// Unique member name; local boundary names match the Streamer endpoint names.
    pub name: String,
    /// Declared transport/API class, not inferred from payload bytes.
    pub api: NativeRouteApi,
    /// Loaded immutable profile for this member, checked against deployment expectations.
    pub profile: Arc<NativeProfile>,
}

/// Per-route native policy over every declared member, including remote classic hops.
///
/// Construct this from the complete provisioned route, not just the local boundary.
/// The explicit override acknowledges lost classic identity evidence; it neither
/// restores that evidence nor bypasses profile, representation or filter checks.
#[derive(Clone, Debug)]
pub struct NativeRoutePolicy {
    name: String,
    hops: Vec<NativeRouteHop>,
    allow_single_type_over_classic_hops: bool,
}

impl NativeRoutePolicy {
    /// Constructs a complete route declaration with the classic-hop override disabled.
    ///
    /// Rejects empty/duplicate members and mismatched full profiles. Use [`Self::validate`]
    /// after selecting the explicit policy and before activating any route segment.
    pub fn new(name: impl Into<String>, hops: Vec<NativeRouteHop>) -> Result<Self, UStatus> {
        let policy = Self {
            name: name.into(),
            hops,
            allow_single_type_over_classic_hops: false,
        };
        if policy.name.is_empty() || !(2..=4096).contains(&policy.hops.len()) {
            return Err(invalid(
                "native route needs a name and complete distinct membership",
            ));
        }
        let mut names = BTreeSet::new();
        let first = &policy.hops[0].profile;
        for hop in &policy.hops {
            if hop.name.is_empty() || !names.insert(&hop.name) {
                return Err(invalid("native route has an empty or duplicate member"));
            }
            NativeProfileAgreement::new(Arc::clone(first), &hop.profile)
                .map_err(|error| invalid(error.to_string()))?;
        }
        Ok(policy)
    }

    /// Explicitly acknowledges SingleType identity loss on this route's classic hops.
    #[must_use]
    pub fn with_allow_single_type_over_classic_hops(mut self, allow: bool) -> Self {
        self.allow_single_type_over_classic_hops = allow;
        self
    }

    /// Checks every declared hop, even when the local ingress and egress are nonclassic.
    pub fn validate(&self) -> Result<(), UStatus> {
        let single_type = matches!(
            self.hops[0].profile.mode(),
            NativeProfileMode::SingleType(_)
        );
        if single_type
            && self
                .hops
                .iter()
                .any(|hop| hop.api == NativeRouteApi::Classic)
            && !self.allow_single_type_over_classic_hops
        {
            return Err(invalid(format!("native route {}: SingleType over a classic hop requires allow_single_type_over_classic_hops=true", self.name)));
        }
        Ok(())
    }

    /// Verifies actual local endpoint API classes and profiles against the complete declaration.
    pub fn validate_boundary(
        &self,
        ingress: (&str, NativeRouteApi, &NativeProfile),
        egress: (&str, NativeRouteApi, &NativeProfile),
    ) -> Result<(), UStatus> {
        self.validate()?;
        if ingress.0 == egress.0 {
            return Err(invalid("native route boundary must have distinct members"));
        }
        for (name, api, profile) in [ingress, egress] {
            let declared = self
                .hops
                .iter()
                .find(|hop| hop.name == name)
                .ok_or_else(|| {
                    invalid("native route boundary member is missing from the complete declaration")
                })?;
            if declared.api != api || declared.profile.as_ref() != profile {
                return Err(invalid(
                    "native route boundary API/profile differs from the complete declaration",
                ));
            }
        }
        Ok(())
    }

    /// Records the accepted complete declaration once after successful route activation.
    ///
    /// Configuration frontends use this for nonclassic segments after checking
    /// [`Self::validate_boundary`]. Policy-aware classic route APIs record it themselves.
    pub fn record_activation(&self) {
        let classes: Vec<_> = self
            .hops
            .iter()
            .map(|hop| (hop.name.as_str(), hop.api.as_str()))
            .collect();
        tracing::info!(event = "native_route_activation", route = %self.name,
            allow_single_type_over_classic_hops = self.allow_single_type_over_classic_hops,
            hops = ?classes, "NATIVE_ROUTE_ACTIVATED");
    }
}

pub(crate) fn admit_classic_boundary(
    ingress: (&str, NativeRouteApi, Option<&Arc<NativeProfile>>),
    egress: (&str, NativeRouteApi, Option<&Arc<NativeProfile>>),
    policy: Option<&NativeRoutePolicy>,
) -> Result<Option<NativeProfileAgreement>, UStatus> {
    let agreement = crate::endpoint::native_route_agreement(ingress.2, egress.2)?;
    if let Some(policy) = policy {
        let left = ingress
            .2
            .ok_or_else(|| invalid("native route policy requires an ingress profile"))?;
        let right = egress
            .2
            .ok_or_else(|| invalid("native route policy requires an egress profile"))?;
        policy.validate_boundary((ingress.0, ingress.1, left), (egress.0, egress.1, right))?;
    } else if agreement
        .as_ref()
        .is_some_and(|value| matches!(value.profile().mode(), NativeProfileMode::SingleType(_)))
    {
        return Err(invalid("SingleType over a classic hop requires a complete native route policy with allow_single_type_over_classic_hops=true"));
    }
    Ok(agreement)
}

fn invalid(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
