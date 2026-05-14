/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
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

use std::{collections::HashMap, sync::Arc};

use tokio::{sync::mpsc, task::JoinHandle};
use up_rust::{
    FetchSubscriptionsRequest, FetchSubscriptionsResponse, UCode, UOwnedFrame, UStatus,
    USubscription, UUri,
};

use crate::{endpoint::NativeIngressRegistration, Endpoint, SubscriptionSyncHealth, TransportMode};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RouteKey {
    ingress_name: String,
    ingress_authority: String,
    egress_name: String,
    egress_authority: String,
}

impl RouteKey {
    fn new(ingress: &Endpoint, egress: &Endpoint) -> Self {
        Self {
            ingress_name: ingress.name.clone(),
            ingress_authority: ingress.authority.clone(),
            egress_name: egress.name.clone(),
            egress_authority: egress.authority.clone(),
        }
    }
}

struct RouteBinding {
    registrations: Vec<Arc<dyn NativeIngressRegistration>>,
    dispatch_task: JoinHandle<()>,
}

pub struct UStreamer {
    name: String,
    message_queue_size: usize,
    usubscription: Arc<dyn USubscription>,
    subscription_snapshot: FetchSubscriptionsResponse,
    subscription_sync_health: SubscriptionSyncHealth,
    routes: HashMap<RouteKey, RouteBinding>,
}

impl UStreamer {
    pub async fn new(
        name: &str,
        message_queue_size: u16,
        usubscription: Arc<dyn USubscription>,
    ) -> Result<Self, UStatus> {
        let mut streamer = Self {
            name: name.to_string(),
            message_queue_size: usize::from(message_queue_size.max(1)),
            usubscription,
            subscription_snapshot: FetchSubscriptionsResponse::default(),
            subscription_sync_health: SubscriptionSyncHealth::default(),
            routes: HashMap::new(),
        };
        let _ = streamer.refresh_subscriptions().await;
        Ok(streamer)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn subscription_sync_health(&self) -> SubscriptionSyncHealth {
        self.subscription_sync_health.clone()
    }

    pub fn subscription_snapshot(&self) -> &FetchSubscriptionsResponse {
        &self.subscription_snapshot
    }

    pub async fn refresh_subscriptions(&mut self) -> Result<SubscriptionSyncHealth, UStatus> {
        self.subscription_sync_health.previous_attempt_succeeded =
            self.subscription_sync_health.last_attempt_succeeded;
        self.subscription_sync_health.last_attempt_at = Some(std::time::SystemTime::now());

        match self
            .usubscription
            .fetch_subscriptions(FetchSubscriptionsRequest)
            .await
        {
            Ok(snapshot) => {
                self.subscription_snapshot = snapshot;
                self.subscription_sync_health.last_attempt_succeeded = Some(true);
                self.subscription_sync_health.last_success_at =
                    self.subscription_sync_health.last_attempt_at;
                Ok(self.subscription_sync_health())
            }
            Err(err) => {
                self.subscription_sync_health.last_attempt_succeeded = Some(false);
                Err(err)
            }
        }
    }

    pub async fn add_route_ref(
        &mut self,
        ingress: &Endpoint,
        egress: &Endpoint,
    ) -> Result<(), UStatus> {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = RouteKey::new(ingress, egress);
        if self.routes.contains_key(&route_key) {
            return Err(UStatus::fail_with_code(
                UCode::ALREADY_EXISTS,
                "route already exists",
            ));
        }

        let (tx, mut rx) = mpsc::channel::<UOwnedFrame>(self.message_queue_size);
        let mut registrations = Vec::new();
        for source_filter in self.source_filters_for_route(ingress, egress) {
            registrations.push(
                ingress
                    .transport
                    .register_ingress(&source_filter, None, tx.clone())
                    .await?,
            );
        }
        let egress_transport = egress.transport.clone();
        let dispatch_task = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let _ = egress_transport.send_frame(frame).await;
            }
        });

        self.routes.insert(
            route_key,
            RouteBinding {
                registrations,
                dispatch_task,
            },
        );
        Ok(())
    }

    pub async fn add_route(&mut self, ingress: Endpoint, egress: Endpoint) -> Result<(), UStatus> {
        self.add_route_ref(&ingress, &egress).await
    }

    pub async fn delete_route_ref(
        &mut self,
        ingress: &Endpoint,
        egress: &Endpoint,
    ) -> Result<(), UStatus> {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = RouteKey::new(ingress, egress);
        let Some(binding) = self.routes.remove(&route_key) else {
            return Err(UStatus::fail_with_code(UCode::NOT_FOUND, "route not found"));
        };
        binding.dispatch_task.abort();
        for registration in binding.registrations {
            registration.unregister().await?;
        }
        Ok(())
    }

    pub async fn delete_route(
        &mut self,
        ingress: Endpoint,
        egress: Endpoint,
    ) -> Result<(), UStatus> {
        self.delete_route_ref(&ingress, &egress).await
    }
}

