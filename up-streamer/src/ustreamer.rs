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
    sync::{Arc, Mutex},
};

use tokio::{sync::mpsc, task::JoinHandle};
use up_rust::usubscription::{
    from_proto_uri, FetchSubscriptionsRequest, FetchSubscriptionsResponse, USubscription,
};
use up_rust::{
    transport::UOwnedFrameEndpointRegistration, UCode, UOwnedFrame, UOwnedListener, UStatus, UUri,
};

use crate::{
    DataPlaneFailureKind, DataPlaneHealth, DataPlaneRoute, OwnedFrameEndpoint,
    SubscriptionSyncHealth,
};

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
    fn new(ingress: &OwnedFrameEndpoint, egress: &OwnedFrameEndpoint) -> Self {
        Self {
            ingress_name: ingress.name.clone(),
            ingress_authority: ingress.authority.clone(),
            egress_name: egress.name.clone(),
            egress_authority: egress.authority.clone(),
        }
    }
}

struct RouteBinding {
    ingress: OwnedFrameEndpoint,
    egress: OwnedFrameEndpoint,
    tx: mpsc::Sender<UOwnedFrame>,
    registrations: Vec<UOwnedFrameEndpointRegistration>,
    dispatch_task: JoinHandle<()>,
}

struct IngressForwarder {
    tx: mpsc::Sender<UOwnedFrame>,
    route: DataPlaneRoute,
    data_plane_health: Arc<Mutex<DataPlaneHealth>>,
}

#[async_trait::async_trait]
impl UOwnedListener for IngressForwarder {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        if self.tx.send(frame).await.is_err() {
            tracing::warn!(
                ingress = %self.route.ingress_name,
                ingress_authority = %self.route.ingress_authority,
                egress = %self.route.egress_name,
                egress_authority = %self.route.egress_authority,
                "ingress_queue_closed"
            );
            record_data_plane_failure(
                &self.data_plane_health,
                DataPlaneFailureKind::IngressQueueClosed,
                self.route.clone(),
                "route ingress queue closed before frame could be forwarded",
            );
        }
    }
}

/// Native-frame uStreamer router.
///
/// `UStreamer` registers owned-frame listeners on ingress endpoints, filters
/// frames using the current uSubscription snapshot, and sends matching frames to
/// egress endpoints. The router stores and forwards [`UOwnedFrame`] values even
/// when an endpoint is backed by a zero-copy transport.
///
/// Routes are authority-to-authority bindings between [`OwnedFrameEndpoint`]s.
/// When a route involves a zero-copy endpoint, copies happen inside that endpoint
/// adapter; the streamer does not preserve zero-copy leases across route
/// boundaries.
pub struct UStreamer {
    name: String,
    message_queue_size: usize,
    usubscription: Arc<dyn USubscription>,
    subscription_snapshot: FetchSubscriptionsResponse,
    subscription_sync_health: SubscriptionSyncHealth,
    data_plane_health: Arc<Mutex<DataPlaneHealth>>,
    routes: HashMap<RouteKey, RouteBinding>,
}

