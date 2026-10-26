// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Setup-owned selected-wire routes for native Streamer boundaries.

use std::{
    mem::MaybeUninit,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use up_rust::{
    Frame, FrameSink, HeaderLease, InitializedTx, LoanError, OpaqueRoute, OpaqueTemplate,
    PreparedTransportCore, PreparedTxStorage, RouteConfig, RouteLimits, SendError, SetupError,
    TxFrameLayout, UFrameFieldsMetadataCodec, UHasWire, UUri, UWire, UWireTransport, WrittenHeader,
};

/// Setup-owned observer at actual native ingress, copy and publish boundaries.
///
/// Callbacks synchronously borrow live bytes and must not retain them. Reserve
/// observer storage before traffic for zero-allocation use; observer costs stay
/// inside the measured operation. Correlate callbacks by the carried message ID,
/// because different registrations may execute on different workers.
pub trait PreparedForwardObserver: Send + Sync + 'static {
    /// Observes admitted ingress before egress address lookup or copying.
    fn ingress(&self, header: &up_rust::RxHeader<'_>, payload: Option<&[u8]>);
    /// Observes initialized metadata in the actual egress loan before native put.
    fn copied_metadata(&self, id: &up_rust::UUID, metadata: &[u8]);
    /// Observes the copied actual egress payload; None differs from present empty.
    fn copied_payload(&self, id: &up_rust::UUID, payload: Option<&[u8]>);
    /// Completes each admitted ingress observation, including unreserved addresses
    /// and failed puts after a completed copy. Pre-admission rejection has no ingress.
    fn completed(&self, id: &up_rust::UUID, result: Result<(), SendError>);
}

/// An explicitly selected wire over a creation-reserved native core.
///
/// This setup accessor keeps the concrete core/wire types. Transmit, receiver
/// admission and generic async handlers use the SDK's prepared contracts directly.
pub trait PreparedEndpoint: UHasWire + Send + Sync + 'static {
    /// Physical native core retaining its setup resources.
    type Core: PreparedTransportCore;
    /// Borrows the shared physical core configured before admission.
    fn prepared_core(&self) -> &Arc<Self::Core>;
}
impl<C, W> PreparedEndpoint for UWireTransport<Arc<C>, W, UFrameFieldsMetadataCodec>
where
    C: PreparedTransportCore,
    W: UWire + Send + Sync + 'static,
{
    type Core = C;
    fn prepared_core(&self) -> &Arc<C> {
        self.core()
    }
}

/// Exact logical address and maximum frame reserved before a route is installed.
#[derive(Clone, Debug)]
pub struct PreparedRouteReservation {
    /// Unmodified incoming source forwarded through this route.
    pub source: UUri,
    /// Exact sink, or None for sinkless publications.
    pub sink: Option<UUri>,
    /// Metadata, payload and alignment bounds delegated to the native core.
    pub limits: RouteLimits,
    /// Token/traceparent bounds fixed at setup.
    pub config: RouteConfig,
}

/// Native forwarding observations without per-message formatting or allocation.
#[derive(Default, Debug)]
pub struct PreparedForwardStats {
    /// Frames successfully published to an egress core.
    pub forwarded: AtomicU64,
    /// Calls rejected because their exact logical address was not reserved.
    pub unreserved: AtomicU64,
    /// Failed sends after finding the reserved route.
    pub failed: AtomicU64,
    /// Bytes actually copied into an initialized egress loan, including failed puts.
    pub copied_payload_bytes: AtomicU64,
    /// Present payload extents copied, including present-empty extents.
    pub payload_copies: AtomicU64,
}