impl UStreamer {
    fn source_filters_for_route(&self, ingress: &Endpoint, egress: &Endpoint) -> Vec<UUri> {
        let mut filters = Vec::new();
        if ingress.mode() == TransportMode::Owned {
            filters.push(authority_to_wildcard_filter(&ingress.authority));
        }

        for subscription in &self.subscription_snapshot.subscriptions {
            let Some(topic) = subscription.topic.as_ref() else {
                continue;
            };
            let Some(subscriber) = subscription.subscriber.as_ref() else {
                continue;
            };
            let Some(subscriber_uri) = subscriber.uri.as_ref() else {
                continue;
            };
            let topic_authority = topic.authority_name();
            let subscriber_authority = subscriber_uri.authority_name();
            let topic_matches_ingress =
                topic_authority == ingress.authority || topic_authority == "*";
            let subscriber_matches_egress =
                subscriber_authority == egress.authority || subscriber_authority == "*";
            if topic_matches_ingress && subscriber_matches_egress && !filters.contains(topic) {
                filters.push(topic.clone());
            }
        }

        if filters.is_empty() {
            filters.push(authority_to_wildcard_filter(&ingress.authority));
        }
        filters
    }
}

impl Drop for UStreamer {
    fn drop(&mut self) {
        for binding in self.routes.values() {
            binding.dispatch_task.abort();
        }
    }
}

fn authority_to_wildcard_filter(authority_name: &str) -> UUri {
    UUri {
        authority_name: authority_name.to_string(),
        ue_id: 0xFFFF_FFFF,
        ue_version_major: 0xFF,
        resource_id: 0xFFFF,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use up_rust::{
        FetchSubscribersRequest, FetchSubscribersResponse, NotificationsRequest, ResetRequest,
        ResetResponse, Subscription, SubscriptionRequest, SubscriptionResponse, UFrameHeader,
        UOwnedListener, UOwnedTransport, UVecTxBuffer, UZeroCopyListener, UZeroCopyTransport,
    };

    use super::*;

    #[derive(Default)]
    struct StaticSubscriptions {
        subscriptions: Vec<Subscription>,
    }

    #[async_trait]
    impl USubscription for StaticSubscriptions {
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
            Ok(FetchSubscriptionsResponse {
                subscriptions: self.subscriptions.clone(),
            })
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

    fn subscription_source() -> Arc<dyn USubscription> {
        Arc::new(StaticSubscriptions::default())
    }

    fn topic(authority: &str) -> UUri {
        UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid topic")
    }

    fn frame(authority: &str) -> UOwnedFrame {
        UOwnedFrame::new(
            UFrameHeader::publish(topic(authority)),
            b"streamed".as_slice(),
        )
    }

    async fn yield_to_forwarder() {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    #[tokio::test]
    async fn routes_owned_to_owned() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_route_ref(
                &Endpoint::from_owned("in", "authority-a", ingress.clone()),
                &Endpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");
        ingress.inject(frame("authority-a")).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn routes_zero_copy_to_owned() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_route_ref(
                &Endpoint::from_zero_copy("in", "authority-a", ingress.clone()),
                &Endpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");
        ingress.inject(frame("authority-a")).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn rejects_duplicate_and_missing_routes() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let in_ep = Endpoint::from_owned("in", "authority-a", ingress);
        let out_ep = Endpoint::from_owned("out", "authority-b", egress);
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        assert!(streamer.add_route_ref(&in_ep, &out_ep).await.is_ok());
        assert_eq!(
            streamer
                .add_route_ref(&in_ep, &out_ep)
                .await
                .expect_err("duplicate route should fail")
                .get_code(),
            UCode::ALREADY_EXISTS
        );
        assert!(streamer.delete_route_ref(&in_ep, &out_ep).await.is_ok());
        assert_eq!(
            streamer
                .delete_route_ref(&in_ep, &out_ep)
                .await
                .expect_err("missing route should fail")
                .get_code(),
            UCode::NOT_FOUND
        );
    }
}
