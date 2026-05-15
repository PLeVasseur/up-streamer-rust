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

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use tokio::{sync::mpsc, task::JoinHandle};
use up_rust::usubscription::{
    from_proto_uri, FetchSubscriptionsRequest, FetchSubscriptionsResponse, USubscription,
};
use up_rust::{UCode, UOwnedFrame, UOwnedListener, UStatus, UTransportEndpointRegistration, UUri};

use crate::{Endpoint, SubscriptionSyncHealth, TransportMode};

const RECENT_FRAME_ID_LIMIT: usize = 1024;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RouteKey {
    ingress_name: String,
    ingress_authority: String,
    egress_name: String,
    egress_authority: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouteFilter {
    source: UUri,
    sink: Option<UUri>,
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
    registrations: Vec<UTransportEndpointRegistration>,
    dispatch_task: JoinHandle<()>,
}

struct IngressForwarder {
    tx: mpsc::Sender<UOwnedFrame>,
}

#[async_trait::async_trait]
impl UOwnedListener for IngressForwarder {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        let _ = self.tx.send(frame).await;
    }
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
            .fetch_subscriptions(FetchSubscriptionsRequest::default())
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
        for route_filter in self.filters_for_route(ingress, egress) {
            registrations.push(
                ingress
                    .transport
                    .register_owned_listener(
                        &route_filter.source,
                        route_filter.sink.as_ref(),
                        Arc::new(IngressForwarder { tx: tx.clone() }),
                    )
                    .await?,
            );
        }
        let ingress_name = ingress.name.clone();
        let ingress_authority = ingress.authority.clone();
        let egress_name = egress.name.clone();
        let egress_authority = egress.authority.clone();
        let egress_transport = egress.transport.clone();
        let dispatch_task = tokio::spawn(async move {
            let mut recent_frame_ids = HashSet::new();
            let mut recent_frame_order = VecDeque::new();
            tracing::debug!(
                ingress = %ingress_name,
                ingress_authority = %ingress_authority,
                egress = %egress_name,
                egress_authority = %egress_authority,
                "egress_worker_create"
            );
            while let Some(frame) = rx.recv().await {
                let frame_id = frame.metadata().attributes().id().clone();
                if !recent_frame_ids.insert(frame_id.clone()) {
                    tracing::debug!(
                        ingress = %ingress_name,
                        ingress_authority = %ingress_authority,
                        egress = %egress_name,
                        egress_authority = %egress_authority,
                        ?frame_id,
                        "egress_duplicate_frame_skip"
                    );
                    continue;
                }
                recent_frame_order.push_back(frame_id);
                if recent_frame_order.len() > RECENT_FRAME_ID_LIMIT {
                    if let Some(expired_frame_id) = recent_frame_order.pop_front() {
                        recent_frame_ids.remove(&expired_frame_id);
                    }
                }

                tracing::debug!(
                    ingress = %ingress_name,
                    ingress_authority = %ingress_authority,
                    egress = %egress_name,
                    egress_authority = %egress_authority,
                    "egress_send_attempt"
                );
                match egress_transport.send_owned(frame).await {
                    Ok(()) => tracing::debug!(
                        ingress = %ingress_name,
                        ingress_authority = %ingress_authority,
                        egress = %egress_name,
                        egress_authority = %egress_authority,
                        "egress_send_ok"
                    ),
                    Err(err) => tracing::debug!(
                        ingress = %ingress_name,
                        ingress_authority = %ingress_authority,
                        egress = %egress_name,
                        egress_authority = %egress_authority,
                        ?err,
                        "egress_send_failed"
                    ),
                }
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
    fn filters_for_route(&self, ingress: &Endpoint, egress: &Endpoint) -> Vec<RouteFilter> {
        let mut filters = Vec::new();
        if ingress.mode() == TransportMode::Owned {
            let source = authority_to_wildcard_filter(&ingress.authority);
            filters.push(RouteFilter {
                source: source.clone(),
                sink: None,
            });
            filters.push(RouteFilter {
                source,
                sink: Some(authority_to_wildcard_filter(&egress.authority)),
            });
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
            let topic = from_proto_uri(topic);
            let subscriber_uri = from_proto_uri(subscriber_uri);
            let topic_authority = topic.authority_name();
            let subscriber_authority = subscriber_uri.authority_name();
            let topic_matches_ingress =
                topic_authority == ingress.authority || topic_authority == "*";
            let subscriber_matches_egress =
                subscriber_authority == egress.authority || subscriber_authority == "*";
            let route_filter = RouteFilter {
                source: topic,
                sink: None,
            };
            if topic_matches_ingress
                && subscriber_matches_egress
                && !filters.contains(&route_filter)
            {
                filters.push(route_filter);
            }
        }

        if filters.is_empty() {
            filters.push(RouteFilter {
                source: authority_to_wildcard_filter(&ingress.authority),
                sink: None,
            });
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
    UUri::try_from_parts(authority_name, 0xFFFF_FFFF, 0xFF, 0xFFFF)
        .expect("wildcard URI authority must be valid")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use up_rust::usubscription::{
        to_proto_uri, FetchSubscribersRequest, FetchSubscribersResponse, NotificationsRequest,
        ResetRequest, ResetResponse, SubscriberInfo, Subscription, SubscriptionRequest,
        SubscriptionResponse, UnsubscribeRequest,
    };
    use up_rust::{
        UFrameMetadata, UOwnedListener, UOwnedTransport, UVecTxBuffer, UZeroCopyListener,
        UZeroCopyTransport,
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
                ..Default::default()
            })
        }

        async fn unsubscribe(
            &self,
            _unsubscribe_request: UnsubscribeRequest,
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
            Ok(ResetResponse::default())
        }
    }

    #[derive(Default)]
    struct MemoryOwnedTransport {
        listeners: Mutex<Vec<RegisteredOwnedListener>>,
        filters: Mutex<Vec<(UUri, Option<UUri>)>>,
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

        fn filters(&self) -> Vec<(UUri, Option<UUri>)> {
            self.filters.lock().expect("filters lock poisoned").clone()
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
            self.filters
                .lock()
                .expect("filters lock poisoned")
                .push((source_filter.clone(), sink_filter.cloned()));
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

    fn subscription_source() -> Arc<dyn USubscription> {
        Arc::new(StaticSubscriptions::default())
    }

    fn subscription_source_with(topic: UUri, subscriber: UUri) -> Arc<dyn USubscription> {
        Arc::new(StaticSubscriptions {
            subscriptions: vec![Subscription {
                topic: Some(to_proto_uri(&topic)).into(),
                subscriber: Some(SubscriberInfo {
                    uri: Some(to_proto_uri(&subscriber)).into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            }],
        })
    }

    fn topic(authority: &str) -> UUri {
        UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid topic")
    }

    fn frame(authority: &str) -> UOwnedFrame {
        UOwnedFrame::new(
            UFrameMetadata::publish(topic(authority)),
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
    async fn owned_routes_register_publish_and_point_to_point_filters() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_route_ref(
                &Endpoint::from_owned("in", "authority-a", ingress.clone()),
                &Endpoint::from_owned("out", "authority-b", egress),
            )
            .await
            .expect("route should register");

        let filters = ingress.filters();
        assert!(filters.contains(&(authority_to_wildcard_filter("authority-a"), None)));
        assert!(filters.contains(&(
            authority_to_wildcard_filter("authority-a"),
            Some(authority_to_wildcard_filter("authority-b"))
        )));
    }

    #[tokio::test]
    async fn route_dispatch_suppresses_duplicate_owned_filter_matches() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = UStreamer::new(
            "test",
            8,
            subscription_source_with(topic("authority-a"), topic("authority-b")),
        )
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

        assert_eq!(egress.sent().len(), 1);
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
