#![cfg(feature = "experimental-copy-minimized-routing")]

use async_trait::async_trait;
use bytes::Bytes;
use std::{
    io::Cursor,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::time::{sleep, Duration};
use up_rust::communication::{zero_copy, CallOptions, SubscriptionStatus};
use up_rust::core::usubscription::{ResetReason, SubscriptionInfo, USubscription};
use up_rust::selected_wire_user_api::{
    ProtobufWireTransport, UProtocolNativeWire, UWithNativePrefixWire,
};
use up_rust::transport_implementer_api::{
    PreparedTxLoanSpec, UEncodedRxFrame, UEncodedZeroCopyListener, UZeroCopyTransportCore,
};
use up_rust::wire_implementer_api::{
    NativePrefixFrameMetadataCodec, ProtobufWire, UWire, UWireMetadataCodec,
};
use up_rust::{
    try_project_umessage_to_frame_metadata, ByteBackedStablePayload, InMemoryZeroCopyTransport,
    StablePayload, StaticUriProvider, UCode, UFrameMetadata, UFrameView, UMessageBuilder,
    UPayloadFormat, UStatus, UTxBuffer, UUri, UVecRxLease, UZeroCopyListener,
    UZeroCopyTransportImpl, ValidatedTxLoanSpec,
};
use up_rust::{UZeroCopyRxLease, UZeroCopyTransport};
use up_streamer::{
    CopyMinimizedRouteOptions, RouteCopySemantics, RouteKind, UStreamer, ZeroCopyFrameEndpoint,
};
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
use up_transport_zenoh::ZenohZeroCopyCore;

#[derive(Default)]
struct EmptySubscription;

#[async_trait]
impl USubscription for EmptySubscription {
    async fn subscribe(
        &self,
        _topic: &UUri,
        _expiration: Option<u64>,
        _min_sample_period: Option<u32>,
    ) -> Result<SubscriptionStatus, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "subscribe is not used by this test",
        ))
    }

    async fn unsubscribe(&self, _topic: &UUri) -> Result<(), UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "unsubscribe is not used by this test",
        ))
    }

    async fn fetch_subscriptions_by_topic(
        &self,
        _topic: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Ok(Vec::new())
    }

    async fn fetch_subscriptions_by_subscriber(
        &self,
        _subscriber: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "fetch_subscriptions_by_subscriber is not used by this test",
        ))
    }

    async fn register_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn unregister_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn fetch_subscribers(&self, _topic: &UUri) -> Result<Vec<UUri>, UStatus> {
        Ok(Vec::new())
    }

    async fn reset(&self, _reason: ResetReason, _message: Option<String>) -> Result<(), UStatus> {
        Ok(())
    }
}

struct StaticSubscription {
    subscriptions: Vec<SubscriptionInfo>,
}

impl StaticSubscription {
    fn new(subscriptions: Vec<SubscriptionInfo>) -> Self {
        Self { subscriptions }
    }
}

#[async_trait]
impl USubscription for StaticSubscription {
    async fn subscribe(
        &self,
        _topic: &UUri,
        _expiration: Option<u64>,
        _min_sample_period: Option<u32>,
    ) -> Result<SubscriptionStatus, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "subscribe is not used by this test",
        ))
    }

    async fn unsubscribe(&self, _topic: &UUri) -> Result<(), UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "unsubscribe is not used by this test",
        ))
    }

    async fn fetch_subscriptions_by_topic(
        &self,
        _topic: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Ok(self.subscriptions.clone())
    }

    async fn fetch_subscriptions_by_subscriber(
        &self,
        _subscriber: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "fetch_subscriptions_by_subscriber is not used by this test",
        ))
    }

    async fn register_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn unregister_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn fetch_subscribers(&self, _topic: &UUri) -> Result<Vec<UUri>, UStatus> {
        Ok(Vec::new())
    }

    async fn reset(&self, _reason: ResetReason, _message: Option<String>) -> Result<(), UStatus> {
        Ok(())
    }
}

fn subscription(topic: &str, subscriber: &str) -> SubscriptionInfo {
    SubscriptionInfo::new(
        topic.parse::<UUri>().expect("valid topic URI"),
        subscriber.parse::<UUri>().expect("valid subscriber URI"),
        SubscriptionStatus::Subscribed,
        None,
        None,
    )
}

