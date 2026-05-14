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
use up_rust::{
    FetchSubscribersRequest, FetchSubscribersResponse, FetchSubscriptionsRequest,
    FetchSubscriptionsResponse, NotificationsRequest, ResetRequest, ResetResponse,
    SubscriptionRequest, SubscriptionResponse, UCode, UFrameHeader, UOwnedFrame, UOwnedListener,
    UOwnedTransport, UStatus, USubscription, UUri, UVecTxBuffer, UZeroCopyListener,
    UZeroCopyTransport,
};
use up_streamer::{Endpoint, TransportMode, UStreamer};

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

    async fn unsubscribe(
        &self,
        _unsubscribe_request: up_rust::UnsubscribeRequest,
    ) -> Result<(), UStatus> {
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
        Ok(ResetResponse)
    }
}

#[derive(Default)]
struct MemoryOwnedTransport {
    listeners: Mutex<Vec<Arc<dyn UOwnedListener>>>,
    sent: Mutex<Vec<UOwnedFrame>>,
}

impl MemoryOwnedTransport {
    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listeners lock poisoned")
            .clone();
        for listener in listeners {
            listener.on_receive_owned(frame.clone()).await;
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
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listeners lock poisoned")
            .push(listener);
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
            .position(|existing| Arc::ptr_eq(existing, &listener))
        {
            listeners.remove(index);
        }
        Ok(())
    }
}

#[derive(Default)]
struct MemoryZeroCopyTransport {
    listeners: Mutex<Vec<Arc<dyn UZeroCopyListener<UOwnedFrame>>>>,
    sent: Mutex<Vec<UOwnedFrame>>,
}

impl MemoryZeroCopyTransport {
    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listeners lock poisoned")
            .clone();
        for listener in listeners {
            listener.on_receive_zero_copy(frame.clone()).await;
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
        header: UFrameHeader,
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
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listeners lock poisoned")
            .push(listener);
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
            .position(|existing| Arc::ptr_eq(existing, &listener))
        {
            listeners.remove(index);
        }
        Ok(())
    }
}

fn topic(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid topic")
}

fn frame(authority: &str) -> UOwnedFrame {
    UOwnedFrame::new(
        UFrameHeader::publish(topic(authority)),
        b"native-stream".as_slice(),
    )
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
        Endpoint::from_owned("owned", "authority-a", owned_ingress.clone()).mode(),
        TransportMode::Owned
    );
    assert_eq!(
        Endpoint::from_zero_copy("zc", "authority-b", zero_copy_ingress.clone()).mode(),
        TransportMode::ZeroCopy
    );

    streamer
        .add_route_ref(
            &Endpoint::from_owned("owned-in", "authority-a", owned_ingress.clone()),
            &Endpoint::from_zero_copy("zc-out", "authority-b", zero_copy_egress.clone()),
        )
        .await
        .expect("owned-to-zero-copy route should register");
    streamer
        .add_route_ref(
            &Endpoint::from_zero_copy("zc-in", "authority-c", zero_copy_ingress.clone()),
            &Endpoint::from_owned("owned-out", "authority-d", owned_egress.clone()),
        )
        .await
        .expect("zero-copy-to-owned route should register");

    owned_ingress.inject(frame("authority-a")).await;
    zero_copy_ingress.inject(frame("authority-c")).await;
    yield_to_forwarder().await;

    assert_eq!(zero_copy_egress.sent()[0].payload_bytes(), b"native-stream");
    assert_eq!(owned_egress.sent()[0].payload_bytes(), b"native-stream");
}

#[tokio::test]
async fn duplicate_and_missing_routes_return_status_codes() {
    let ingress = Arc::new(MemoryOwnedTransport::default());
    let egress = Arc::new(MemoryOwnedTransport::default());
    let in_ep = Endpoint::from_owned("in", "authority-a", ingress);
    let out_ep = Endpoint::from_owned("out", "authority-b", egress);
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
