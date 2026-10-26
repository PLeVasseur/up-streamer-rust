// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Shared explicit native route catalogs for configured bridges and role workers.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use up_rust::{
    Frame, HeaderAdmission, HeaderLease, HeaderLimits, OpaqueRoute, OpaqueTemplate,
    PreparedTransportCore, SendError, TxHeaderParts, UFrameFieldsMetadataCodec, UFrameView,
    UHasWire, UWireMetadataCodec, UWireMetadataContext,
};
use up_rust::{RouteConfig, RouteLimits, UCode, UStatus, UUri};
pub use up_streamer::PreparedEndpoint;
use up_streamer::{PreparedRouteReservation, ZeroCopyFrameEndpoint};

/// Logical address and frame bounds reserved before a native endpoint admits traffic.
///
/// Native mapping/service geometry remains binding-owned. This configuration
/// supplies shared semantic addresses and maximum logical extents to that setup.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedAddress {
    /// Exact uProtocol source URI.
    pub source: String,
    /// Exact destination URI, or None for sinkless publication.
    #[serde(default)]
    pub sink: Option<String>,
    /// Largest encoded SDK metadata block, including its prefix (default 3072).
    #[serde(default = "metadata_capacity")]
    pub max_metadata_len: usize,
    /// Largest application payload carried by this reserved address.
    pub max_payload_len: usize,
    /// Required payload alignment (default eight bytes).
    #[serde(default = "alignment")]
    pub payload_alignment: usize,
    /// Maximum access token in the route (default 2048 bytes; zero disables it).
    #[serde(default = "token_capacity")]
    pub max_token_len: usize,
    /// Whether this route permits bounded traceparent metadata (default true).
    #[serde(default = "enabled")]
    pub allow_traceparent: bool,
    /// Whether this endpoint reserves native TX for this address (default true).
    #[serde(default = "enabled")]
    pub transmit: bool,
}
const fn metadata_capacity() -> usize {
    3072
}
const fn token_capacity() -> usize {
    2048
}
const fn alignment() -> usize {
    8
}
const fn enabled() -> bool {
    true
}

struct SessionRoute<C: PreparedTransportCore> {
    source: UUri,
    sink: Option<UUri>,
    route: OpaqueRoute<C>,
}

/// Binding-owned native readiness invoked once after a role attaches its receivers.
///
/// This composes existing binding setup APIs without changing native capacities
/// or sending warm-up traffic. It is separate from first-client RPC admission.
pub trait NativeSetupReady: PreparedTransportCore {
    /// Completes configured connection/matching checks for the reserved TX addresses.
    fn wait_native_ready(
        &self,
        addresses: &[(UUri, Option<UUri>)],
        zenoh_matching: bool,
    ) -> impl std::future::Future<Output = Result<(), UStatus>> + Send;
}
#[cfg(feature = "iceoryx2-zero-copy")]
impl NativeSetupReady for up_transport_iceoryx2_rust::Iceoryx2PreparedCore {
    async fn wait_native_ready(&self, _: &[(UUri, Option<UUri>)], _: bool) -> Result<(), UStatus> {
        self.wait_ready().await
    }
}
#[cfg(feature = "zenoh-zero-copy")]
impl NativeSetupReady for up_transport_zenoh::ZenohPreparedCore {
    async fn wait_native_ready(
        &self,
        addresses: &[(UUri, Option<UUri>)],
        matching: bool,
    ) -> Result<(), UStatus> {
        if matching {
            for (source, sink) in addresses {
                self.wait_for_matching(source, sink.as_ref(), std::time::Duration::from_secs(5))
                    .await?;
            }
        }
        Ok(())
    }
}
#[cfg(feature = "lola-transport")]
impl NativeSetupReady for up_transport_lola_rust::LolaPreparedCore {
    async fn wait_native_ready(&self, _: &[(UUri, Option<UUri>)], _: bool) -> Result<(), UStatus> {
        Ok(())
    }
}

