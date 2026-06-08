#![cfg(feature = "owned-frame-transport")]

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};
use up_rust::core::usubscription::{
    ResetReason, SubscriptionInfo, SubscriptionStatus, USubscription,
};
use up_rust::{
    try_project_umessage_to_frame_metadata, UCode, UMessageBuilder, UOwnedFrame, UOwnedListener,
    UOwnedTransportImpl, UPayloadFormat, UStatus, UUri, ValidatedOwnedFrame,
};
use up_streamer::{OwnedFrameEndpoint, UStreamer};

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

#[derive(Default)]
struct RecordingOwnedTransport {
    sent: Mutex<Vec<UOwnedFrame>>,
    listeners: Mutex<Vec<Arc<dyn UOwnedListener>>>,
    unregister_count: AtomicUsize,
}

impl RecordingOwnedTransport {
    fn first_listener(&self) -> Arc<dyn UOwnedListener> {
        self.listeners
            .lock()
            .expect("listener lock")
            .first()
            .expect("route should register a listener")
            .clone()
    }

    fn sent_frames(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn wait_for_sent_count(&self, expected: usize) {
        for _ in 0..20 {
            if self.sent.lock().expect("sent lock").len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }
}

#[async_trait]
impl UOwnedTransportImpl for RecordingOwnedTransport {
    async fn send_validated_owned(&self, frame: ValidatedOwnedFrame) -> Result<(), UStatus> {
        self.sent
            .lock()
            .expect("sent lock")
            .push(frame.into_inner());
        Ok(())
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
        _listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.unregister_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn owned_payload_frame() -> UOwnedFrame {
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(
        Bytes::from_static(b"protobuf-payload"),
        UPayloadFormat::Protobuf,
    )
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UOwnedFrame::new(metadata, message.payload().map(Bytes::copy_from_slice)).expect("owned frame")
}

#[tokio::test]
async fn owned_route_forwards_standard_payload_as_owned_frame() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint =
        OwnedFrameEndpoint::from_owned("ingress", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("owned-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route add");

    let frame = owned_payload_frame();
    ingress
        .first_listener()
        .on_receive_owned(frame.clone())
        .await;
    egress.wait_for_sent_count(1).await;

    let sent = egress.sent_frames();
    assert_eq!(sent, vec![frame]);
}

#[tokio::test]
async fn delete_owned_route_unregisters_owned_listener() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint =
        OwnedFrameEndpoint::from_owned("ingress", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("egress", "authority-b", egress);
    let mut streamer = UStreamer::new("owned-route-delete", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route add");
    streamer
        .delete_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route delete");

    assert_eq!(ingress.unregister_count.load(Ordering::Relaxed), 1);
}
