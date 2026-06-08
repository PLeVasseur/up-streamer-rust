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
use up_rust::core::usubscription::{
    ResetReason, SubscriptionInfo, SubscriptionStatus, USubscription,
};
use up_rust::{
    try_project_umessage_to_frame_metadata, InMemoryZeroCopyTransport, UCode, UFrameMetadata,
    UFrameView, UMessageBuilder, UPayloadFormat, UStatus, UTxBuffer, UUri, UVecRxLease,
    UZeroCopyListener, UZeroCopyTransportImpl, ValidatedTxLoanSpec,
};
use up_rust::{UZeroCopyRxLease, UZeroCopyTransport};
use up_streamer::{
    CopyMinimizedRouteOptions, RouteCopySemantics, RouteKind, UStreamer, ZeroCopyFrameEndpoint,
};
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
use up_transport_zenoh::UPTransportZenoh;

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

    async fn reset(
        &self,
        _reason: ResetReason,
        _message: Option<String>,
        _before: Option<u64>,
    ) -> Result<(), UStatus> {
        Ok(())
    }
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
}

#[derive(Clone)]
struct InstrumentedZeroCopyTransport {
    instrumentation: Arc<RouteInstrumentation>,
    state: Arc<Mutex<InstrumentedTransportState>>,
}

impl InstrumentedZeroCopyTransport {
    fn new(instrumentation: Arc<RouteInstrumentation>) -> Self {
        Self {
            instrumentation,
            state: Arc::new(Mutex::new(InstrumentedTransportState::default())),
        }
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
        self.state
            .lock()
            .expect("instrumented zero-copy state lock poisoned")
            .listeners
            .push(listener);
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
    assert_route_pair::<UPTransportZenoh, UPTransportZenoh>();
    assert_route_pair::<UPTransportZenoh, Iceoryx2PubSub>();
    assert_route_pair::<Iceoryx2PubSub, UPTransportZenoh>();
    assert_route_pair::<Iceoryx2PubSub, Iceoryx2PubSub>();
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