fn seeded_publish_subscription() -> Arc<dyn USubscription> {
    Arc::new(StaticSubscription::new(vec![subscription(
        "//authority-a/5BA0/1/8001",
        "//authority-b/5678/1/1234",
    )]))
}

fn payload_frame(payload: &'static [u8]) -> UVecRxLease {
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(Bytes::from_static(payload), UPayloadFormat::Protobuf)
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UVecRxLease::new(metadata, Some(payload.to_vec())).expect("zero-copy frame")
}

#[derive(Default)]
struct RouteInstrumentation {
    payload_slices_calls: AtomicUsize,
    route_payload_slices_calls: AtomicUsize,
    route_payload_slice_items: AtomicUsize,
    payload_reader_calls: AtomicUsize,
    try_contiguous_payload_calls: AtomicUsize,
    tx_payload_mut_calls: AtomicUsize,
    loan_tx_calls: AtomicUsize,
    send_calls: AtomicUsize,
}

#[derive(Clone)]
struct InstrumentedRxLease {
    metadata: UFrameMetadata,
    slices: Vec<Vec<u8>>,
    instrumentation: Arc<RouteInstrumentation>,
}

struct InstrumentedPayloadSlices<'a> {
    iter: std::slice::Iter<'a, Vec<u8>>,
    instrumentation: Arc<RouteInstrumentation>,
    route_observation: bool,
}

impl<'a> Iterator for InstrumentedPayloadSlices<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let slice = self.iter.next()?;
        if self.route_observation {
            self.instrumentation
                .route_payload_slice_items
                .fetch_add(1, Ordering::SeqCst);
        }
        Some(slice.as_slice())
    }
}

impl UFrameView for InstrumentedRxLease {
    type PayloadReader<'a>
        = Cursor<&'a [u8]>
    where
        Self: 'a;
    type PayloadSlices<'a>
        = InstrumentedPayloadSlices<'a>
    where
        Self: 'a;

    fn metadata(&self) -> &UFrameMetadata {
        &self.metadata
    }

    fn payload_len(&self) -> usize {
        self.slices.iter().map(Vec::len).sum()
    }

    fn has_payload(&self) -> bool {
        !self.slices.is_empty()
    }

    fn payload_reader(&self) -> Self::PayloadReader<'_> {
        self.instrumentation
            .payload_reader_calls
            .fetch_add(1, Ordering::SeqCst);
        Cursor::new(&[])
    }

    fn payload_slices(&self) -> Self::PayloadSlices<'_> {
        let call = self
            .instrumentation
            .payload_slices_calls
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        let route_observation = call > 1;
        if route_observation {
            self.instrumentation
                .route_payload_slices_calls
                .fetch_add(1, Ordering::SeqCst);
        }
        InstrumentedPayloadSlices {
            iter: self.slices.iter(),
            instrumentation: self.instrumentation.clone(),
            route_observation,
        }
    }

    fn try_contiguous_payload(&self) -> Option<&[u8]> {
        self.instrumentation
            .try_contiguous_payload_calls
            .fetch_add(1, Ordering::SeqCst);
        None
    }
}

impl UZeroCopyRxLease for InstrumentedRxLease {}

struct InstrumentedTxBuffer {
    metadata: UFrameMetadata,
    encoded_metadata: Option<Vec<u8>>,
    payload: Vec<u8>,
    instrumentation: Arc<RouteInstrumentation>,
}

impl UTxBuffer for InstrumentedTxBuffer {
    fn metadata(&self) -> &UFrameMetadata {
        &self.metadata
    }

    fn payload(&self) -> &[u8] {
        &self.payload
    }

    fn payload_mut(&mut self) -> &mut [u8] {
        self.instrumentation
            .tx_payload_mut_calls
            .fetch_add(1, Ordering::SeqCst);
        &mut self.payload
    }
}

#[derive(Default)]
struct InstrumentedTransportState {
    listeners: Vec<Arc<dyn UZeroCopyListener<InstrumentedRxLease>>>,
    sent_payloads: Vec<Vec<u8>>,
    loan_specs: Vec<(usize, usize)>,
    register_calls: usize,
    unregister_calls: usize,
}

#[derive(Default)]
struct SelectedWireCoreState {
    listeners: Vec<Arc<dyn UEncodedZeroCopyListener<EncodedRxLease>>>,
    registered_filters: Vec<(UUri, Option<UUri>)>,
    sent_payloads: Vec<Vec<u8>>,
    sent_metadata: Vec<UFrameMetadata>,
    loan_specs: Vec<(usize, usize)>,
}

