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

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use up_rust::usubscription::{
    FetchSubscribersRequest, FetchSubscribersResponse, FetchSubscriptionsRequest,
    FetchSubscriptionsResponse, NotificationsRequest, ResetRequest, ResetResponse,
    SubscriptionRequest, SubscriptionResponse, USubscription, UnsubscribeRequest,
};
use up_rust::{
    zero_copy::{UVecTxBuffer, UZeroCopyListener, UZeroCopyTransport},
    UCode, UFrameBuilder, UFrameMetadata, UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus,
    UUri,
};
use up_streamer::{OwnedFrameEndpoint, TransportMode, UStreamer};

#[derive(Default)]
struct EmptySubscriptions;

#[async_trait]
impl USubscription for EmptySubscriptions {
    async fn subscribe(
        &self,
        _subscription_request: SubscriptionRequest,
    ) -> Result<SubscriptionResponse, UStatus> {
        Ok(SubscriptionResponse::default())
    }

    async fn fetch_subscriptions(
        &self,
        _fetch_subscriptions_request: FetchSubscriptionsRequest,
    ) -> Result<FetchSubscriptionsResponse, UStatus> {
        Ok(FetchSubscriptionsResponse::default())
    }

    async fn unsubscribe(&self, _unsubscribe_request: UnsubscribeRequest) -> Result<(), UStatus> {
        Ok(())
    }

    async fn register_for_notifications(
        &self,
        _notifications_register_request: NotificationsRequest,
    ) -> Result<(), UStatus> {
        Ok(())
    }

    async fn unregister_for_notifications(
        &self,
        _notifications_unregister_request: NotificationsRequest,
    ) -> Result<(), UStatus> {
        Ok(())
    }

    async fn fetch_subscribers(
        &self,
        _fetch_subscribers_request: FetchSubscribersRequest,
    ) -> Result<FetchSubscribersResponse, UStatus> {
        Ok(FetchSubscribersResponse::default())
    }

    async fn reset(&self, _reset_request: ResetRequest) -> Result<ResetResponse, UStatus> {
        Ok(ResetResponse::default())
    }
}

#[derive(Default)]
struct MemoryOwnedTransport {
    listeners: Mutex<Vec<RegisteredOwnedListener>>,
    sent: Mutex<Vec<UOwnedFrame>>,
}

#[derive(Clone)]
struct RegisteredOwnedListener {
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UOwnedListener>,
}

impl RegisteredOwnedListener {
    fn matches_frame(&self, frame: &UOwnedFrame) -> bool {
        if !self.source_filter.matches(frame.metadata().source()) {
            return false;
        }
        if let Some(sink_filter) = &self.sink_filter {
            frame
                .metadata()
                .sink()
                .is_some_and(|sink| sink_filter.matches(sink))
        } else {
            frame.metadata().sink().is_none()
        }
    }
}

impl MemoryOwnedTransport {
    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listeners lock poisoned")
            .clone();
        for registration in listeners {
            if registration.matches_frame(&frame) {
                registration.listener.on_receive_owned(frame.clone()).await;
            }
        }
    }

    fn sent(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().expect("sent lock poisoned").clone()
    }
}

#[async_trait]
impl UOwnedTransport for MemoryOwnedTransport {
    async fn send_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
        self.sent.lock().expect("sent lock poisoned").push(frame);
        Ok(())
    }

    async fn register_owned_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listeners lock poisoned")
            .push(RegisteredOwnedListener {
                source_filter: source_filter.clone(),
                sink_filter: sink_filter.cloned(),
                listener,
            });
        Ok(())
    }

    async fn unregister_owned_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        let mut listeners = self.listeners.lock().expect("listeners lock poisoned");
        if let Some(index) = listeners
            .iter()
            .position(|existing| Arc::ptr_eq(&existing.listener, &listener))
        {
            listeners.remove(index);
        }
        Ok(())
    }
}

#[derive(Default)]
struct MemoryZeroCopyTransport {
    listeners: Mutex<Vec<RegisteredZeroCopyListener>>,
    sent: Mutex<Vec<UOwnedFrame>>,
}

#[derive(Clone)]
struct RegisteredZeroCopyListener {
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UZeroCopyListener<UOwnedFrame>>,
}

impl RegisteredZeroCopyListener {
    fn matches_frame(&self, frame: &UOwnedFrame) -> bool {
        if !self.source_filter.matches(frame.metadata().source()) {
            return false;
        }
        if let Some(sink_filter) = &self.sink_filter {
            frame
                .metadata()
                .sink()
                .is_some_and(|sink| sink_filter.matches(sink))
        } else {
            frame.metadata().sink().is_none()
        }
    }
}

