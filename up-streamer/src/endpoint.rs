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

#[cfg(feature = "experimental-copy-minimized-routing")]
use crate::prepared_endpoint::{
    EgressCatalog, PreparedEndpoint, PreparedForwardStats, PreparedRouteReservation,
};
use std::sync::Arc;
#[cfg(feature = "owned-frame-transport")]
use up_rust::UOwnedTransport;
use up_rust::UTransport;

///
/// [`Endpoint`] is defined as a combination of `authority_name` and
/// [`Arc<Mutex<Box<dyn UTransport>>>`][up_rust::UTransport] as endpoints are at the authority level.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
/// use up_rust::UTransport;
/// use up_streamer::Endpoint;
///
/// # pub mod up_client_foo {
/// #     use std::sync::Arc;
/// #     use up_rust::{UMessage, UTransport, UStatus, UUri, UListener};
/// #     use async_trait::async_trait;
/// #     pub struct UPClientFoo;
/// #
/// #     #[async_trait]
/// #     impl UTransport for UPClientFoo {
/// #         async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
/// #             todo!()
/// #         }
/// #
/// #         async fn receive(
/// #             &self,
/// #            _source_filter: &UUri,
/// #            _sink_filter: Option<&UUri>,
/// #         ) -> Result<UMessage, UStatus> {
/// #             todo!()
/// #         }
/// #
/// #         async fn register_listener(
/// #                     &self,
/// #                     source_filter: &UUri,
/// #                     sink_filter: Option<&UUri>,
/// #                     listener: Arc<dyn UListener>,
/// #         ) -> Result<(), UStatus> {
/// #             println!("UPClientFoo: registering source_filter: {:?}", source_filter);
/// #             Ok(())
/// #         }
/// #
/// #         async fn unregister_listener(
/// #                     &self,
/// #                     source_filter: &UUri,
/// #                     sink_filter: Option<&UUri>,
/// #                     listener: Arc<dyn UListener>,
/// #         ) -> Result<(), UStatus> {
/// #             println!(
/// #                 "UPClientFoo: unregistering source_filter: {source_filter:?}"
/// #             );
/// #             Ok(())
/// #         }
/// #     }
/// #
/// #     impl UPClientFoo {
/// #         pub fn new() -> Self {
/// #             Self {}
/// #         }
/// #     }
/// # }
///
/// let local_transport: Arc<dyn UTransport> = Arc::new(up_client_foo::UPClientFoo::new());
///
/// let authority_foo = "foo_authority";
///
/// let local_endpoint = Endpoint::new("local_endpoint", authority_foo, local_transport);
/// ```
#[derive(Clone)]
pub struct Endpoint {
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: Arc<dyn UTransport>,
    pub(crate) native_profile: Option<Arc<up_rust::NativeProfile>>,
}

/// How a feature-gated owned-frame endpoint reaches its transport.
#[cfg(feature = "owned-frame-transport")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportMode {
    /// Native owned-frame transport path. This is owned/copying compatibility,
    /// not zero-copy-preserving forwarding.
    Owned,
}

/// Named endpoint backed by an experimental owned-frame transport.
#[cfg(feature = "owned-frame-transport")]
#[derive(Clone)]
pub struct OwnedFrameEndpoint {
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: Arc<dyn UOwnedTransport>,
    pub(crate) native_profile: Option<Arc<up_rust::NativeProfile>>,
}

/// Shared exact-address catalog used while attaching native receive filters.
#[cfg(feature = "experimental-copy-minimized-routing")]
type ReceiveCatalog = Arc<[(up_rust::UUri, Option<up_rust::UUri>)]>;

/// Named endpoint backed by a prepared native core and an exact selected wire.
#[cfg(feature = "experimental-copy-minimized-routing")]
pub struct ZeroCopyFrameEndpoint<T>
where
    T: PreparedEndpoint,
{
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: Arc<T>,
    pub(crate) native_profile: Option<Arc<up_rust::NativeProfile>>,
    pub(crate) routes: Option<Arc<EgressCatalog<T::Core>>>,
    pub(crate) receive_catalog: Option<ReceiveCatalog>,
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl<T> Clone for ZeroCopyFrameEndpoint<T>
where
    T: PreparedEndpoint,
{
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            authority: self.authority.clone(),
            transport: self.transport.clone(),
            native_profile: self.native_profile.clone(),
            routes: self.routes.clone(),
            receive_catalog: self.receive_catalog.clone(),
        }
    }
}