/// A role's finite sender routes and exact header admission, prepared before traffic.
///
/// Backend opening and physical geometry remain in the binding constructors.
/// This composition retains concrete core/wire types and does not declare native
/// resources, allocate descriptors or create tasks on each send.
pub struct PreparedSession<T: PreparedEndpoint> {
    transport: Arc<T>,
    routes: Box<[SessionRoute<T::Core>]>,
    admission: HeaderAdmission,
}

impl<T: PreparedEndpoint> PreparedSession<T> {
    /// Reserves all transmitting catalog entries and header admission during role setup.
    ///
    /// # Errors
    /// Rejects malformed/duplicate addresses, unavailable routes or invalid header bounds.
    pub fn new(transport: Arc<T>, catalog: &[PreparedAddress]) -> Result<Self, UStatus> {
        let template = OpaqueTemplate::for_wire::<T::Wire>().map_err(setup_status)?;
        let mut routes: Vec<SessionRoute<T::Core>> = Vec::new();
        routes
            .try_reserve_exact(catalog.len())
            .map_err(setup_status)?;
        for address in catalog.iter().filter(|address| address.transmit) {
            let definition = address.reservation()?;
            if routes
                .iter()
                .any(|entry| entry.source == definition.source && entry.sink == definition.sink)
            {
                return Err(setup_status("duplicate prepared sender address"));
            }
            let route = template
                .route(
                    Arc::clone(transport.prepared_core()),
                    definition.source.clone(),
                    definition.sink.clone(),
                    definition.limits,
                    definition.config,
                )
                .map_err(setup_status)?;
            routes.push(SessionRoute {
                source: definition.source,
                sink: definition.sink,
                route,
            });
        }
        Ok(Self {
            transport,
            routes: routes.into_boxed_slice(),
            admission: HeaderAdmission::for_wire::<T::Wire>(HeaderLimits::default())
                .map_err(UStatus::from)?,
        })
    }

    /// Borrows the configured physical core for setup-only native registration/readiness.
    pub fn core(&self) -> &Arc<T::Core> {
        self.transport.prepared_core()
    }

    /// Borrows the selected-wire adapter, including any confirmed native agreement.
    pub fn transport(&self) -> &Arc<T> {
        &self.transport
    }

    /// Completes native setup readiness after receiver registration, before traffic.
    ///
    /// # Errors
    /// Reports native connection failure or the configured bounded matching timeout.
    pub async fn wait_ready(&self, zenoh_matching: bool) -> Result<(), UStatus>
    where
        T::Core: NativeSetupReady,
    {
        let addresses: Vec<_> = self
            .routes
            .iter()
            .map(|route| (route.source.clone(), route.sink.clone()))
            .collect();
        self.core()
            .wait_native_ready(&addresses, zenoh_matching)
            .await
    }

    /// Publishes already encoded payload bytes through an exact creation-reserved route.
    ///
    /// This path intentionally copies serialized bytes once into the native loan.
    /// Stable-native role code may instead use a prepared typed SDK TX route.
    ///
    /// # Errors
    /// Reports unreserved addresses, invalid metadata/presence/capacity or native delivery errors.
    pub fn send(&self, header: TxHeaderParts<'_>, payload: Option<&[u8]>) -> Result<(), SendError> {
        let entry = self
            .routes
            .iter()
            .find(|entry| &entry.source == header.source && entry.sink.as_ref() == header.sink)
            .ok_or(SendError::Loan(up_rust::LoanError::InvalidRoute))?;
        entry.route.send_bytes(header, payload)
    }

    /// Admits a native frame in place without owning metadata strings or copying payload.
    ///
    /// # Errors
    /// Rejects incompatible wire/family/layout, malformed metadata and inconsistent presence.
    pub fn admit(
        &self,
        frame: <T::Core as PreparedTransportCore>::Rx,
    ) -> Result<HeaderLease<<T::Core as PreparedTransportCore>::Rx>, up_rust::AdmissionError> {
        self.admission.admit(frame)
    }