#[derive(Clone)]
struct InstrumentedZeroCopyTransport {
    instrumentation: Arc<RouteInstrumentation>,
    state: Arc<Mutex<InstrumentedTransportState>>,
    fail_register_after: Option<usize>,
    fail_unregister: bool,
}

impl InstrumentedZeroCopyTransport {
    fn new(instrumentation: Arc<RouteInstrumentation>) -> Self {
        Self {
            instrumentation,
            state: Arc::new(Mutex::new(InstrumentedTransportState::default())),
            fail_register_after: None,
            fail_unregister: false,
        }
    }

    fn fail_register_after(mut self, successful_registrations: usize) -> Self {
        self.fail_register_after = Some(successful_registrations);
        self
    }

    fn fail_unregister(mut self) -> Self {
        self.fail_unregister = true;
        self
    }

    async fn inject(&self, frame: InstrumentedRxLease) {
        let listeners = self
            .state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .listeners
            .clone();
        for listener in listeners {
            listener.on_receive_zero_copy(frame.clone()).await;
        }
    }

    fn sent_payloads(&self) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .sent_payloads
            .clone()
    }

    fn loan_specs(&self) -> Vec<(usize, usize)> {
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .loan_specs
            .clone()
    }

    fn listener_count(&self) -> usize {
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .listeners
            .len()
    }

    fn unregister_calls(&self) -> usize {
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .unregister_calls
    }
}

#[async_trait]
impl UZeroCopyTransportImpl for InstrumentedZeroCopyTransport {
    type Tx = InstrumentedTxBuffer;
    type Rx = InstrumentedRxLease;

    async fn loan_validated_tx(&self, spec: ValidatedTxLoanSpec) -> Result<Self::Tx, UStatus> {
        self.instrumentation
            .loan_tx_calls
            .fetch_add(1, Ordering::SeqCst);
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .loan_specs
            .push((spec.payload_len(), spec.payload_alignment()));
        Ok(InstrumentedTxBuffer {
            metadata: spec.metadata().clone(),
            encoded_metadata: None,
            payload: vec![0; spec.payload_len()],
            instrumentation: self.instrumentation.clone(),
        })
    }

    async fn send_validated_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
        self.instrumentation
            .send_calls
            .fetch_add(1, Ordering::SeqCst);
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .sent_payloads
            .push(buffer.payload);
        Ok(())
    }

    async fn register_validated_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        let mut state = self
            .state
            .lock()
            .expect("instrumented zero-copy state lock poisoned");
        if self
            .fail_register_after
            .is_some_and(|limit| state.register_calls >= limit)
        {
            return Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "injected register failure",
            ));
        }
        state.register_calls += 1;
        state.listeners.push(listener);
        Ok(())
    }

    async fn unregister_validated_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        let mut state = self
            .state
            .lock()
            .expect("instrumented zero-copy state lock poisoned");
        state.unregister_calls += 1;
        if self.fail_unregister {
            return Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "injected unregister failure",
            ));
        }
        let Some(index) = state
            .listeners
            .iter()
            .position(|registered| Arc::ptr_eq(registered, &listener))
        else {
            return Err(UStatus::fail_with_code(
                UCode::NotFound,
                "instrumented zero-copy listener not registered",
            ));
        };
        state.listeners.remove(index);
        Ok(())
    }
}

#[derive(Clone)]
struct EncodedRxLease {
    encoded_metadata: Vec<u8>,
    slices: Vec<Vec<u8>>,
}

impl UEncodedRxFrame for EncodedRxLease {
    type PayloadReader<'a>
        = Cursor<Vec<u8>>
    where
        Self: 'a;
    type PayloadSlices<'a>
        = std::iter::Map<std::slice::Iter<'a, Vec<u8>>, fn(&'a Vec<u8>) -> &'a [u8]>
    where
        Self: 'a;

    fn encoded_metadata(&self) -> &[u8] {
        &self.encoded_metadata
    }

    fn payload_len(&self) -> usize {
        self.slices.iter().map(Vec::len).sum()
    }

    fn payload_reader(&self) -> Self::PayloadReader<'_> {
        Cursor::new(self.slices.concat())
    }

    fn payload_slices(&self) -> Self::PayloadSlices<'_> {
        fn as_slice(value: &Vec<u8>) -> &[u8] {
            value.as_slice()
        }

        self.slices.iter().map(as_slice)
    }
}

