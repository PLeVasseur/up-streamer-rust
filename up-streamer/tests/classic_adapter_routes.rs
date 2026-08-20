/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
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

//! R3A: runtime proof of the classic <-> owned-frame adapter routes, including
//! open payload-encoding transit over the classic surface.

#![cfg(feature = "owned-frame-transport")]

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};
use up_rust::communication::SubscriptionStatus;
use up_rust::core::usubscription::{ResetReason, SubscriptionInfo, USubscription};
use up_rust::frame::metadata::try_project_umessage_to_frame_metadata;
use up_rust::{
    PayloadEncoding, UCode, UListener, UMessage, UMessageBuilder, UOwnedFrame, UOwnedListener,
    UOwnedTransportImpl, UStatus, UTransport, UUri,
};
use up_streamer::{Endpoint, OwnedFrameEndpoint, UStreamer};

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

/// Classic `UTransport` double: records sends, lets tests inject receives.
#[derive(Default)]
struct RecordingClassicTransport {
    sent: Mutex<Vec<UMessage>>,
    listeners: Mutex<Vec<Arc<dyn UListener>>>,
}

impl RecordingClassicTransport {
    fn sent_messages(&self) -> Vec<UMessage> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn inject(&self, message: UMessage) {
        let listeners = self.listeners.lock().expect("listener lock").clone();
        for listener in listeners {
            listener.on_receive(message.clone()).await;
        }
    }
}

#[async_trait]
impl UTransport for RecordingClassicTransport {
    async fn send(&self, message: UMessage) -> Result<(), UStatus> {
        self.sent.lock().expect("sent lock").push(message);
        Ok(())
    }

    async fn register_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UListener>,
    ) -> Result<(), UStatus> {
        self.listeners.lock().expect("listener lock").push(listener);
        Ok(())
    }

    async fn unregister_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listener lock")
            .retain(|registered| !Arc::ptr_eq(registered, &listener));
        Ok(())
    }
}