    /// Registers a generic header handler with one setup-owned worker and physical subscription.
    ///
    /// # Errors
    /// Reports invalid registration capacity, worker creation or native attachment failure.
    pub async fn register<H: up_rust::HeaderHandler<<T::Core as PreparedTransportCore>::Rx>>(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        config: up_rust::RegistrationConfig,
        handler: H,
    ) -> Result<up_rust::WireRegistration<T::Core>, up_rust::SetupError> {
        let worker = self
            .admission
            .register_filtered(source.clone(), sink.cloned(), config, handler, || {})
            .map_err(|_| up_rust::SetupError::Worker)?;
        up_rust::WireRegistration::attach(Arc::clone(self.core()), source, sink, worker).await
    }

    /// Decodes owned metadata for compatibility diagnostics while retaining the original native frame.
    ///
    /// This explicitly allocating view is separate from `admit` and typed SDK
    /// receive. Its payload reader borrows the original native lease without copying.
    ///
    /// # Errors
    /// Reports malformed metadata or wire mismatch; no compatibility alias is applied.
    pub fn decode_compatibility_frame(
        &self,
        frame: HeaderLease<<T::Core as PreparedTransportCore>::Rx>,
    ) -> Result<PreparedReceived<<T::Core as PreparedTransportCore>::Rx>, UStatus> {
        let raw = frame.into_frame();
        let metadata = UFrameFieldsMetadataCodec
            .decode_frame_metadata(
                UWireMetadataContext::from_wire::<T::Wire, UFrameFieldsMetadataCodec>(),
                raw.encoded_metadata(),
            )
            .map_err(UStatus::from)?;
        Ok(PreparedReceived {
            raw,
            metadata,
            agreement: self.transport.native_agreement().cloned(),
        })
    }
}
impl<T: PreparedEndpoint> UHasWire for PreparedSession<T> {
    type Wire = T::Wire;
    fn wire(&self) -> &Self::Wire {
        self.transport.wire()
    }
    fn native_agreement(&self) -> Option<&up_rust::NativeProfileAgreement> {
        self.transport.native_agreement()
    }
}

/// Allocating metadata view used by compatibility controls, retaining native payload ownership.
pub struct PreparedReceived<F: Frame> {
    raw: F,
    metadata: up_rust::UFrameMetadata,
    agreement: Option<up_rust::NativeProfileAgreement>,
}
impl<F: Frame> UFrameView for PreparedReceived<F> {
    type PayloadReader<'a>
        = std::io::Cursor<&'a [u8]>
    where
        Self: 'a;
    type PayloadSlices<'a>
        = std::option::IntoIter<&'a [u8]>
    where
        Self: 'a;
    fn metadata(&self) -> &up_rust::UFrameMetadata {
        &self.metadata
    }
    fn native_agreement(&self) -> Option<&up_rust::NativeProfileAgreement> {
        self.agreement.as_ref()
    }
    fn payload_len(&self) -> usize {
        self.raw
            .payload()
            .map_or(0, |payload| payload.bytes().len())
    }
    fn has_payload(&self) -> bool {
        self.raw.payload().is_some()
    }
    fn payload_reader(&self) -> Self::PayloadReader<'_> {
        std::io::Cursor::new(self.raw.payload().map_or(&[], |payload| payload.bytes()))
    }
    fn payload_slices(&self) -> Self::PayloadSlices<'_> {
        self.raw
            .payload()
            .map(|payload| payload.bytes())
            .into_iter()
    }
    fn try_contiguous_payload(&self) -> Option<&[u8]> {
        self.raw.payload().map(|payload| payload.bytes())
    }
}
impl<F: Frame> up_rust::UZeroCopyRxLease for PreparedReceived<F> {}
impl<F: Frame> up_rust::ULoanedContiguousZeroCopyRxFrame for PreparedReceived<F> {
    fn loaned_contiguous_payload(&self) -> Result<up_rust::LoanedPayload<'_>, up_rust::UWireError> {
        self.raw
            .payload()
            .ok_or(up_rust::UWireError::MissingPayload)
    }
}

fn setup_status(error: impl std::fmt::Display) -> UStatus {
    UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string())
}