#[derive(Clone, Default)]
struct SelectedWireCore {
    instrumentation: Arc<RouteInstrumentation>,
    state: Arc<Mutex<SelectedWireCoreState>>,
    dispatch_sent: bool,
}

impl SelectedWireCore {
    fn new(instrumentation: Arc<RouteInstrumentation>) -> Self {
        Self {
            instrumentation,
            state: Arc::new(Mutex::new(SelectedWireCoreState::default())),
            dispatch_sent: false,
        }
    }

    fn dispatch_sent(instrumentation: Arc<RouteInstrumentation>) -> Self {
        Self {
            dispatch_sent: true,
            ..Self::new(instrumentation)
        }
    }

    async fn inject_encoded(&self, frame: EncodedRxLease) {
        let listeners = self
            .state
            .lock()
            .expect("selected-wire core state lock poisoned")
            .listeners
            .clone();
        for listener in listeners {
            listener.on_receive_encoded_zero_copy(frame.clone()).await;
        }
    }

    fn sent_payloads(&self) -> Vec<Vec<u8>> {
        self.state
            .lock()
            .expect("selected-wire core state lock poisoned")
            .sent_payloads
            .clone()
    }

    fn sent_metadata(&self) -> Vec<UFrameMetadata> {
        self.state
            .lock()
            .expect("selected-wire core state lock poisoned")
            .sent_metadata
            .clone()
    }

    fn loan_specs(&self) -> Vec<(usize, usize)> {
        self.state
            .lock()
            .expect("selected-wire core state lock poisoned")
            .loan_specs
            .clone()
    }

    fn registered_filters(&self) -> Vec<(UUri, Option<UUri>)> {
        self.state
            .lock()
            .expect("selected-wire core state lock poisoned")
            .registered_filters
            .clone()
    }
}

#[async_trait]
impl UZeroCopyTransportCore for SelectedWireCore {
    type Tx = InstrumentedTxBuffer;
    type Rx = EncodedRxLease;

    async fn loan_prepared_tx(&self, spec: PreparedTxLoanSpec) -> Result<Self::Tx, UStatus> {
        self.instrumentation
            .loan_tx_calls
            .fetch_add(1, Ordering::SeqCst);
        self.state
            .lock()
            .expect("selected-wire core state lock poisoned")
            .loan_specs
            .push((spec.payload_len(), spec.payload_alignment()));
        Ok(InstrumentedTxBuffer {
            metadata: spec.metadata().clone(),
            encoded_metadata: Some(spec.encoded_metadata().to_vec()),
            payload: vec![0; spec.payload_len()],
            instrumentation: self.instrumentation.clone(),
        })
    }

    async fn send_prepared_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
        self.instrumentation
            .send_calls
            .fetch_add(1, Ordering::SeqCst);
        let frame = self.dispatch_sent.then(|| EncodedRxLease {
            encoded_metadata: buffer
                .encoded_metadata
                .clone()
                .expect("selected-wire TX buffer carries encoded metadata"),
            slices: vec![buffer.payload.clone()],
        });
        let listeners = {
            let mut state = self
                .state
                .lock()
                .expect("selected-wire core state lock poisoned");
            state.sent_metadata.push(buffer.metadata);
            state.sent_payloads.push(buffer.payload);
            if frame.is_some() {
                state.listeners.clone()
            } else {
                Vec::new()
            }
        };
        if let Some(frame) = frame {
            for listener in listeners {
                listener.on_receive_encoded_zero_copy(frame.clone()).await;
            }
        }
        Ok(())
    }

    async fn register_encoded_zero_copy_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UEncodedZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        let mut state = self
            .state
            .lock()
            .expect("selected-wire core state lock poisoned");
        state
            .registered_filters
            .push((source_filter.clone(), sink_filter.cloned()));
        state.listeners.push(listener);
        Ok(())
    }

    async fn unregister_encoded_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UEncodedZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        let mut state = self
            .state
            .lock()
            .expect("selected-wire core state lock poisoned");
        let Some(index) = state
            .listeners
            .iter()
            .position(|registered| Arc::ptr_eq(registered, &listener))
        else {
            return Err(UStatus::fail_with_code(
                UCode::NotFound,
                "selected-wire listener not registered",
            ));
        };
        state.listeners.remove(index);
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AlternateSelectedWireCore(SelectedWireCore);