impl Endpoint {
    /// Names a classic transport endpoint and its routing authority, initially without a native profile.
    pub fn new(name: &str, authority: &str, transport: Arc<dyn UTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport,
            native_profile: None,
        }
    }

    /// Attaches this endpoint's immutable deployment profile. Route activation
    /// checks its domain, version and complete content against the peer's profile.
    #[must_use]
    pub fn with_native_profile(mut self, profile: Arc<up_rust::NativeProfile>) -> Self {
        self.native_profile = Some(profile);
        self
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl<T> ZeroCopyFrameEndpoint<T>
where
    T: PreparedEndpoint,
{
    /// Creates an endpoint for feature-gated copy-minimized routes.
    pub fn new(name: &str, authority: &str, transport: Arc<T>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport,
            native_profile: None,
            routes: None,
            receive_catalog: None,
        }
    }

    /// Reserves every exact egress address and maximum frame during configuration.
    ///
    /// Ingress-only endpoints need no egress catalog. Route installation rejects a
    /// missing catalog on an egress; traffic never creates or enlarges it.
    ///
    /// # Errors
    /// Rejects duplicate/wildcard addresses, invalid bounds or native route exhaustion.
    pub fn with_routes(
        mut self,
        routes: &[PreparedRouteReservation],
    ) -> Result<Self, up_rust::SetupError> {
        self.routes = Some(Arc::new(EgressCatalog::new::<T::Wire>(
            Arc::clone(self.transport.prepared_core()),
            routes,
            None,
        )?));
        self.receive_catalog = Some(
            routes
                .iter()
                .map(|route| (route.source.clone(), route.sink.clone()))
                .collect(),
        );
        Ok(self)
    }

    /// Reserves exact egress routes and a synchronous actual-boundary observer.
    ///
    /// # Errors
    /// Rejects the same invalid addresses/bounds/resource exhaustion as `with_routes`.
    pub fn with_observed_routes(
        mut self,
        routes: &[PreparedRouteReservation],
        observer: Arc<dyn crate::PreparedForwardObserver>,
    ) -> Result<Self, up_rust::SetupError> {
        self.routes = Some(Arc::new(EgressCatalog::new::<T::Wire>(
            Arc::clone(self.transport.prepared_core()),
            routes,
            Some(observer),
        )?));
        self.receive_catalog = Some(
            routes
                .iter()
                .map(|route| (route.source.clone(), route.sink.clone()))
                .collect(),
        );
        Ok(self)
    }

    /// Declares exact receive addresses available in this native core's setup catalog.
    ///
    /// Ingress routing intersects semantic filters with these addresses before
    /// attaching native subscribers; it does not ask a pubsub-only catalog to
    /// materialize an unrelated RPC service. Runtime filters remain SDK-validated.
    ///
    /// # Errors
    /// Rejects an empty catalog or a wildcard address.
    pub fn with_receive_routes(
        mut self,
        routes: &[(up_rust::UUri, Option<up_rust::UUri>)],
    ) -> Result<Self, up_rust::SetupError> {
        if routes.is_empty() {
            return Err(up_rust::SetupError::InvalidConfig(
                "receive catalog must not be empty",
            ));
        }
        for (source, sink) in routes {
            source
                .verify_no_wildcards()
                .map_err(|_| up_rust::SetupError::InvalidConfig("exact receive source required"))?;
            if let Some(sink) = sink {
                sink.verify_no_wildcards().map_err(|_| {
                    up_rust::SetupError::InvalidConfig("exact receive sink required")
                })?;
            }
        }
        self.receive_catalog = Some(routes.to_vec().into());
        Ok(self)
    }

    /// Observes this endpoint's egress copy/success/failure accounting, when configured.
    pub fn forwarding_stats(&self) -> Option<Arc<PreparedForwardStats>> {
        self.routes.as_ref().map(|routes| routes.stats())
    }

    /// Borrows the concrete selected-wire adapter for binding-owned setup/readiness.
    pub fn transport(&self) -> &Arc<T> {
        &self.transport
    }

    /// Attaches the immutable deployment profile used to configure this endpoint.
    #[must_use]
    pub fn with_native_profile(mut self, profile: Arc<up_rust::NativeProfile>) -> Self {
        self.native_profile = Some(profile);
        self
    }

    /// Human-readable endpoint name used in diagnostics and route keys.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// uProtocol authority represented by this endpoint.
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

#[cfg(feature = "owned-frame-transport")]
impl OwnedFrameEndpoint {
    /// Creates an endpoint backed by a native owned-frame transport.
    pub fn from_owned(name: &str, authority: &str, transport: Arc<dyn UOwnedTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport,
            native_profile: None,
        }
    }

    /// Attaches the immutable deployment profile used to configure this endpoint.
    #[must_use]
    pub fn with_native_profile(mut self, profile: Arc<up_rust::NativeProfile>) -> Self {
        self.native_profile = Some(profile);
        self
    }

    /// Human-readable endpoint name used in diagnostics and route keys.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// uProtocol authority represented by this endpoint.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Returns the owned/copying compatibility mode for this endpoint.
    pub fn mode(&self) -> TransportMode {
        TransportMode::Owned
    }
}

pub(crate) fn native_route_agreement(
    ingress: Option<&Arc<up_rust::NativeProfile>>,
    egress: Option<&Arc<up_rust::NativeProfile>>,
) -> Result<Option<up_rust::NativeProfileAgreement>, up_rust::UStatus> {
    match (ingress, egress) {
        (None, None) => Ok(None),
        (Some(local), Some(peer)) => up_rust::NativeProfileAgreement::new(local.clone(), peer)
            .map(Some)
            .map_err(|error| {
                up_rust::UStatus::fail_with_code(up_rust::UCode::InvalidArgument, error.to_string())
            }),
        _ => Err(up_rust::UStatus::fail_with_code(
            up_rust::UCode::InvalidArgument,
            "native route requires explicit matching profile configuration at both endpoints",
        )),
    }
}
