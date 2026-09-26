#![cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "owned-frame-transport"
))]

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};
use up_rust::communication::SubscriptionStatus;
use up_rust::core::usubscription::{ResetReason, SubscriptionInfo, USubscription};
use up_rust::frame::metadata::try_project_umessage_to_frame_metadata;
use up_rust::{
    PayloadEncoding, UCode, UFrameView, UMessageBuilder, UOwnedFrame, UOwnedListener,
    UOwnedTransportImpl, UStatus, UTxLoanSpec, UUri, UVecRxLease, UVecTxBuffer, UZeroCopyListener,
    UZeroCopyTransportImpl,
};
use up_streamer::{
    CopyMinimizedRouteOptions, OwnedFrameEndpoint, RouteCopySemantics, RouteKind, UStreamer,
    ZeroCopyFrameEndpoint,
};

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

#[derive(Default)]
struct RecordingOwnedTransport {
    sent: Mutex<Vec<UOwnedFrame>>,
    listeners: Mutex<Vec<Arc<dyn UOwnedListener>>>,
}

#[derive(Default)]
struct RecordingZeroCopyTransport {
    sent: Mutex<Vec<UVecRxLease>>,
    listeners: Mutex<Vec<Arc<dyn UZeroCopyListener<UVecRxLease>>>>,
}

impl RecordingZeroCopyTransport {
    fn sent_frames(&self) -> Vec<UVecRxLease> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn inject(&self, frame: UVecRxLease) {
        let listeners = self.listeners.lock().expect("listener lock").clone();
        for listener in listeners {
            listener.on_receive_zero_copy(frame.clone()).await;
        }
    }
}

#[async_trait]
impl UZeroCopyTransportImpl for RecordingZeroCopyTransport {
    type Tx = UVecTxBuffer;
    type Rx = UVecRxLease;

    async fn loan_validated_tx(&self, spec: UTxLoanSpec) -> Result<Self::Tx, UStatus> {
        UVecTxBuffer::with_alignment(
            spec.metadata().clone(),
            spec.payload_len(),
            spec.payload_alignment_proof().as_usize(),
        )
    }

    async fn send_validated_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
        let payload = buffer
            .metadata()
            .payload_encoding()
            .is_some()
            .then(|| buffer.payload().to_vec());
        let frame = UVecRxLease::new(buffer.metadata().clone(), payload)?;
        self.sent.lock().expect("sent lock").push(frame);
        Ok(())
    }

    async fn receive_validated_zero_copy(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
    ) -> Result<Self::Rx, UStatus> {
        Err(UStatus::fail_with_code(UCode::NotFound, "unused"))
    }

    async fn register_validated_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.listeners.lock().expect("listener lock").push(listener);
        Ok(())
    }

    async fn unregister_validated_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listener lock")
            .retain(|registered| !Arc::ptr_eq(registered, &listener));
        Ok(())
    }
}

impl RecordingOwnedTransport {
    fn sent_frames(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self.listeners.lock().expect("listener lock").clone();
        for listener in listeners {
            listener.on_receive_owned(frame.clone()).await;
        }
    }
}

#[async_trait]
impl UOwnedTransportImpl for RecordingOwnedTransport {
    async fn send_validated_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
        self.sent.lock().expect("sent lock").push(frame);
        Ok(())
    }

    async fn receive_validated_owned(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
    ) -> Result<UOwnedFrame, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::NotFound,
            "receive is not used by this test",
        ))
    }

    async fn register_validated_owned_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners.lock().expect("listener lock").push(listener);
        Ok(())
    }

    async fn unregister_validated_owned_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listener lock")
            .retain(|registered| !Arc::ptr_eq(registered, &listener));
        Ok(())
    }
}

fn topic() -> UUri {
    UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic")
}

fn owned_payload_frame(payload: &'static [u8]) -> UOwnedFrame {
    let message = UMessageBuilder::publish(topic())
        .build_with_payload(Bytes::from_static(payload), PayloadEncoding::PROTOBUF)
        .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UOwnedFrame::with_payload(metadata, Bytes::from_static(payload)).expect("owned frame")
}

fn zero_copy_payload_frame(payload: &'static [u8]) -> UVecRxLease {
    let message = UMessageBuilder::publish(topic())
        .build_with_payload(Bytes::from_static(payload), PayloadEncoding::PROTOBUF)
        .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UVecRxLease::new(metadata, Some(payload.to_vec())).expect("zero-copy frame")
}

async fn wait_for_zero_copy_sent_count(transport: &RecordingZeroCopyTransport, expected: usize) {
    for _ in 0..20 {
        if transport.sent_frames().len() >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_owned_sent_count(transport: &RecordingOwnedTransport, expected: usize) {
    for _ in 0..20 {
        if transport.sent_frames().len() >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn owned_to_copy_minimized_adapter_copies_payload_into_egress_loan() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingZeroCopyTransport::default());
    let ingress_endpoint = OwnedFrameEndpoint::from_owned("owned", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("copy", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("owned-to-copy", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_owned_to_copy_minimized_route_ref_with_options(
            &ingress_endpoint,
            &egress_endpoint,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .await
        .expect("adapter route add");

    let diagnostics = streamer.route_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].route_kind, RouteKind::AdapterBacked);
    assert_eq!(
        diagnostics[0].copy_semantics,
        RouteCopySemantics::AdapterBoundaryCopying
    );

    let frame = owned_payload_frame(b"owned-copy");
    ingress.inject(frame.clone()).await;
    wait_for_zero_copy_sent_count(&egress, 1).await;

    let sent = egress.sent_frames();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].metadata(), frame.metadata());
    assert_eq!(sent[0].try_contiguous_payload(), Some(&b"owned-copy"[..]));

    streamer
        .delete_owned_to_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("adapter route delete");
    assert!(streamer.route_diagnostics().is_empty());
}

#[tokio::test]
async fn copy_minimized_to_owned_adapter_coalesces_payload_slices() {
    let ingress = Arc::new(RecordingZeroCopyTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("copy", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("owned", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("copy-to-owned", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_copy_minimized_to_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("adapter route add");

    let frame = zero_copy_payload_frame(b"copy-owned");
    let expected_metadata = frame.metadata().clone();
    ingress.inject(frame).await;
    wait_for_owned_sent_count(&egress, 1).await;

    let sent = egress.sent_frames();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].metadata(), &expected_metadata);
    assert_eq!(sent[0].payload_bytes(), b"copy-owned");

    let diagnostics = streamer.route_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].route_kind, RouteKind::AdapterBacked);
    assert_eq!(
        diagnostics[0].copy_semantics,
        RouteCopySemantics::AdapterBoundaryCopying
    );

    streamer
        .delete_copy_minimized_to_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("adapter route delete");
    assert!(streamer.route_diagnostics().is_empty());
}