impl AlternateSelectedWireCore {
    fn new(instrumentation: Arc<RouteInstrumentation>) -> Self {
        Self(SelectedWireCore::new(instrumentation))
    }

    fn sent_payloads(&self) -> Vec<Vec<u8>> {
        self.0.sent_payloads()
    }

    fn sent_metadata(&self) -> Vec<UFrameMetadata> {
        self.0.sent_metadata()
    }

    fn loan_specs(&self) -> Vec<(usize, usize)> {
        self.0.loan_specs()
    }
}

#[async_trait]
impl UZeroCopyTransportCore for AlternateSelectedWireCore {
    type Tx = InstrumentedTxBuffer;
    type Rx = EncodedRxLease;

    async fn loan_prepared_tx(&self, spec: PreparedTxLoanSpec) -> Result<Self::Tx, UStatus> {
        self.0.loan_prepared_tx(spec).await
    }

    async fn send_prepared_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
        self.0.send_prepared_zero_copy(buffer).await
    }

    async fn register_encoded_zero_copy_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UEncodedZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.0
            .register_encoded_zero_copy_listener(source_filter, sink_filter, listener)
            .await
    }

    async fn unregister_encoded_zero_copy_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UEncodedZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.0
            .unregister_encoded_zero_copy_listener(source_filter, sink_filter, listener)
            .await
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StableBytes {
    bytes: [u8; 4],
}

unsafe impl StablePayload for StableBytes {
    const TYPE_NAME: &'static str = "up_streamer.tests.StableBytes";
}

unsafe impl ByteBackedStablePayload for StableBytes {}

fn stable_uri_provider(authority: &str) -> Arc<StaticUriProvider> {
    Arc::new(StaticUriProvider::new(authority, 0x5BA0, 0x01).expect("uri provider"))
}

fn authority_wildcard_filter(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0xFFFF_FFFF, 0xFF, 0xFFFF).expect("authority wildcard filter")
}

