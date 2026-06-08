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
#[cfg(feature = "owned-frame-transport")]
use up_rust::UOwnedTransport;
use up_rust::UTransport;
#[cfg(feature = "experimental-copy-minimized-routing")]
use up_rust::UZeroCopyTransport;

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
}

/// Named endpoint backed by a zero-copy transport for copy-minimized routes.
#[cfg(feature = "experimental-copy-minimized-routing")]
pub struct ZeroCopyFrameEndpoint<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    pub(crate) name: String,
    pub(crate) authority: String,
    pub(crate) transport: Arc<T>,
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl<T> Clone for ZeroCopyFrameEndpoint<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            authority: self.authority.clone(),
            transport: self.transport.clone(),
        }
    }
}

impl Endpoint {
    pub fn new(name: &str, authority: &str, transport: Arc<dyn UTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport,
        }
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl<T> ZeroCopyFrameEndpoint<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    /// Creates an endpoint for feature-gated copy-minimized routes.
    pub fn new(name: &str, authority: &str, transport: Arc<T>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport,
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
}

#[cfg(feature = "owned-frame-transport")]
impl OwnedFrameEndpoint {
    /// Creates an endpoint backed by a native owned-frame transport.
    pub fn from_owned(name: &str, authority: &str, transport: Arc<dyn UOwnedTransport>) -> Self {
        Self {
            name: name.to_string(),
            authority: authority.to_string(),
            transport,
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

    /// Returns the owned/copying compatibility mode for this endpoint.
    pub fn mode(&self) -> TransportMode {
        TransportMode::Owned
    }
}