impl UStreamer {
    /// Creates a streamer and fetches the initial subscription snapshot.
    ///
    /// `message_queue_size` controls the bounded channel used between ingress
    /// listener callbacks and the egress worker for each route. A value of zero
    /// is treated as one.
    ///
    /// Construction succeeds even if the initial subscription refresh fails; the
    /// failure is reflected in [`Self::subscription_sync_health`]. Route creation
    /// will fail until a subscription snapshot has been fetched successfully.
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
            data_plane_health: Arc::new(Mutex::new(DataPlaneHealth::default())),
            routes: HashMap::new(),
        };
        let _ = streamer.refresh_subscriptions().await;
        Ok(streamer)
    }

    /// Returns the streamer name used for diagnostics.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns health metadata for the most recent subscription refresh attempts.
    pub fn subscription_sync_health(&self) -> SubscriptionSyncHealth {
        self.subscription_sync_health.clone()
    }

    /// Returns the last successfully fetched subscription snapshot.
    pub fn subscription_snapshot(&self) -> &FetchSubscriptionsResponse {
        &self.subscription_snapshot
    }

    /// Returns data-plane health metadata for route dispatch failures.
    pub fn data_plane_health(&self) -> DataPlaneHealth {
        self.data_plane_health
            .lock()
            .expect("data-plane health lock poisoned")
            .clone()
    }

    /// Fetches subscriptions and rewires existing routes to match the new
    /// snapshot.
    ///
    /// Route rewiring unregisters old ingress filters and registers the filters
    /// required by the new snapshot. If fetching or rewiring fails, health state
    /// records the failed attempt.
    ///
    /// # Errors
    ///
    /// Returns the uSubscription fetch error or a transport registration error
    /// encountered while rewiring routes.
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
                self.rewire_routes(&snapshot).await?;
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

    /// Adds a route from `ingress` to `egress`.
    ///
    /// The route registers listener filters derived from the current subscription
    /// snapshot. Frames delivered by the ingress endpoint are forwarded to the
    /// egress endpoint unless their frame ID was recently seen on this route.
    ///
    /// # Errors
    ///
    /// Returns an error when authorities are identical, no successful
    /// subscription snapshot is available, the route already exists, or ingress
    /// listener registration fails.
    pub async fn add_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                "ingress and egress authorities must differ",
            ));
        }

        if self.subscription_sync_health.last_success_at.is_none() {
            return Err(UStatus::fail_with_code(
                UCode::FAILED_PRECONDITION,
                "subscription snapshot is unavailable",
            ));
        }

        let route_key = RouteKey::new(ingress, egress);
        let route = data_plane_route(ingress, egress);
        if self.routes.contains_key(&route_key) {
            return Err(UStatus::fail_with_code(
                UCode::ALREADY_EXISTS,
                "route already exists",
            ));
        }

        let ingress_mode = ingress.mode();
        let egress_mode = egress.mode();
        tracing::debug!(
            ingress = %ingress.name,
            ingress_authority = %ingress.authority,
            ?ingress_mode,
            egress = %egress.name,
            egress_authority = %egress.authority,
            ?egress_mode,
            "route_create"
        );

        let (tx, mut rx) = mpsc::channel::<UOwnedFrame>(self.message_queue_size);
        let mut registrations = Vec::new();
        for route_filter in self.filters_for_route(ingress, egress) {
            match ingress
                .transport
                .register_owned_listener(
                    &route_filter.source,
                    route_filter.sink.as_ref(),
                    Arc::new(IngressForwarder {
                        tx: tx.clone(),
                        route: route.clone(),
                        data_plane_health: self.data_plane_health.clone(),
                    }),
                )
                .await
            {
                Ok(registration) => registrations.push(registration),
                Err(err) => {
                    for registration in registrations {
                        let _ = registration.unregister().await;
                    }
                    return Err(err);
                }
            }
        }
        let ingress_name = ingress.name.clone();
        let ingress_authority = ingress.authority.clone();
        let egress_name = egress.name.clone();
        let egress_authority = egress.authority.clone();
        let egress_transport = egress.transport.clone();
        let data_plane_health = self.data_plane_health.clone();
        let dispatch_task = tokio::spawn(async move {
            let mut recent_frame_ids = HashSet::new();
            let mut recent_frame_order = VecDeque::new();
            tracing::debug!(
                ingress = %ingress_name,
                ingress_authority = %ingress_authority,
                ?ingress_mode,
                egress = %egress_name,
                egress_authority = %egress_authority,
                ?egress_mode,
                "egress_worker_create"
            );
            while let Some(frame) = rx.recv().await {
                let frame_id = frame.metadata().attributes().id().clone();
                if !recent_frame_ids.insert(frame_id.clone()) {
                    tracing::debug!(
                        ingress = %ingress_name,
                        ingress_authority = %ingress_authority,
                        ?ingress_mode,
                        egress = %egress_name,
                        egress_authority = %egress_authority,
                        ?egress_mode,
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
                    ?ingress_mode,
                    egress = %egress_name,
                    egress_authority = %egress_authority,
                    ?egress_mode,
                    "egress_send_attempt"
                );
                match egress_transport.send_owned(frame).await {
                    Ok(()) => tracing::debug!(
                        ingress = %ingress_name,
                        ingress_authority = %ingress_authority,
                        ?ingress_mode,
                        egress = %egress_name,
                        egress_authority = %egress_authority,
                        ?egress_mode,
                        "egress_send_ok"
                    ),
                    Err(err) => {
                        tracing::warn!(
                            ingress = %ingress_name,
                            ingress_authority = %ingress_authority,
                            ?ingress_mode,
                            egress = %egress_name,
                            egress_authority = %egress_authority,
                            ?egress_mode,
                            ?err,
                            "egress_send_failed"
                        );
                        record_data_plane_failure(
                            &data_plane_health,
                            DataPlaneFailureKind::EgressSend,
                            route.clone(),
                            format!("{err:?}"),
                        );
                    }
                }
            }
        });

        self.routes.insert(
            route_key,
            RouteBinding {
                ingress: ingress.clone(),
                egress: egress.clone(),
                tx,
                registrations,
                dispatch_task,
            },
        );
        Ok(())
    }

    /// Adds a route, consuming endpoint values after registration.
    ///
    /// This is a convenience wrapper around [`Self::add_route_ref`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::add_route_ref`].
    pub async fn add_route(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.add_route_ref(&ingress, &egress).await
    }

    /// Deletes a route from `ingress` to `egress` and unregisters its listeners.
    ///
    /// If any unregister operation fails, the route is restored with the
    /// remaining registrations so callers can retry deletion.
    ///
    /// # Errors
    ///
    /// Returns an error when authorities are identical, the route does not exist,
    /// or an underlying unregister operation fails.
    pub async fn delete_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
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
        let mut binding = binding;
        let mut remaining_registrations = Vec::new();
        let mut first_err = None;
        for registration in binding.registrations {
            if let Err(err) = registration.unregister().await {
                if first_err.is_none() {
                    first_err = Some(err);
                }
                remaining_registrations.push(registration);
            }
        }
        if let Some(err) = first_err {
            binding.registrations = remaining_registrations;
            self.routes.insert(route_key, binding);
            return Err(err);
        }
        binding.dispatch_task.abort();
        Ok(())
    }

    /// Deletes a route, consuming endpoint values after lookup.
    ///
    /// This is a convenience wrapper around [`Self::delete_route_ref`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::delete_route_ref`].
    pub async fn delete_route(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.delete_route_ref(&ingress, &egress).await
    }
}

