// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Verify owned/native-loan adapter routing and teardown across endpoint API classes.
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
    PayloadEncoding, ProtobufWire, UCode, UFrameView, UMessageBuilder, UOwnedFrame, UOwnedListener,
    UOwnedTransportImpl, UStatus, UUri, UWithNativePrefixWire,
};
#[path = "support/prepared.rs"]
mod prepared;
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
        Ok(vec![SubscriptionInfo::new(
            topic(),
            UUri::try_from_parts("authority-b", 0x5678, 1, 0).unwrap(),
            SubscriptionStatus::Subscribed,
            None,
            None,
        )])
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
    listeners: Mutex<Vec<OwnedEntry>>,
}
type OwnedEntry = (UUri, Option<UUri>, Arc<dyn UOwnedListener>);

impl RecordingOwnedTransport {
    fn sent_frames(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self.listeners.lock().expect("listener lock").clone();
        for (source, sink, listener) in listeners {
            if source.matches(frame.metadata().source())
                && match (sink.as_ref(), frame.metadata().sink()) {
                    (None, None) => true,
                    (Some(filter), Some(actual)) => filter.matches(actual),
                    _ => false,
                }
            {
                listener.on_receive_owned(frame.clone()).await;
            }
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
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners.lock().expect("listener lock").push((
            source_filter.clone(),
            sink_filter.cloned(),
            listener,
        ));
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
            .retain(|(_, _, registered)| !Arc::ptr_eq(registered, &listener));
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

fn zero_copy_payload_frame(payload: &'static [u8]) -> UOwnedFrame {
    let message = UMessageBuilder::publish(topic())
        .build_with_payload(Bytes::from_static(payload), PayloadEncoding::PROTOBUF)
        .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UOwnedFrame::new(metadata, Some(Bytes::from_static(payload))).expect("prepared input frame")
}

async fn wait_for_zero_copy_sent_count(transport: &prepared::Core, expected: usize) {
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
    let egress = Arc::new(prepared::Core::default());
    let ingress_endpoint = OwnedFrameEndpoint::from_owned("owned", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new(
        "copy",
        "authority-b",
        Arc::new(Arc::clone(&egress).into_protobuf_transport()),
    )
    .with_routes(&[prepared::reservation(topic(), None)])
    .unwrap();
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
    let ingress = Arc::new(prepared::Core::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new(
        "copy",
        "authority-a",
        Arc::new(Arc::clone(&ingress).into_protobuf_transport()),
    )
    .with_receive_routes(&[(topic(), None)])
    .unwrap();
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
    ingress.inject::<ProtobufWire>(frame);
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

#[tokio::test]
async fn owned_ingress_backpressure_yields_to_its_worker_on_a_current_thread_runtime() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(prepared::Core::default());
    let left = OwnedFrameEndpoint::from_owned("owned", "authority-a", ingress.clone());
    let right = ZeroCopyFrameEndpoint::new(
        "native",
        "authority-b",
        Arc::new(Arc::clone(&egress).into_protobuf_transport()),
    )
    .with_routes(&[prepared::reservation(topic(), None)])
    .unwrap();
    let mut streamer = UStreamer::new("async-backpressure", 1, Arc::new(EmptySubscription))
        .await
        .unwrap();
    streamer
        .add_owned_to_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    for _ in 0..3 {
        ingress.inject(owned_payload_frame(b"ordered")).await;
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "async admission blocked the only runtime thread"
    );
    prepared::wait_for(|| egress.sent_frames().len() == 3).await;
    streamer
        .delete_owned_to_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
}