/// Owned-frame transport double.
#[derive(Default)]
struct RecordingOwnedTransport {
    sent: Mutex<Vec<UOwnedFrame>>,
    listeners: Mutex<Vec<Arc<dyn UOwnedListener>>>,
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
        Err(UStatus::fail_with_code(UCode::NotFound, "unused"))
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

fn xcdr_encoding() -> PayloadEncoding {
    PayloadEncoding::from_registry_entry(9)
}

async fn wait_for<F: Fn() -> bool>(check: F) {
    for _ in 0..50 {
        if check() {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

/// A classic message carrying a registered payload encoding crosses the bridge into
/// the owned-frame world with its identity and message id intact.
#[tokio::test]
async fn classic_to_owned_forwards_encoding_message() {
    let ingress = Arc::new(RecordingClassicTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint = Endpoint::new("classic", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("owned", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("classic-to-owned", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");
    streamer
        .add_classic_to_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("route");

    let message = UMessageBuilder::publish(topic())
        .build_with_payload(Bytes::from_static(b"cdr-bytes"), xcdr_encoding())
        .expect("message");
    let expected_id = message.attributes().id().clone();
    ingress.inject(message).await;

    wait_for(|| !egress.sent_frames().is_empty()).await;
    let frames = egress.sent_frames();
    assert_eq!(frames.len(), 1, "exactly one frame forwarded");
    let frame = &frames[0];
    assert_eq!(frame.metadata().payload_encoding(), Some(&xcdr_encoding()));
    assert_eq!(
        frame.metadata().id(),
        &expected_id,
        "bridge must not mint ids"
    );
    assert_eq!(frame.payload_bytes(), b"cdr-bytes");
}

/// An owned frame with a registered payload encoding crosses back into the
/// classic world with bytes untouched.
#[tokio::test]
async fn owned_to_classic_restores_identity() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingClassicTransport::default());
    let ingress_endpoint = OwnedFrameEndpoint::from_owned("owned", "authority-a", ingress.clone());
    let egress_endpoint = Endpoint::new("classic", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("owned-to-classic", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");
    streamer
        .add_owned_to_classic_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("route");

    let source = UMessageBuilder::publish(topic())
        .build_with_payload(Bytes::from_static(b"cdr-bytes"), xcdr_encoding())
        .expect("message");
    let expected_id = source.attributes().id().clone();
    let metadata = try_project_umessage_to_frame_metadata(&source).expect("metadata");
    let frame =
        UOwnedFrame::with_payload(metadata, Bytes::from_static(b"cdr-bytes")).expect("owned frame");
    ingress.inject(frame).await;

    wait_for(|| !egress.sent_messages().is_empty()).await;
    let messages = egress.sent_messages();
    assert_eq!(messages.len(), 1, "exactly one message forwarded");
    let attributes = messages[0].attributes();
    assert_eq!(attributes.payload_encoding(), Some(xcdr_encoding()));
    assert_eq!(attributes.id(), &expected_id, "bridge must not mint ids");
    assert_eq!(
        messages[0].payload().as_deref(),
        Some(b"cdr-bytes".as_slice())
    );
}

#[cfg(feature = "experimental-copy-minimized-routing")]
mod classic_copy_minimized {
    use super::*;
    use up_rust::{
        UFrameView, UTxLoanSpec, UVecRxLease, UVecTxBuffer, UZeroCopyListener,
        UZeroCopyTransportImpl,
    };
    use up_streamer::{CopyMinimizedRouteOptions, ZeroCopyFrameEndpoint};

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

    /// The 96-row route kind: a classic message with a registered encoding crosses
    /// into the copy-minimized world through the composed
    /// classic-ingress -> projection -> CM-egress path.
    #[tokio::test]
    async fn classic_to_copy_minimized_forwards_encoding_message() {
        let ingress = Arc::new(RecordingClassicTransport::default());
        let egress = Arc::new(RecordingZeroCopyTransport::default());
        let ingress_endpoint = Endpoint::new("classic", "authority-a", ingress.clone());
        let egress_endpoint = ZeroCopyFrameEndpoint::new("cm", "authority-b", egress.clone());
        let mut streamer = UStreamer::new("classic-to-cm", 4, Arc::new(EmptySubscription))
            .await
            .expect("streamer");
        streamer
            .add_classic_to_copy_minimized_route_ref(
                &ingress_endpoint,
                &egress_endpoint,
                CopyMinimizedRouteOptions {
                    payload_alignment: 8,
                },
            )
            .await
            .expect("route");

        let message = UMessageBuilder::publish(topic())
            .build_with_payload(Bytes::from_static(b"cdr-bytes"), xcdr_encoding())
            .expect("message");
        let expected_id = message.attributes().id().clone();
        ingress.inject(message).await;

        wait_for(|| !egress.sent_frames().is_empty()).await;
        let frames = egress.sent_frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].metadata().payload_encoding(),
            Some(&xcdr_encoding())
        );
        assert_eq!(frames[0].metadata().id(), &expected_id);
    }

    /// And back: a zero-copy lease lands as a classic message with its numeric
    /// identity populated.
    #[tokio::test]
    async fn copy_minimized_to_classic_restores_identity() {
        let ingress = Arc::new(RecordingZeroCopyTransport::default());
        let egress = Arc::new(RecordingClassicTransport::default());
        let ingress_endpoint = ZeroCopyFrameEndpoint::new("cm", "authority-a", ingress.clone());
        let egress_endpoint = Endpoint::new("classic", "authority-b", egress.clone());
        let mut streamer = UStreamer::new("cm-to-classic", 4, Arc::new(EmptySubscription))
            .await
            .expect("streamer");
        streamer
            .add_copy_minimized_to_classic_route_ref(&ingress_endpoint, &egress_endpoint)
            .await
            .expect("route");

        let source = UMessageBuilder::publish(topic())
            .build_with_payload(Bytes::from_static(b"cdr-bytes"), xcdr_encoding())
            .expect("message");
        let expected_id = source.attributes().id().clone();
        let metadata = try_project_umessage_to_frame_metadata(&source).expect("metadata");
        let lease = UVecRxLease::new(metadata, Some(b"cdr-bytes".to_vec())).expect("lease");
        ingress.inject(lease).await;

        wait_for(|| !egress.sent_messages().is_empty()).await;
        let messages = egress.sent_messages();
        assert_eq!(messages.len(), 1);
        let attributes = messages[0].attributes();
        assert_eq!(attributes.payload_encoding(), Some(xcdr_encoding()));
        assert_eq!(attributes.id(), &expected_id);
        assert_eq!(
            messages[0].payload().as_deref(),
            Some(b"cdr-bytes".as_slice())
        );
    }
}