struct CountedCore<C: PreparedTransportCore> {
    core: Arc<C>,
    stats: Arc<PreparedForwardStats>,
    observer: Option<Arc<dyn PreparedForwardObserver>>,
    prefix: Box<[u8]>,
}
struct CountedTx<T> {
    inner: T,
    payload_len: Option<usize>,
}
// SAFETY: the wrapped storage retains its original exclusive, immobile and disjoint
// extents. This wrapper only forwards borrows and records an immutable byte count;
// it does not resize, replace, copy or share the native backing storage.
unsafe impl<T: PreparedTxStorage> PreparedTxStorage for CountedTx<T> {
    fn metadata_uninit_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        self.inner.metadata_uninit_mut()
    }
    fn payload_uninit_mut(&mut self) -> Option<&mut [MaybeUninit<u8>]> {
        self.inner.payload_uninit_mut()
    }
    fn record_header(&mut self, header: WrittenHeader) -> Result<(), SendError> {
        self.inner.record_header(header)
    }
}
impl<C: PreparedTransportCore> PreparedTransportCore for CountedCore<C> {
    type Route = C::Route;
    type Tx<'a> = CountedTx<C::Tx<'a>>;
    type Rx = C::Rx;
    type Subscription = C::Subscription;
    fn prepare_route(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        limits: RouteLimits,
    ) -> Result<Self::Route, SetupError> {
        self.core.prepare_route(source, sink, limits)
    }
    fn loan<'a>(
        &'a self,
        route: &'a Self::Route,
        layout: TxFrameLayout,
    ) -> Result<Self::Tx<'a>, LoanError> {
        self.core.loan(route, layout).map(|inner| CountedTx {
            inner,
            payload_len: layout.payload_len,
        })
    }
    fn publish<'a>(
        &'a self,
        route: &'a Self::Route,
        frame: InitializedTx<Self::Tx<'a>>,
    ) -> Result<(), SendError> {
        let mut tx = frame.into_storage();
        // Only OpaqueRoute uses this private wrapper; its initialization proof
        // reaches publish after the payload was actually copied. Charge that
        // work before native publication so copied-then-failed puts remain counted.
        if let Some(bytes) = tx.payload_len {
            self.stats
                .copied_payload_bytes
                .fetch_add(bytes as u64, Ordering::Relaxed);
            self.stats.payload_copies.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(observer) = &self.observer {
            let id = {
                // SAFETY: InitializedTx proves every logical metadata byte is
                // initialized; the native loan remains owned and immobile. The
                // immutable borrow ends before another storage accessor is used.
                let metadata = unsafe { initialized_bytes(tx.inner.metadata_uninit_mut()) };
                let header = up_rust::parse_header(
                    &self.prefix,
                    metadata,
                    &up_rust::HeaderLimits {
                        max_token_len: u16::MAX as usize,
                    },
                )
                .map_err(SendError::Header)?;
                observer.copied_metadata(&header.id, metadata);
                header.id
            };
            let payload = tx.inner.payload_uninit_mut().map(|bytes| {
                // SAFETY: InitializedTx separately proves the whole payload
                // initialized; this borrow ends before publication consumes it.
                unsafe { initialized_bytes(bytes) }
            });
            observer.copied_payload(&id, payload);
        }
        // SAFETY: the SDK InitializedTx proved the unchanged wrapped storage.
        // Removing this forwarding-only wrapper preserves every byte, extent,
        // native owner, header record and initializer witness.
        let frame = unsafe { InitializedTx::from_initialized(tx.inner) };
        self.core.publish(route, frame)
    }
    async fn attach(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        inbox: Arc<dyn FrameSink<Self::Rx>>,
    ) -> Result<Self::Subscription, SetupError> {
        self.core.attach(source, sink, inbox).await
    }
    fn detach(&self, subscription: &mut Self::Subscription) -> Result<(), SetupError> {
        self.core.detach(subscription)
    }
}