fn instrumented_payload_frame(
    payload_slices: &[&'static [u8]],
    instrumentation: Arc<RouteInstrumentation>,
) -> InstrumentedRxLease {
    let payload_len: usize = payload_slices.iter().map(|slice| slice.len()).sum();
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(Bytes::from(vec![0; payload_len]), UPayloadFormat::Protobuf)
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    InstrumentedRxLease {
        metadata,
        slices: payload_slices.iter().map(|slice| slice.to_vec()).collect(),
        instrumentation,
    }
}

fn selected_wire_payload_frame<W>(payload_slices: &[&'static [u8]]) -> EncodedRxLease
where
    W: UWire,
{
    let payload_len: usize = payload_slices.iter().map(|slice| slice.len()).sum();
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(Bytes::from(vec![0; payload_len]), UPayloadFormat::Protobuf)
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    EncodedRxLease {
        encoded_metadata: NativePrefixFrameMetadataCodec
            .encode_frame_metadata(W::metadata_context(), &metadata)
            .expect("encoded metadata"),
        slices: payload_slices.iter().map(|slice| slice.to_vec()).collect(),
    }
}

fn no_payload_frame() -> UVecRxLease {
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build()
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UVecRxLease::new(metadata, None).expect("zero-copy frame")
}

fn present_empty_payload_frame() -> UVecRxLease {
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(Bytes::new(), UPayloadFormat::Protobuf)
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UVecRxLease::new(metadata, Some(Vec::new())).expect("zero-copy frame")
}

async fn wait_for_sent_count(transport: &InMemoryZeroCopyTransport, expected: usize) {
    for _ in 0..20 {
        if transport.sent_frames().len() >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_instrumented_sent_count(
    transport: &InstrumentedZeroCopyTransport,
    expected: usize,
) {
    for _ in 0..20 {
        if transport.sent_payloads().len() >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_selected_wire_sent_count(transport: &SelectedWireCore, expected: usize) {
    for _ in 0..20 {
        if transport.sent_payloads().len() >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

fn assert_route_pair<I, E>()
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxLease + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    let _ = std::mem::size_of::<ZeroCopyFrameEndpoint<I>>();
    let _ = std::mem::size_of::<ZeroCopyFrameEndpoint<E>>();
}

#[test]
fn selected_zenoh_iceoryx2_route_pairs_typecheck() {
    type Zenoh = ProtobufWireTransport<ZenohZeroCopyCore>;
    type Iceoryx2 = ProtobufWireTransport<Iceoryx2PubSub>;

    assert_route_pair::<Zenoh, Zenoh>();
    assert_route_pair::<Zenoh, Iceoryx2>();
    assert_route_pair::<Iceoryx2, Zenoh>();
    assert_route_pair::<Iceoryx2, Iceoryx2>();
}

#[tokio::test]
async fn copy_minimized_route_copies_payload_once_into_egress_loan() {
    let ingress = Arc::new(InMemoryZeroCopyTransport::default());
    let egress = Arc::new(InMemoryZeroCopyTransport::default());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("copy-minimized-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_copy_minimized_route_ref_with_options(
            &ingress_endpoint,
            &egress_endpoint,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .await
        .expect("copy-minimized route add");

    let diagnostics = streamer.route_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].route.ingress_name, "ingress");
    assert_eq!(diagnostics[0].route.ingress_authority, "authority-a");
    assert_eq!(diagnostics[0].route.egress_name, "egress");
    assert_eq!(diagnostics[0].route.egress_authority, "authority-b");
    assert_eq!(diagnostics[0].route_kind, RouteKind::CopyMinimized);
    assert_eq!(
        diagnostics[0].copy_semantics,
        RouteCopySemantics::CopyMinimizedOneCopy
    );

    ingress.inject(payload_frame(b"copy-minimized")).await;
    wait_for_sent_count(&egress, 1).await;

    let sent = egress.sent_frames();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].try_contiguous_payload(),
        Some(&b"copy-minimized"[..])
    );

    streamer
        .delete_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route delete");
    assert!(streamer.route_diagnostics().is_empty());

    ingress.inject(payload_frame(b"after-delete")).await;
    sleep(Duration::from_millis(20)).await;
    assert_eq!(egress.sent_frames().len(), 1);
}

#[tokio::test]
async fn copy_minimized_route_uses_ordered_slices_without_owned_materialization() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress = Arc::new(InstrumentedZeroCopyTransport::new(instrumentation.clone()));
    let egress = Arc::new(InstrumentedZeroCopyTransport::new(instrumentation.clone()));
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("copy-minimized-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_copy_minimized_route_ref_with_options(
            &ingress_endpoint,
            &egress_endpoint,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .await
        .expect("copy-minimized route add");

    ingress
        .inject(instrumented_payload_frame(
            &[b"copy-", b"min", b"imized"],
            instrumentation.clone(),
        ))
        .await;
    wait_for_instrumented_sent_count(&egress, 1).await;

    assert_eq!(egress.sent_payloads(), vec![b"copy-minimized".to_vec()]);
    assert_eq!(egress.loan_specs(), vec![(b"copy-minimized".len(), 8)]);
    assert_eq!(instrumentation.loan_tx_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        instrumentation.tx_payload_mut_calls.load(Ordering::SeqCst),
        1
    );
    assert_eq!(instrumentation.send_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        instrumentation
            .route_payload_slices_calls
            .load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        instrumentation
            .route_payload_slice_items
            .load(Ordering::SeqCst),
        3
    );
    assert_eq!(
        instrumentation.payload_reader_calls.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        instrumentation
            .try_contiguous_payload_calls
            .load(Ordering::SeqCst),
        0
    );

    streamer
        .delete_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route delete");
}

#[tokio::test]
async fn copy_minimized_route_rejects_duplicate_same_authority_and_missing_delete() {
    let ingress = Arc::new(InMemoryZeroCopyTransport::default());
    let egress = Arc::new(InMemoryZeroCopyTransport::default());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let same_authority_endpoint =
        ZeroCopyFrameEndpoint::new("same-authority", "authority-a", egress.clone());
    let mut streamer = UStreamer::new("copy-minimized-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    let same_authority_error = streamer
        .add_copy_minimized_route_ref(&ingress_endpoint, &same_authority_endpoint)
        .await
        .expect_err("same-authority route should fail");
    assert_eq!(same_authority_error.get_code(), UCode::InvalidArgument);

    streamer
        .add_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route add");
    let duplicate_error = streamer
        .add_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect_err("duplicate route should fail");
    assert_eq!(duplicate_error.get_code(), UCode::AlreadyExists);

    streamer
        .delete_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route delete");
    let missing_delete_error = streamer
        .delete_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect_err("missing route delete should fail");
    assert_eq!(missing_delete_error.get_code(), UCode::NotFound);
}

#[tokio::test]
async fn copy_minimized_route_preserves_no_payload_and_present_empty_payload() {
    let ingress = Arc::new(InMemoryZeroCopyTransport::default());
    let egress = Arc::new(InMemoryZeroCopyTransport::default());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("copy-minimized-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route add");

    ingress.inject(no_payload_frame()).await;
    ingress.inject(present_empty_payload_frame()).await;
    wait_for_sent_count(&egress, 2).await;

    let sent = egress.sent_frames();
    assert_eq!(sent.len(), 2);
    assert!(!sent[0].has_payload());
    assert_eq!(sent[0].payload_len(), 0);
    assert!(sent[1].has_payload());
    assert_eq!(sent[1].payload_len(), 0);
}

#[tokio::test]
async fn copy_minimized_route_rolls_back_after_partial_registration_failure() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress = Arc::new(
        InstrumentedZeroCopyTransport::new(instrumentation.clone()).fail_register_after(1),
    );
    let egress = Arc::new(InstrumentedZeroCopyTransport::new(instrumentation));
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let subscriptions = vec![subscription(
        "//authority-a/5BA0/1/8001",
        "//authority-b/5678/1/1234",
    )];
    let mut streamer = UStreamer::new(
        "copy-minimized-route",
        4,
        Arc::new(StaticSubscription::new(subscriptions)),
    )
    .await
    .expect("streamer");

    let error = streamer
        .add_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect_err("partial registration failure should fail route add");

    assert_eq!(error.get_code(), UCode::Unavailable);
    assert_eq!(ingress.listener_count(), 0);
    assert_eq!(ingress.unregister_calls(), 1);
    assert!(streamer.route_diagnostics().is_empty());
}

#[tokio::test]
async fn copy_minimized_route_delete_failure_keeps_route_registered() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress =
        Arc::new(InstrumentedZeroCopyTransport::new(instrumentation.clone()).fail_unregister());
    let egress = Arc::new(InstrumentedZeroCopyTransport::new(instrumentation));
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("copy-minimized-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route add");

    let error = streamer
        .delete_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect_err("injected unregister failure should fail delete");

    assert_eq!(error.get_code(), UCode::Unavailable);
    assert_eq!(streamer.route_diagnostics().len(), 1);
    assert_eq!(ingress.listener_count(), 1);

    ingress
        .inject(instrumented_payload_frame(
            &[b"still-", b"registered"],
            Arc::new(RouteInstrumentation::default()),
        ))
        .await;
    wait_for_instrumented_sent_count(&egress, 1).await;
    assert_eq!(egress.sent_payloads(), vec![b"still-registered".to_vec()]);
}

#[tokio::test]
async fn selected_wire_copy_minimized_route_accepts_same_static_wire() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress_core = SelectedWireCore::new(instrumentation.clone());
    let egress_core = SelectedWireCore::new(instrumentation);
    let ingress = Arc::new(ingress_core.clone().into_protobuf_transport());
    let egress = Arc::new(egress_core.clone().into_protobuf_transport());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress);
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress);
    let mut streamer = UStreamer::new("selected-wire-route", 4, seeded_publish_subscription())
        .await
        .expect("streamer");

    streamer
        .add_selected_wire_copy_minimized_route_ref_with_options(
            &ingress_endpoint,
            &egress_endpoint,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .await
        .expect("selected-wire route add");
    let registered_filters = ingress_core.registered_filters();
    assert!(
        registered_filters.contains(&(
            authority_wildcard_filter("*"),
            Some(authority_wildcard_filter("authority-b")),
        )),
        "selected-wire registration must preserve sink-bearing route filters"
    );
    assert!(
        registered_filters.iter().any(|(_, sink)| sink.is_none()),
        "publish subscription should add a source-only listener filter"
    );

    ingress_core
        .inject_encoded(selected_wire_payload_frame::<ProtobufWire>(&[
            b"selected-",
            b"wire",
        ]))
        .await;
    wait_for_selected_wire_sent_count(&egress_core, 1).await;

    assert_eq!(egress_core.sent_payloads(), vec![b"selected-wire".to_vec()]);
    assert_eq!(egress_core.loan_specs(), vec![(b"selected-wire".len(), 8)]);
    assert_eq!(egress_core.sent_metadata().len(), 1);

    streamer
        .delete_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("selected-wire route delete");
}

#[tokio::test]
async fn zero_copy_l2_stable_publish_routes_through_streamer() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress_core = SelectedWireCore::dispatch_sent(instrumentation.clone());
    let egress_core = SelectedWireCore::new(instrumentation);
    let ingress = Arc::new(ingress_core.into_stable_container_transport());
    let egress = Arc::new(egress_core.clone().into_stable_container_transport());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress);
    let mut streamer = UStreamer::new(
        "zero-copy-l2-stable-publish",
        4,
        seeded_publish_subscription(),
    )
    .await
    .expect("streamer");
    let publisher =
        zero_copy::Endpoint::new(ingress, stable_uri_provider("authority-a")).publisher();

    streamer
        .add_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("selected-wire stable route add");
    publisher
        .publish_stable::<StableBytes>(
            0x8001,
            CallOptions::for_publish(None, None, None),
            |payload| payload.bytes.copy_from_slice(b"zcpy"),
        )
        .await
        .expect("stable publish succeeds");
    wait_for_selected_wire_sent_count(&egress_core, 1).await;

    assert_eq!(egress_core.sent_payloads(), vec![b"zcpy".to_vec()]);
    assert_eq!(
        egress_core.loan_specs(),
        vec![(
            std::mem::size_of::<StableBytes>(),
            std::mem::align_of::<StableBytes>()
        )]
    );
    assert_eq!(egress_core.sent_metadata().len(), 1);

    streamer
        .delete_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("selected-wire stable route delete");
}