impl MemoryZeroCopyTransport {
    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listeners lock poisoned")
            .clone();
        for registration in listeners {
            if registration.matches_frame(&frame) {
                registration
                    .listener
                    .on_receive_zero_copy(frame.clone())
                    .await;
            }
        }
    }

    fn sent(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().expect("sent lock poisoned").clone()
    }
}

#[async_trait]
impl UZeroCopyTransport for MemoryZeroCopyTransport {
    type Tx = UVecTxBuffer;
    type Rx = UOwnedFrame;

    async fn reserve(
        &self,
        header: UFrameMetadata,
        payload_len: usize,
        _alignment: usize,
    ) -> Result<Self::Tx, UStatus> {
        Ok(UVecTxBuffer::new(header, payload_len))
    }

    async fn send_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
        self.sent
            .lock()
            .expect("sent lock poisoned")
            .push(buffer.into_frame());
        Ok(())
    }

    async fn register_zero_copy_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listeners lock poisoned")
            .push(RegisteredZeroCopyListener {
                source_filter: source_filter.clone(),
                sink_filter: sink_filter.cloned(),
                listener,
            });
        Ok(())
    }

    async fn unregister_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        let mut listeners = self.listeners.lock().expect("listeners lock poisoned");
        if let Some(index) = listeners
            .iter()
            .position(|existing| Arc::ptr_eq(&existing.listener, &listener))
        {
            listeners.remove(index);
        }
        Ok(())
    }
}

fn point_to_point_frame(
    source_authority: &str,
    sink_authority: &str,
    resource_id: u16,
) -> UOwnedFrame {
    let source =
        UUri::try_from_parts(source_authority, 0x4210, 1, resource_id).expect("valid source URI");
    let sink = UUri::try_from_parts(sink_authority, 0x4220, 1, 0).expect("valid sink URI");
    UFrameBuilder::notification(source, sink)
        .build_with_raw_payload("native-stream")
        .expect("valid notification frame")
}

fn sent_frame_ids(transport: &MemoryOwnedTransport) -> Vec<up_rust::UUID> {
    transport
        .sent()
        .iter()
        .map(|frame| frame.metadata().attributes().id().clone())
        .collect()
}

fn frame_id(frame: &UOwnedFrame) -> up_rust::UUID {
    frame.metadata().attributes().id().clone()
}

async fn yield_to_forwarder() {
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
}

fn subscription_source() -> Arc<dyn USubscription> {
    Arc::new(EmptySubscriptions)
}

#[tokio::test]
async fn routes_owned_and_zero_copy_configurations() {
    let owned_ingress = Arc::new(MemoryOwnedTransport::default());
    let zero_copy_ingress = Arc::new(MemoryZeroCopyTransport::default());
    let owned_egress = Arc::new(MemoryOwnedTransport::default());
    let zero_copy_egress = Arc::new(MemoryZeroCopyTransport::default());
    let mut streamer = UStreamer::new("native", 8, subscription_source())
        .await
        .expect("streamer should build");

    assert_eq!(
        OwnedFrameEndpoint::from_owned("owned", "authority-a", owned_ingress.clone()).mode(),
        TransportMode::Owned
    );
    assert_eq!(
        OwnedFrameEndpoint::from_zero_copy("zc", "authority-b", zero_copy_ingress.clone()).mode(),
        TransportMode::ZeroCopy
    );

    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_owned("owned-in", "authority-a", owned_ingress.clone()),
            &OwnedFrameEndpoint::from_zero_copy("zc-out", "authority-b", zero_copy_egress.clone()),
        )
        .await
        .expect("owned-to-zero-copy route should register");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy("zc-in", "authority-c", zero_copy_ingress.clone()),
            &OwnedFrameEndpoint::from_owned("owned-out", "authority-d", owned_egress.clone()),
        )
        .await
        .expect("zero-copy-to-owned route should register");

    owned_ingress
        .inject(point_to_point_frame("authority-a", "authority-b", 0x9001))
        .await;
    zero_copy_ingress
        .inject(point_to_point_frame("authority-c", "authority-d", 0x9002))
        .await;
    yield_to_forwarder().await;

    assert_eq!(zero_copy_egress.sent()[0].payload_bytes(), b"native-stream");
    assert_eq!(owned_egress.sent()[0].payload_bytes(), b"native-stream");
}