impl PreparedAddress {
    /// Validates exact addressing and constructs SDK route limits without opening native resources.
    ///
    /// # Errors
    /// Rejects invalid/wildcard URI text, zero metadata capacity, invalid alignment,
    /// token overflow or logical length arithmetic overflow.
    pub fn reservation(&self) -> Result<PreparedRouteReservation, UStatus> {
        let invalid = |message| UStatus::fail_with_code(UCode::InvalidArgument, message);
        let source: UUri = self
            .source
            .parse()
            .map_err(|_| invalid("invalid prepared source URI"))?;
        let sink: Option<UUri> = self
            .sink
            .as_ref()
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| invalid("invalid prepared sink URI"))
            })
            .transpose()?;
        source
            .verify_no_wildcards()
            .map_err(|_| invalid("prepared source must be exact"))?;
        if let Some(sink) = &sink {
            sink.verify_no_wildcards()
                .map_err(|_| invalid("prepared sink must be exact"))?;
        }
        if self.max_metadata_len == 0
            || !self.payload_alignment.is_power_of_two()
            || self.max_token_len > u16::MAX as usize
            || self
                .max_metadata_len
                .checked_add(self.max_payload_len)
                .and_then(|len| len.checked_add(self.payload_alignment - 1))
                .is_none()
        {
            return Err(invalid("invalid prepared frame bounds"));
        }
        Ok(PreparedRouteReservation {
            source,
            sink,
            limits: RouteLimits {
                max_metadata_len: self.max_metadata_len,
                max_payload_len: self.max_payload_len,
                payload_alignment: self.payload_alignment,
            },
            config: RouteConfig {
                max_token_len: self.max_token_len,
                allow_traceparent: self.allow_traceparent,
            },
        })
    }
}

/// Applies one explicit catalog to a wire-specific endpoint using the same shared setup rules.
///
/// # Errors
/// Rejects missing/invalid addresses, duplicate egress entries or native reservation failures.
pub fn configure<T: PreparedEndpoint>(
    endpoint: ZeroCopyFrameEndpoint<T>,
    catalog: &[PreparedAddress],
) -> Result<ZeroCopyFrameEndpoint<T>, UStatus> {
    let reservations: Vec<_> = catalog
        .iter()
        .map(PreparedAddress::reservation)
        .collect::<Result<_, _>>()?;
    let receive: Vec<_> = reservations
        .iter()
        .map(|route| (route.source.clone(), route.sink.clone()))
        .collect();
    let transmit: Vec<_> = reservations
        .into_iter()
        .zip(catalog)
        .filter_map(|(route, entry)| entry.transmit.then_some(route))
        .collect();
    let endpoint = if transmit.is_empty() {
        endpoint
    } else {
        endpoint.with_routes(&transmit).map_err(|error| {
            UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string())
        })?
    };
    endpoint
        .with_receive_routes(&receive)
        .map_err(|error| UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string()))
}