struct Entry<C: PreparedTransportCore> {
    source: UUri,
    sink: Option<UUri>,
    limits: RouteLimits,
    route: OpaqueRoute<CountedCore<C>>,
}
pub(crate) struct EgressCatalog<C: PreparedTransportCore> {
    entries: Box<[Entry<C>]>,
    stats: Arc<PreparedForwardStats>,
    observer: Option<Arc<dyn PreparedForwardObserver>>,
    #[cfg(feature = "owned-frame-transport")]
    admission: up_rust::HeaderAdmission,
    #[cfg(feature = "owned-frame-transport")]
    context: up_rust::UWireMetadataContext,
}
impl<C: PreparedTransportCore> EgressCatalog<C> {
    pub(crate) fn new<W: UWire>(
        core: Arc<C>,
        definitions: &[PreparedRouteReservation],
        observer: Option<Arc<dyn PreparedForwardObserver>>,
    ) -> Result<Self, SetupError> {
        if definitions.is_empty() {
            return Err(SetupError::InvalidConfig(
                "egress route catalog must not be empty",
            ));
        }
        let template = OpaqueTemplate::for_wire::<W>()?;
        let admission = up_rust::HeaderAdmission::for_wire::<W>(up_rust::HeaderLimits::default())
            .map_err(|_| SetupError::Prefix)?;
        let stats = Arc::new(PreparedForwardStats::default());
        let counted = Arc::new(CountedCore {
            core,
            stats: Arc::clone(&stats),
            observer: observer.clone(),
            prefix: admission.prefix().into(),
        });
        let mut entries: Vec<Entry<C>> = Vec::new();
        entries
            .try_reserve_exact(definitions.len())
            .map_err(|_| SetupError::Allocation)?;
        for definition in definitions {
            if entries
                .iter()
                .any(|entry| entry.source == definition.source && entry.sink == definition.sink)
            {
                return Err(SetupError::InvalidConfig(
                    "duplicate prepared egress address",
                ));
            }
            let route = template.route(
                Arc::clone(&counted),
                definition.source.clone(),
                definition.sink.clone(),
                definition.limits,
                definition.config,
            )?;
            entries.push(Entry {
                source: definition.source.clone(),
                sink: definition.sink.clone(),
                limits: definition.limits,
                route,
            });
        }
        Ok(Self {
            entries: entries.into_boxed_slice(),
            stats,
            observer,
            #[cfg(feature = "owned-frame-transport")]
            admission,
            #[cfg(feature = "owned-frame-transport")]
            context: up_rust::UWireMetadataContext::from_wire::<W, UFrameFieldsMetadataCodec>(),
        })
    }
    fn entry(&self, source: &UUri, sink: Option<&UUri>) -> Result<&Entry<C>, SendError> {
        self.entries
            .iter()
            .find(|entry| &entry.source == source && entry.sink.as_ref() == sink)
            .ok_or_else(|| {
                self.stats.unreserved.fetch_add(1, Ordering::Relaxed);
                SendError::Loan(LoanError::InvalidRoute)
            })
    }
    fn outcome(&self, result: Result<(), SendError>) -> Result<(), SendError> {
        if result.is_ok() {
            self.stats.forwarded.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.failed.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
    pub(crate) fn forward<F: Frame>(&self, frame: &HeaderLease<F>) -> Result<(), SendError> {
        let header = frame.header().map_err(|_| SendError::PayloadStorage)?;
        if let Some(observer) = &self.observer {
            observer.ingress(&header, frame.payload().map(|payload| payload.bytes()));
        }
        let result = match self.entry(&header.source, header.sink.as_ref()) {
            Ok(entry) => self.outcome(entry.route.forward(frame)),
            Err(error) => Err(error),
        };
        if let Some(observer) = &self.observer {
            observer.completed(&header.id, result);
        }
        result
    }
    #[cfg(feature = "owned-frame-transport")]
    pub(crate) fn forward_owned(&self, frame: &up_rust::UOwnedFrame) -> Result<(), SendError> {
        use up_rust::UWireMetadataCodec;
        let metadata = UFrameFieldsMetadataCodec
            .encode_frame_metadata(self.context, frame.metadata())
            .map_err(|_| SendError::PayloadStorage)?;
        let raw = OwnedAdapterFrame {
            metadata,
            payload: frame.payload().cloned(),
        };
        let admitted = self
            .admission
            .admit(raw)
            .map_err(|_| SendError::PayloadStorage)?;
        self.forward(&admitted)
    }
    pub(crate) fn check_alignment(&self, alignment: usize) -> Result<(), SetupError> {
        if !alignment.is_power_of_two()
            || self
                .entries
                .iter()
                .any(|entry| entry.limits.payload_alignment < alignment)
        {
            Err(SetupError::InvalidConfig(
                "route option exceeds reserved egress alignment",
            ))
        } else {
            Ok(())
        }
    }
    pub(crate) fn stats(&self) -> Arc<PreparedForwardStats> {
        Arc::clone(&self.stats)
    }
}

// Caller must prove every byte initialized and retain this exact storage borrow.
unsafe fn initialized_bytes(bytes: &[MaybeUninit<u8>]) -> &[u8] {
    // SAFETY: caller supplies initialization and lifetime proof; u8 and
    // MaybeUninit<u8> have identical byte layout, alignment and extent.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len()) }
}

#[cfg(feature = "owned-frame-transport")]
struct OwnedAdapterFrame {
    metadata: Vec<u8>,
    payload: Option<bytes::Bytes>,
}
#[cfg(feature = "owned-frame-transport")]
impl Frame for OwnedAdapterFrame {
    fn encoded_metadata(&self) -> &[u8] {
        &self.metadata
    }
    fn payload(&self) -> Option<up_rust::LoanedPayload<'_>> {
        self.payload.as_ref().map(|payload| {
            up_rust::LoanedPayload::new(payload, up_rust::PayloadLoanProvenance::OwnedReceiveLease)
        })
    }
}