#[tokio::test]
async fn duplicate_and_missing_routes_return_status_codes() {
    let ingress = Arc::new(MemoryOwnedTransport::default());
    let egress = Arc::new(MemoryOwnedTransport::default());
    let in_ep = OwnedFrameEndpoint::from_owned("in", "authority-a", ingress);
    let out_ep = OwnedFrameEndpoint::from_owned("out", "authority-b", egress);
    let mut streamer = UStreamer::new("native", 8, subscription_source())
        .await
        .expect("streamer should build");

    assert!(streamer.add_route_ref(&in_ep, &out_ep).await.is_ok());
    assert_eq!(
        streamer
            .add_route_ref(&in_ep, &out_ep)
            .await
            .expect_err("duplicate should fail")
            .get_code(),
        UCode::ALREADY_EXISTS
    );
    assert!(streamer.delete_route_ref(&in_ep, &out_ep).await.is_ok());
    assert_eq!(
        streamer
            .delete_route_ref(&in_ep, &out_ep)
            .await
            .expect_err("missing should fail")
            .get_code(),
        UCode::NOT_FOUND
    );
}

#[tokio::test]
async fn routes_multiple_owned_authorities_and_removes_rules() {
    let local = Arc::new(MemoryOwnedTransport::default());
    let remote_a = Arc::new(MemoryOwnedTransport::default());
    let remote_b = Arc::new(MemoryOwnedTransport::default());
    let local_ep = OwnedFrameEndpoint::from_owned("local", "local-authority", local.clone());
    let remote_a_ep =
        OwnedFrameEndpoint::from_owned("remote-a", "remote-a-authority", remote_a.clone());
    let remote_b_ep =
        OwnedFrameEndpoint::from_owned("remote-b", "remote-b-authority", remote_b.clone());
    let mut streamer = UStreamer::new("native", 8, subscription_source())
        .await
        .expect("streamer should build");

    streamer
        .add_route_ref(&local_ep, &remote_a_ep)
        .await
        .expect("local to remote-a should register");
    streamer
        .add_route_ref(&remote_a_ep, &local_ep)
        .await
        .expect("remote-a to local should register");

    let local_to_a = point_to_point_frame("local-authority", "remote-a-authority", 0x9001);
    let local_to_b_before = point_to_point_frame("local-authority", "remote-b-authority", 0x9002);
    let a_to_local = point_to_point_frame("remote-a-authority", "local-authority", 0x9003);
    let b_to_local_before = point_to_point_frame("remote-b-authority", "local-authority", 0x9004);
    local.inject(local_to_a.clone()).await;
    local.inject(local_to_b_before.clone()).await;
    remote_a.inject(a_to_local.clone()).await;
    remote_b.inject(b_to_local_before).await;
    yield_to_forwarder().await;

    assert!(sent_frame_ids(&remote_a).contains(&frame_id(&local_to_a)));
    assert!(!sent_frame_ids(&remote_a).contains(&frame_id(&local_to_b_before)));
    assert!(sent_frame_ids(&local).contains(&frame_id(&a_to_local)));
    assert!(remote_b.sent().is_empty());

    streamer
        .add_route_ref(&local_ep, &remote_b_ep)
        .await
        .expect("local to remote-b should register");
    streamer
        .add_route_ref(&remote_b_ep, &local_ep)
        .await
        .expect("remote-b to local should register");

    let local_to_b = point_to_point_frame("local-authority", "remote-b-authority", 0x9005);
    let b_to_local = point_to_point_frame("remote-b-authority", "local-authority", 0x9006);
    local.inject(local_to_b.clone()).await;
    remote_b.inject(b_to_local.clone()).await;
    yield_to_forwarder().await;

    assert!(sent_frame_ids(&remote_b).contains(&frame_id(&local_to_b)));
    assert!(sent_frame_ids(&local).contains(&frame_id(&b_to_local)));

    streamer
        .delete_route_ref(&local_ep, &remote_a_ep)
        .await
        .expect("local to remote-a should unregister");
    streamer
        .delete_route_ref(&remote_a_ep, &local_ep)
        .await
        .expect("remote-a to local should unregister");

    let local_to_a_after_delete =
        point_to_point_frame("local-authority", "remote-a-authority", 0x9007);
    let a_to_local_after_delete =
        point_to_point_frame("remote-a-authority", "local-authority", 0x9008);
    let local_to_b_after_delete =
        point_to_point_frame("local-authority", "remote-b-authority", 0x9009);
    local.inject(local_to_a_after_delete.clone()).await;
    remote_a.inject(a_to_local_after_delete.clone()).await;
    local.inject(local_to_b_after_delete.clone()).await;
    yield_to_forwarder().await;

    assert!(!sent_frame_ids(&remote_a).contains(&frame_id(&local_to_a_after_delete)));
    assert!(!sent_frame_ids(&local).contains(&frame_id(&a_to_local_after_delete)));
    assert!(sent_frame_ids(&remote_b).contains(&frame_id(&local_to_b_after_delete)));
}