impl UStreamer {
    async fn rewire_routes(
        &mut self,
        snapshot: &FetchSubscriptionsResponse,
    ) -> Result<(), UStatus> {
        let mut new_registrations_by_route: HashMap<
            RouteKey,
            Vec<UOwnedFrameEndpointRegistration>,
        > = HashMap::new();
        for (route_key, binding) in &self.routes {
            let filters = filters_for_snapshot(snapshot, &binding.ingress, &binding.egress);
            let mut new_registrations = Vec::new();
            for route_filter in filters {
                match binding
                    .ingress
                    .transport
                    .register_owned_listener(
                        &route_filter.source,
                        route_filter.sink.as_ref(),
                        Arc::new(IngressForwarder {
                            tx: binding.tx.clone(),
                            route: data_plane_route(&binding.ingress, &binding.egress),
                            data_plane_health: self.data_plane_health.clone(),
                        }),
                    )
                    .await
                {
                    Ok(registration) => new_registrations.push(registration),
                    Err(err) => {
                        for registrations in new_registrations_by_route.into_values() {
                            for registration in registrations {
                                let _ = registration.unregister().await;
                            }
                        }
                        for registration in new_registrations {
                            let _ = registration.unregister().await;
                        }
                        return Err(err);
                    }
                }
            }
            new_registrations_by_route.insert(route_key.clone(), new_registrations);
        }

        for (route_key, mut new_registrations) in new_registrations_by_route {
            let Some(binding) = self.routes.get_mut(&route_key) else {
                continue;
            };
            let route = data_plane_route(&binding.ingress, &binding.egress);
            let old_registrations = std::mem::take(&mut binding.registrations);
            for registration in old_registrations {
                if let Err(err) = registration.unregister().await {
                    tracing::warn!(
                        ingress = %route.ingress_name,
                        ingress_authority = %route.ingress_authority,
                        egress = %route.egress_name,
                        egress_authority = %route.egress_authority,
                        ?err,
                        "route_rewire_unregister_old_failed"
                    );
                    record_data_plane_failure(
                        &self.data_plane_health,
                        DataPlaneFailureKind::RouteRewireUnregister,
                        route.clone(),
                        format!("{err:?}"),
                    );
                    new_registrations.push(registration);
                }
            }
            binding.registrations = new_registrations;
        }
        Ok(())
    }