#[tokio::test]
async fn zero_copy_l2_stable_publish_routes_across_heterogeneous_selected_wire_cores() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress_core = SelectedWireCore::dispatch_sent(instrumentation.clone());
    let egress_core = AlternateSelectedWireCore::new(instrumentation);
    let ingress = Arc::new(ingress_core.into_stable_container_transport());
    let egress = Arc::new(egress_core.clone().into_stable_container_transport());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress);
    let mut streamer = UStreamer::new(
        "zero-copy-l2-cross-transport",
        4,
        seeded_publish_subscription(),
    )
    .await
    .expect("streamer");
    let publisher =
        zero_copy::Endpoint::new(ingress, stable_uri_provider("authority-a")).publisher();

    streamer
        .add_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("heterogeneous selected-wire route add");
    let diagnostics = streamer.route_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].route_kind, RouteKind::CopyMinimized);
    assert_eq!(
        diagnostics[0].copy_semantics,
        RouteCopySemantics::CopyMinimizedOneCopy
    );

    publisher
        .publish_stable::<StableBytes>(
            0x8001,
            CallOptions::for_publish(None, None, None),
            |payload| payload.bytes.copy_from_slice(b"xprt"),
        )
        .await
        .expect("stable publish succeeds");
    wait_for_selected_wire_sent_count(&egress_core.0, 1).await;

    assert_eq!(egress_core.sent_payloads(), vec![b"xprt".to_vec()]);
    assert_eq!(
        egress_core.loan_specs(),
        vec![(
            std::mem::size_of::<StableBytes>(),
            std::mem::align_of::<StableBytes>()
        )]
    );
    assert_eq!(egress_core.sent_metadata().len(), 1);

    streamer
        .delete_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("heterogeneous selected-wire route delete");
}

#[tokio::test]
async fn selected_wire_copy_minimized_route_drops_mismatched_wire_before_forwarding() {
    let instrumentation = Arc::new(RouteInstrumentation::default());
    let ingress_core = SelectedWireCore::new(instrumentation.clone());
    let egress_core = SelectedWireCore::new(instrumentation);
    let ingress = Arc::new(ingress_core.clone().into_protobuf_transport());
    let egress = Arc::new(egress_core.clone().into_protobuf_transport());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress);
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress);
    let mut streamer = UStreamer::new("selected-wire-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("selected-wire route add");

    ingress_core
        .inject_encoded(selected_wire_payload_frame::<UProtocolNativeWire>(&[
            b"wrong-wire",
        ]))
        .await;
    sleep(Duration::from_millis(20)).await;

    assert!(egress_core.sent_payloads().is_empty());
    assert!(egress_core.loan_specs().is_empty());
    assert_eq!(egress_core.sent_metadata().len(), 0);

    streamer
        .delete_selected_wire_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("selected-wire route delete");
}