/// Creates a setup-reserved iceoryx2 core using binding-owned geometry and service mapping.
///
/// The catalog determines the default fixed sample extent; optional capacities
/// override that extent and native limits explicitly. Receive-only addresses
/// retain native service ownership through receive attachment, including
/// receiver-first startup, without reserving unused publisher/data pools. Dynamic allocation remains
/// invalid for prepared traffic. Namespace environment is resolved before applying
/// explicit capacity controls, without changing native RX/borrow/slot defaults.
#[cfg(feature = "iceoryx2-zero-copy")]
pub fn iceoryx2_core(
    mut config: up_transport_iceoryx2_rust::Iceoryx2PubSubConfig,
    catalog: &[PreparedAddress],
    routes: usize,
    registrations: usize,
    capacities: &crate::capacity::Iceoryx2Capacity,
) -> Result<std::sync::Arc<up_transport_iceoryx2_rust::Iceoryx2PreparedCore>, UStatus> {
    use iceoryx2::prelude::SemanticString;
    if config.iceoryx2_config.is_none() {
        let root = std::env::var("UP_ICEORYX2_ROOT_PATH").ok();
        let prefix = std::env::var("UP_ICEORYX2_PREFIX").ok();
        if root.is_some() || prefix.is_some() {
            let mut native = iceoryx2::prelude::Config::default();
            if let Some(root) = root {
                native.global.set_root_path(
                    &iceoryx2::prelude::Path::new(root.as_bytes()).map_err(setup_status)?,
                );
            }
            if let Some(prefix) = prefix {
                native.global.prefix =
                    iceoryx2::prelude::FileName::new(prefix.as_bytes()).map_err(setup_status)?;
            }
            config = config.with_iceoryx2_config(native);
        }
    }
    let addresses = catalog
        .iter()
        .map(|address| {
            address.reservation().map(|route| {
                up_transport_iceoryx2_rust::Iceoryx2RouteReservation {
                    source: route.source,
                    sink: route.sink,
                    limits: route.limits,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let required = addresses
        .iter()
        .try_fold(0usize, |largest, address| {
            address
                .limits
                .max_metadata_len
                .checked_add(address.limits.max_payload_len)
                .and_then(|bytes| bytes.checked_add(address.limits.payload_alignment - 1))
                .map(|bytes| largest.max(bytes))
        })
        .ok_or_else(|| setup_status("prepared native frame capacity overflow"))?;
    config.publisher_initial_max_slice_len = required;
    config.publisher_allocation_strategy = iceoryx2::prelude::AllocationStrategy::Static;
    capacities.apply(&mut config)?;
    let transmit: Vec<_> = catalog.iter().map(|address| address.transmit).collect();
    up_transport_iceoryx2_rust::Iceoryx2PreparedCore::new_with_transmit_mask(
        config,
        &addresses,
        &transmit,
        routes,
        registrations,
    )
    .map(std::sync::Arc::new)
}

/// Creates a prepared Zenoh SHM core with the supplied native session configuration.
#[cfg(feature = "zenoh-zero-copy")]
pub async fn zenoh_core(
    config: up_transport_zenoh::zenoh_config::Config,
    uri: &str,
    catalog: &[PreparedAddress],
    mut setup: up_transport_zenoh::ZenohPreparedConfig,
) -> Result<std::sync::Arc<up_transport_zenoh::ZenohPreparedCore>, UStatus> {
    if setup.allocation_observer.is_none() {
        setup.allocation_observer = crate::allocation_observer::configured()?;
    }
    let addresses = catalog
        .iter()
        .map(|address| {
            address
                .reservation()
                .map(|route| up_transport_zenoh::ZenohRouteReservation {
                    source: route.source,
                    sink: route.sink,
                    limits: route.limits,
                    transmit: address.transmit,
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    up_transport_zenoh::ZenohPreparedCore::new(config, uri, &addresses, setup)
        .await
        .map(std::sync::Arc::new)
}

/// Creates a prepared LoLa core, selecting actual request/response deployments by source kind.
/// Native proxy discovery completes during attachment within the binding setup
/// deadline; shared setup adds no provider or payload pool for receive-only routes.
///
/// The caller supplies the binding's physical configurations and proxy credits.
/// Response routes have a method-resource source and a destination. The SDK still
/// validates carried message kind/correlation at admission; this is setup mapping only.
#[cfg(feature = "lola-transport")]
pub fn lola_core(
    primary: &up_transport_lola_rust::LolaTransportConfig,
    response: Option<&up_transport_lola_rust::LolaTransportConfig>,
    catalog: &[PreparedAddress],
    routes: usize,
    registrations: usize,
    borrowed: usize,
) -> Result<std::sync::Arc<up_transport_lola_rust::LolaPreparedCore>, UStatus> {
    let addresses = catalog
        .iter()
        .map(|address| {
            address.reservation().map(|route| {
                let response_route =
                    route.sink.is_some() && (1..0x8000).contains(&route.source.resource_id());
                let config = if response_route {
                    response.unwrap_or(primary)
                } else {
                    primary
                };
                up_transport_lola_rust::LolaRouteReservation {
                    source: route.source,
                    sink: route.sink,
                    limits: route.limits,
                    config: config.clone(),
                    max_borrowed_samples: borrowed,
                    transmit: address.transmit,
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    up_transport_lola_rust::LolaPreparedCore::new(&addresses, routes, registrations)
        .map(std::sync::Arc::new)
}

#[cfg(all(test, feature = "iceoryx2-zero-copy"))]
mod native_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use test_case::test_case;
    use up_rust::{
        PayloadEncoding, RegistrationConfig, RegistrationInbox, UFrameMetadata,
        UWithNativePrefixWire,
    };
    use up_transport_iceoryx2_rust::Iceoryx2PubSubConfig;

    fn setup() -> (Iceoryx2PubSubConfig, PreparedAddress, UUri) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var("UP_ICEORYX2_TEST_ROOT")
            .expect("native acceptance requires its explicit test root");
        let prefix = format!(
            "prepared-setup-{}-{}-",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut config = Iceoryx2PubSubConfig::default()
            .with_namespace(&root, &prefix)
            .unwrap();
        config.publisher_min_subscribers = 1;
        let source = UUri::try_from_parts("prepared-source", 0x5BA0, 1, 0x8001).unwrap();
        let address = PreparedAddress {
            source: source.to_uri(false),
            sink: None,
            max_metadata_len: 3072,
            max_payload_len: 64,
            payload_alignment: 8,
            max_token_len: 2048,
            allow_traceparent: true,
            transmit: true,
        };
        (config, address, source)
    }

    #[tokio::test]
    async fn default_catalog_reservation_delivers_and_retains_an_actual_native_frame() {
        let (config, address, source) = setup();
        let catalog = [address];
        let core = iceoryx2_core(config, &catalog, 2, 1, &Default::default()).unwrap();
        let session = PreparedSession::new(
            Arc::new(Arc::clone(&core).into_protobuf_transport()),
            &catalog,
        )
        .unwrap();
        let inbox = Arc::new(
            RegistrationInbox::new(RegistrationConfig {
                capacity: 1,
                overflow: up_rust::OverflowPolicy::Block {
                    deadline: Duration::from_secs(5),
                },
            })
            .unwrap(),
        );
        let mut subscription = core.attach(&source, None, inbox.clone()).await.unwrap();
        session.wait_ready(false).await.unwrap();
        let token = "t".repeat(2048);
        let metadata = UFrameMetadata::publish(source)
            .with_payload_encoding(PayloadEncoding::PROTOBUF)
            .with_token(token.clone())
            .build()
            .unwrap();
        session
            .send(TxHeaderParts::from(&metadata), Some(&[17; 64]))
            .unwrap();
        let raw = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
            .await
            .unwrap()
            .unwrap();
        let frame = session.admit(raw).unwrap();
        assert_eq!(frame.header().unwrap().token(), Some(token.as_str()));
        let pointer = frame.payload().unwrap().bytes().as_ptr();
        inbox.close();
        core.detach(&mut subscription).unwrap();
        drop(subscription);
        drop(session);
        drop(core);
        assert_eq!(frame.payload().unwrap().bytes(), &[17; 64]);
        assert_eq!(frame.payload().unwrap().bytes().as_ptr(), pointer);
    }

    #[test_case(true; "explicit undersized sample is not enlarged")]
    #[test_case(false; "explicit dynamic policy is not converted")]
    fn explicit_incompatible_capacity_is_rejected(undersized: bool) {
        use crate::capacity::{Iceoryx2Capacity, PublisherAllocationStrategy};
        let (config, address, _) = setup();
        let capacities = Iceoryx2Capacity {
            publisher_initial_max_slice_len: undersized.then_some(8),
            publisher_allocation_strategy: Some(if undersized {
                PublisherAllocationStrategy::Static
            } else {
                PublisherAllocationStrategy::PowerOfTwo
            }),
            ..Default::default()
        };
        assert!(iceoryx2_core(config, &[address], 2, 1, &capacities).is_err());
    }
}