    fn filters_for_route(
        &self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Vec<RouteFilter> {
        filters_for_snapshot(&self.subscription_snapshot, ingress, egress)
    }
}

fn filters_for_snapshot(
    snapshot: &FetchSubscriptionsResponse,
    ingress: &OwnedFrameEndpoint,
    egress: &OwnedFrameEndpoint,
) -> Vec<RouteFilter> {
    let mut filters = vec![RouteFilter {
        source: authority_to_wildcard_filter(&ingress.authority),
        sink: Some(authority_to_wildcard_filter(&egress.authority)),
    }];

    for subscription in &snapshot.subscriptions {
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
        let topic_matches_ingress = topic_authority == ingress.authority || topic_authority == "*";
        let subscriber_matches_egress =
            subscriber_authority == egress.authority || subscriber_authority == "*";
        let route_filter = RouteFilter {
            source: topic,
            sink: None,
        };
        if topic_matches_ingress && subscriber_matches_egress && !filters.contains(&route_filter) {
            filters.push(route_filter);
        }
    }

    filters
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

fn data_plane_route(ingress: &OwnedFrameEndpoint, egress: &OwnedFrameEndpoint) -> DataPlaneRoute {
    DataPlaneRoute {
        ingress_name: ingress.name.clone(),
        ingress_authority: ingress.authority.clone(),
        egress_name: egress.name.clone(),
        egress_authority: egress.authority.clone(),
    }
}

fn record_data_plane_failure(
    health: &Arc<Mutex<DataPlaneHealth>>,
    kind: DataPlaneFailureKind,
    route: DataPlaneRoute,
    message: impl Into<String>,
) {
    health
        .lock()
        .expect("data-plane health lock poisoned")
        .record(kind, route, message.into());
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use protobuf::well_known_types::{any::Any, wrappers::StringValue};
    use up_rust::usubscription::{
        to_proto_uri, FetchSubscribersRequest, FetchSubscribersResponse, NotificationsRequest,
        ResetRequest, ResetResponse, SubscriberInfo, Subscription, SubscriptionRequest,
        SubscriptionResponse, UnsubscribeRequest,
    };
    use up_rust::{
        frame_wire::{ProtobufUMessageFrame, UFrameWireFormat},
        payload::{PlacementDefault, RawBytes, StableContainerPayload, UWireError},
        zero_copy::{UVecTxBuffer, UZeroCopyListener, UZeroCopyTransport},
        PayloadEncoding, ProtobufAnyPayload, ProtobufPayload, UFrameBuilder, UFrameMetadata,
        UOwnedListener, UOwnedTransport,
    };

    use super::*;

    #[repr(C)]
    #[derive(
        Clone,
        Copy,
        Debug,
        Default,
        Eq,
        PartialEq,
        PlacementDefault,
        up_rust::StablePayload,
        up_rust::ByteBackedStablePayload,
    )]
    #[stable_payload(type_name = "example.vehicle.VehiclePose")]
    struct VehiclePose {
        x: u32,
        y: u32,
    }

    #[derive(Default)]
    struct StaticSubscriptions {
        subscriptions: Vec<Subscription>,
        fail_fetch: bool,
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
            if self.fail_fetch {
                return Err(UStatus::fail_with_code(
                    UCode::UNAVAILABLE,
                    "subscription fetch failed",
                ));
            }
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
        fail_send: Mutex<bool>,
        fail_on_registration: Mutex<Option<usize>>,
        register_attempts: Mutex<usize>,
        fail_on_unregister: Mutex<bool>,
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

        fn fail_on_registration(registration_attempt: usize) -> Self {
            Self {
                fail_on_registration: Mutex::new(Some(registration_attempt)),
                ..Default::default()
            }
        }

        fn fail_sends() -> Self {
            Self {
                fail_send: Mutex::new(true),
                ..Default::default()
            }
        }

        fn fail_on_unregister() -> Self {
            Self {
                fail_on_unregister: Mutex::new(true),
                ..Default::default()
            }
        }

        fn listener_count(&self) -> usize {
            self.listeners
                .lock()
                .expect("listeners lock poisoned")
                .len()
        }
    }

    #[async_trait]
    impl UOwnedTransport for MemoryOwnedTransport {
        async fn send_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
            if *self.fail_send.lock().expect("fail_send lock poisoned") {
                return Err(UStatus::fail_with_code(UCode::UNAVAILABLE, "send failed"));
            }
            self.sent.lock().expect("sent lock poisoned").push(frame);
            Ok(())
        }

        async fn register_owned_listener(
            &self,
            source_filter: &UUri,
            sink_filter: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            let mut register_attempts = self
                .register_attempts
                .lock()
                .expect("register_attempts lock poisoned");
            *register_attempts += 1;
            if self
                .fail_on_registration
                .lock()
                .expect("fail_on_registration lock poisoned")
                .is_some_and(|fail_on_registration| fail_on_registration == *register_attempts)
            {
                return Err(UStatus::fail_with_code(
                    UCode::UNAVAILABLE,
                    "listener registration failed",
                ));
            }
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
            if *self
                .fail_on_unregister
                .lock()
                .expect("fail_on_unregister lock poisoned")
            {
                return Err(UStatus::fail_with_code(
                    UCode::UNAVAILABLE,
                    "listener unregister failed",
                ));
            }
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
            alignment: usize,
        ) -> Result<Self::Tx, UStatus> {
            UVecTxBuffer::with_alignment(header, payload_len, alignment).map_err(UStatus::from)
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

    fn failing_subscription_source() -> Arc<dyn USubscription> {
        Arc::new(StaticSubscriptions {
            fail_fetch: true,
            ..Default::default()
        })
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
            ..Default::default()
        })
    }

    fn topic(authority: &str) -> UUri {
        UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid topic")
    }

    fn frame(authority: &str) -> UOwnedFrame {
        UOwnedFrame::new(
            UFrameMetadata::publish(topic(authority)).with_encoding(RawBytes::encoding()),
            b"streamed".as_slice(),
        )
    }

    fn protobuf_payload(value: &str) -> StringValue {
        let mut payload = StringValue::new();
        payload.value = value.to_string();
        payload
    }

    fn point_to_point_frame(source_authority: &str, sink_authority: &str) -> UOwnedFrame {
        let source =
            UUri::try_from_parts(source_authority, 0x4210, 1, 0x9001).expect("valid source URI");
        let sink = UUri::try_from_parts(sink_authority, 0x4220, 1, 0).expect("valid sink URI");
        UFrameBuilder::notification(source, sink)
            .build_with_raw_payload("streamed")
            .expect("valid notification frame")
    }

    async fn yield_to_forwarder() {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    #[tokio::test]
    async fn routes_owned_to_owned() {
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");
        ingress.inject(frame("authority-a")).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn routes_owned_to_owned_preserves_custom_payload_encoding() {
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");

        let encoding = PayloadEncoding::custom(
            "com.example.streamed-native-v1",
            "application/vnd.example.streamed-native",
        );
        let frame = UOwnedFrame::new(
            UFrameMetadata::publish(topic("authority-a")).with_encoding(encoding.clone()),
            b"native-layout".as_slice(),
        );
        ingress.inject(frame).await;
        yield_to_forwarder().await;

        let sent = egress.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].metadata().encoding(), Some(&encoding));
        assert_eq!(sent[0].payload_bytes(), b"native-layout");
    }

    #[tokio::test]
    async fn routes_owned_to_owned_preserves_stable_container_payload_bytes() {
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");

        let pose = VehiclePose { x: 3, y: 5 };
        let frame =
            UOwnedFrame::from_payload_as::<StableContainerPayload<VehiclePose>, VehiclePose>(
                UFrameMetadata::publish(topic("authority-a")),
                &pose,
            )
            .expect("stable-container payload should encode as owned bytes");
        let expected_payload = frame.payload_bytes().to_vec();
        ingress.inject(frame).await;
        yield_to_forwarder().await;

        let sent = egress.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].metadata().encoding(),
            Some(&StableContainerPayload::<VehiclePose>::encoding())
        );
        assert_eq!(sent[0].payload_bytes(), expected_payload.as_slice());
    }

    #[tokio::test]
    async fn routes_owned_to_owned_preserves_protobuf_payload_encoding() {
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");

        let payload = protobuf_payload("protobuf payload routed by streamer");
        let frame = UOwnedFrame::from_serializable::<ProtobufPayload, _>(
            UFrameMetadata::publish(topic("authority-a")),
            &payload,
        )
        .expect("protobuf payload should serialize");
        ingress.inject(frame).await;
        yield_to_forwarder().await;

        let sent = egress.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].metadata().encoding(),
            Some(&ProtobufPayload::encoding())
        );
        let decoded: StringValue = sent[0]
            .deserialize::<ProtobufPayload, _>()
            .expect("protobuf payload should still decode after routing");
        assert_eq!(decoded.value, payload.value);
    }

    #[tokio::test]
    async fn routes_owned_to_owned_preserves_protobuf_any_payload_encoding() {
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");

        let payload = protobuf_payload("protobuf Any payload routed by streamer");
        let any = Any::pack(&payload).expect("protobuf Any should pack");
        let frame = UOwnedFrame::from_serializable::<ProtobufAnyPayload, _>(
            UFrameMetadata::publish(topic("authority-a")),
            &any,
        )
        .expect("protobuf Any payload should serialize");
        ingress.inject(frame).await;
        yield_to_forwarder().await;

        let sent = egress.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].metadata().encoding(),
            Some(&ProtobufAnyPayload::encoding())
        );
        let decoded_any: Any = sent[0]
            .deserialize::<ProtobufAnyPayload, _>()
            .expect("protobuf Any payload should still decode after routing");
        let decoded = decoded_any
            .unpack::<StringValue>()
            .expect("protobuf Any should unpack")
            .expect("protobuf Any should contain StringValue");
        assert_eq!(decoded.value, payload.value);
    }

    #[tokio::test]
    async fn routes_owned_to_owned_preserves_protobuf_umessage_frame_payload_bytes() {
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");

        let payload = protobuf_payload("protobuf payload inside protobuf UMessage frame");
        let inner_frame = UOwnedFrame::from_serializable::<ProtobufPayload, _>(
            UFrameMetadata::publish(topic("authority-inner")),
            &payload,
        )
        .expect("protobuf payload should serialize");
        let envelope = ProtobufUMessageFrame::serialize_frame(&inner_frame)
            .expect("protobuf UMessage frame should serialize");
        let carrier = UOwnedFrame::new(
            UFrameMetadata::publish(topic("authority-a")).with_encoding(RawBytes::encoding()),
            envelope.clone(),
        );

        ingress.inject(carrier).await;
        yield_to_forwarder().await;

        let sent = egress.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].payload_bytes(), envelope.as_ref());

        let wrong_layer = sent[0].deserialize::<ProtobufPayload, StringValue>();
        assert!(matches!(
            wrong_layer,
            Err(UWireError::UnsupportedEncoding { .. })
        ));

        let decoded_frame = ProtobufUMessageFrame::deserialize_frame(sent[0].payload_bytes())
            .expect("outer UMessage frame should decode");
        let decoded_payload: StringValue = decoded_frame
            .deserialize::<ProtobufPayload, _>()
            .expect("inner protobuf payload should decode after outer frame decode");
        assert_eq!(decoded_payload.value, payload.value);
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress),
            )
            .await
            .expect("route should register");

        let filters = ingress.filters();
        assert!(!filters.contains(&(authority_to_wildcard_filter("authority-a"), None)));
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");
        ingress.inject(frame("authority-a")).await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent().len(), 1);
    }

    #[tokio::test]
    async fn egress_send_failure_updates_data_plane_health() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::fail_sends());
        let mut streamer = UStreamer::new(
            "test",
            8,
            subscription_source_with(topic("authority-a"), topic("authority-b")),
        )
        .await
        .expect("streamer should build");

        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress),
            )
            .await
            .expect("route should register");
        ingress.inject(frame("authority-a")).await;
        yield_to_forwarder().await;

        let health = streamer.data_plane_health();
        assert_eq!(health.egress_send_failures, 1);
        let failure = health.last_failure.expect("last failure recorded");
        assert_eq!(failure.kind, DataPlaneFailureKind::EgressSend);
        assert_eq!(failure.route.ingress_authority, "authority-a");
        assert_eq!(failure.route.egress_authority, "authority-b");
    }

    #[tokio::test]
    async fn ingress_forwarder_records_closed_route_queue() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let data_plane_health = Arc::new(Mutex::new(DataPlaneHealth::default()));
        let forwarder = IngressForwarder {
            tx,
            route: DataPlaneRoute {
                ingress_name: "in".to_string(),
                ingress_authority: "authority-a".to_string(),
                egress_name: "out".to_string(),
                egress_authority: "authority-b".to_string(),
            },
            data_plane_health: data_plane_health.clone(),
        };

        forwarder.on_receive_owned(frame("authority-a")).await;

        let health = data_plane_health
            .lock()
            .expect("data-plane health lock poisoned")
            .clone();
        assert_eq!(health.ingress_queue_failures, 1);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::IngressQueueClosed
        );
    }

    #[tokio::test]
    async fn rewire_unregister_failure_records_health_and_suppresses_duplicate_callbacks() {
        let ingress = Arc::new(MemoryOwnedTransport::fail_on_unregister());
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
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");
        streamer
            .refresh_subscriptions()
            .await
            .expect("refresh should keep route installed");
        assert_eq!(ingress.listener_count(), 4);

        ingress.inject(frame("authority-a")).await;
        yield_to_forwarder().await;

        let health = streamer.data_plane_health();
        assert_eq!(health.route_rewire_unregister_failures, 2);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::RouteRewireUnregister
        );
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
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "in",
                    "authority-a",
                    ingress.clone(),
                ),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress.clone()),
            )
            .await
            .expect("route should register");
        ingress
            .inject(point_to_point_frame("authority-a", "authority-b"))
            .await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent()[0].payload_bytes(), b"streamed");
    }

    #[tokio::test]
    async fn failed_subscription_fetch_prevents_wildcard_route() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = UStreamer::new("test", 8, failing_subscription_source())
            .await
            .expect("streamer should build");

        let err = streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress),
            )
            .await
            .expect_err("route should require a valid subscription snapshot");

        assert_eq!(err.get_code(), UCode::FAILED_PRECONDITION);
        assert!(ingress.filters().is_empty());
    }

    #[tokio::test]
    async fn add_route_rolls_back_partial_registration_failure() {
        let ingress = Arc::new(MemoryOwnedTransport::fail_on_registration(2));
        let egress = Arc::new(MemoryOwnedTransport::default());
        let mut streamer = UStreamer::new(
            "test",
            8,
            subscription_source_with(topic("authority-a"), topic("authority-b")),
        )
        .await
        .expect("streamer should build");

        let err = streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_owned("in", "authority-a", ingress.clone()),
                &OwnedFrameEndpoint::from_owned("out", "authority-b", egress),
            )
            .await
            .expect_err("second listener registration should fail");

        assert_eq!(err.get_code(), UCode::UNAVAILABLE);
        assert_eq!(ingress.listener_count(), 0);
    }

    #[tokio::test]
    async fn point_to_point_filters_work_for_owned_and_zero_copy_without_subscriptions() {
        let owned_ingress = Arc::new(MemoryOwnedTransport::default());
        let zero_copy_ingress = Arc::new(MemoryZeroCopyTransport::default());
        let owned_egress = Arc::new(MemoryOwnedTransport::default());
        let zero_copy_egress = Arc::new(MemoryZeroCopyTransport::default());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_owned("owned-in", "authority-a", owned_ingress.clone()),
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "zc-out",
                    "authority-b",
                    zero_copy_egress.clone(),
                ),
            )
            .await
            .expect("owned route should register");
        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "zc-in",
                    "authority-c",
                    zero_copy_ingress.clone(),
                ),
                &OwnedFrameEndpoint::from_owned("owned-out", "authority-d", owned_egress.clone()),
            )
            .await
            .expect("zero-copy route should register");

        owned_ingress
            .inject(point_to_point_frame("authority-a", "authority-b"))
            .await;
        zero_copy_ingress
            .inject(point_to_point_frame("authority-c", "authority-d"))
            .await;
        owned_ingress.inject(frame("authority-a")).await;
        zero_copy_ingress.inject(frame("authority-c")).await;
        yield_to_forwarder().await;

        assert_eq!(zero_copy_egress.sent().len(), 1);
        assert_eq!(owned_egress.sent().len(), 1);
    }

    #[tokio::test]
    async fn rejects_duplicate_and_missing_routes() {
        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let in_ep = OwnedFrameEndpoint::from_owned("in", "authority-a", ingress);
        let out_ep = OwnedFrameEndpoint::from_owned("out", "authority-b", egress);
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
