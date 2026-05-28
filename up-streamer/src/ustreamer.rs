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

use tokio::{sync::mpsc, sync::mpsc::error::TrySendError, task::JoinHandle};
use up_rust::usubscription::{
    from_proto_uri, FetchSubscriptionsRequest, FetchSubscriptionsResponse, USubscription,
};
#[cfg(feature = "experimental-loaned-frame")]
use up_rust::{
    copy_loaned_frame_payload_to_tx,
    zero_copy::{UZeroCopyListener, UZeroCopyRxFrame, UZeroCopyTransport},
    ZeroCopyLoanedFrame,
};
use up_rust::{
    transport::UOwnedFrameEndpointRegistration, UCode, UOwnedFrame, UOwnedListener, UStatus, UUri,
};

#[cfg(feature = "experimental-loaned-frame")]
use crate::copy_minimized::loan_spec_for_copy_minimized;
#[cfg(feature = "experimental-loaned-frame")]
use crate::{CopyMinimizedRouteOptions, ZeroCopyFrameEndpoint};
use crate::{
    DataPlaneFailureKind, DataPlaneHealth, DataPlaneRoute, OwnedFrameEndpoint, RouteDiagnostic,
    RouteKind, RouteOptions, RouteQueuePolicy, SubscriptionSyncHealth,
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
        Self::from_parts(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        )
    }

    #[cfg(feature = "experimental-loaned-frame")]
    fn new_zero_copy<I, E>(
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Self
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        Self::from_parts(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        )
    }

    fn from_parts(
        ingress_name: &str,
        ingress_authority: &str,
        egress_name: &str,
        egress_authority: &str,
    ) -> Self {
        Self {
            ingress_name: ingress_name.to_string(),
            ingress_authority: ingress_authority.to_string(),
            egress_name: egress_name.to_string(),
            egress_authority: egress_authority.to_string(),
        }
    }
}

struct RouteBinding {
    ingress: OwnedFrameEndpoint,
    egress: OwnedFrameEndpoint,
    tx: mpsc::Sender<UOwnedFrame>,
    queue_policy: RouteQueuePolicy,
    registrations: Vec<UOwnedFrameEndpointRegistration>,
    dispatch_task: JoinHandle<()>,
}

struct IngressForwarder {
    tx: mpsc::Sender<UOwnedFrame>,
    route: DataPlaneRoute,
    queue_policy: RouteQueuePolicy,
    data_plane_health: Arc<Mutex<DataPlaneHealth>>,
}

#[async_trait::async_trait]
impl UOwnedListener for IngressForwarder {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        match self.queue_policy {
            RouteQueuePolicy::Backpressure => {
                if self.tx.send(frame).await.is_err() {
                    record_ingress_queue_closed(&self.data_plane_health, &self.route);
                }
            }
            RouteQueuePolicy::DropAndReport => match self.tx.try_send(frame) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    tracing::warn!(
                        ingress = %self.route.ingress_name,
                        ingress_authority = %self.route.ingress_authority,
                        egress = %self.route.egress_name,
                        egress_authority = %self.route.egress_authority,
                        "ingress_queue_full_drop"
                    );
                    record_data_plane_failure(
                        &self.data_plane_health,
                        DataPlaneFailureKind::IngressQueueFull,
                        self.route.clone(),
                        "route ingress queue full; frame dropped by drop-and-report policy",
                    );
                }
                Err(TrySendError::Closed(_)) => {
                    record_ingress_queue_closed(&self.data_plane_health, &self.route);
                }
            },
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
    #[cfg(feature = "experimental-loaned-frame")]
    copy_minimized_routes: HashMap<RouteKey, Box<dyn CopyMinimizedRouteOps>>,
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
            #[cfg(feature = "experimental-loaned-frame")]
            copy_minimized_routes: HashMap::new(),
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

    /// Returns route diagnostics for installed owned and copy-minimized routes.
    pub fn route_diagnostics(&self) -> Vec<RouteDiagnostic> {
        let mut diagnostics = Vec::with_capacity(self.routes.len());
        diagnostics.extend(self.routes.values().map(|binding| RouteDiagnostic {
            route: data_plane_route(&binding.ingress, &binding.egress),
            ingress_mode: binding.ingress.mode(),
            egress_mode: binding.egress.mode(),
            route_kind: route_kind_for_modes(binding.ingress.mode(), binding.egress.mode()),
        }));

        #[cfg(feature = "experimental-loaned-frame")]
        diagnostics.extend(
            self.copy_minimized_routes
                .values()
                .map(|binding| binding.diagnostic()),
        );

        diagnostics
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
        self.add_route_ref_with_options(ingress, egress, RouteOptions::default())
            .await
    }

    /// Adds a route from `ingress` to `egress` with explicit route options.
    ///
    /// The default [`RouteOptions`] preserves backpressure on full route queues.
    /// Use [`RouteQueuePolicy::DropAndReport`] only when bounded latency and
    /// explicit drop accounting are preferred over listener backpressure.
    ///
    /// # Errors
    ///
    /// Returns an error when authorities are identical, no successful
    /// subscription snapshot is available, the route already exists, or ingress
    /// listener registration fails.
    pub async fn add_route_ref_with_options(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
        options: RouteOptions,
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
        if self.route_key_exists(&route_key) {
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
                        queue_policy: options.queue_policy,
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
                queue_policy: options.queue_policy,
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

    /// Adds a route with explicit options, consuming endpoint values after registration.
    ///
    /// This is a convenience wrapper around [`Self::add_route_ref_with_options`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::add_route_ref_with_options`].
    pub async fn add_route_with_options(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
        options: RouteOptions,
    ) -> Result<(), UStatus> {
        self.add_route_ref_with_options(&ingress, &egress, options)
            .await
    }

    /// Adds an experimental copy-minimized route between zero-copy endpoints.
    ///
    /// This route mode keeps ingress receive leases out of the owned-frame router
    /// and copies ordered payload slices directly into egress transmit loans. It
    /// still copies payload bytes into the egress loan and must not be described
    /// as zero-copy-preserving forwarding.
    ///
    /// # Errors
    ///
    /// Returns an error when authorities are identical, no successful
    /// subscription snapshot is available, the route already exists, or ingress
    /// zero-copy listener registration fails.
    #[cfg(feature = "experimental-loaned-frame")]
    #[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
    pub async fn add_copy_minimized_route_ref<I, E>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxFrame + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.add_copy_minimized_route_ref_with_options(
            ingress,
            egress,
            CopyMinimizedRouteOptions::default(),
        )
        .await
    }

    /// Adds an experimental copy-minimized route with explicit options.
    ///
    /// `options.alignment` is passed to the egress zero-copy transport when each
    /// transmit loan is reserved. `options.queue_policy` controls the bounded
    /// ingress worker queue and defaults to backpressure.
    ///
    /// # Errors
    ///
    /// Returns an error when authorities are identical, no successful
    /// subscription snapshot is available, the route already exists, or ingress
    /// zero-copy listener registration fails.
    #[cfg(feature = "experimental-loaned-frame")]
    #[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
    pub async fn add_copy_minimized_route_ref_with_options<I, E>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxFrame + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
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

        let route_key = RouteKey::new_zero_copy(ingress, egress);
        if self.route_key_exists(&route_key) {
            return Err(UStatus::fail_with_code(
                UCode::ALREADY_EXISTS,
                "route already exists",
            ));
        }

        tracing::debug!(
            ingress = %ingress.name,
            ingress_authority = %ingress.authority,
            ingress_mode = ?ingress.mode(),
            egress = %egress.name,
            egress_authority = %egress.authority,
            egress_mode = ?egress.mode(),
            route_kind = ?RouteKind::CopyMinimizedZeroCopyToZeroCopy,
            "route_create"
        );

        let binding = CopyMinimizedRouteBinding::new(
            ingress,
            egress,
            filters_for_authorities(
                &self.subscription_snapshot,
                &ingress.authority,
                &egress.authority,
            ),
            self.message_queue_size,
            options,
            self.data_plane_health.clone(),
        )
        .await?;
        self.copy_minimized_routes
            .insert(route_key, Box::new(binding));
        Ok(())
    }

    /// Adds a copy-minimized route, consuming endpoint values after registration.
    ///
    /// This is a convenience wrapper around [`Self::add_copy_minimized_route_ref`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::add_copy_minimized_route_ref`].
    #[cfg(feature = "experimental-loaned-frame")]
    #[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
    pub async fn add_copy_minimized_route<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxFrame + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.add_copy_minimized_route_ref(&ingress, &egress).await
    }

    /// Adds a copy-minimized route with explicit options, consuming endpoint values.
    ///
    /// This is a convenience wrapper around
    /// [`Self::add_copy_minimized_route_ref_with_options`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::add_copy_minimized_route_ref_with_options`].
    #[cfg(feature = "experimental-loaned-frame")]
    #[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
    pub async fn add_copy_minimized_route_with_options<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxFrame + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.add_copy_minimized_route_ref_with_options(&ingress, &egress, options)
            .await
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

    /// Deletes an experimental copy-minimized route and unregisters its listeners.
    ///
    /// If any unregister operation fails, the route is restored with the
    /// remaining registrations so callers can retry deletion.
    ///
    /// # Errors
    ///
    /// Returns an error when authorities are identical, the route does not exist,
    /// or an underlying unregister operation fails.
    #[cfg(feature = "experimental-loaned-frame")]
    #[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
    pub async fn delete_copy_minimized_route_ref<I, E>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = RouteKey::new_zero_copy(ingress, egress);
        let Some(mut binding) = self.copy_minimized_routes.remove(&route_key) else {
            return Err(UStatus::fail_with_code(UCode::NOT_FOUND, "route not found"));
        };
        if let Err(err) = binding.unregister_for_delete().await {
            self.copy_minimized_routes.insert(route_key, binding);
            return Err(err);
        }
        binding.abort_dispatch();
        Ok(())
    }

    /// Deletes a copy-minimized route, consuming endpoint values after lookup.
    ///
    /// This is a convenience wrapper around [`Self::delete_copy_minimized_route_ref`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::delete_copy_minimized_route_ref`].
    #[cfg(feature = "experimental-loaned-frame")]
    #[cfg_attr(docsrs, doc(cfg(feature = "experimental-loaned-frame")))]
    pub async fn delete_copy_minimized_route<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.delete_copy_minimized_route_ref(&ingress, &egress)
            .await
    }
}

#[cfg(feature = "experimental-loaned-frame")]
#[async_trait::async_trait]
trait CopyMinimizedRouteOps: Send {
    fn diagnostic(&self) -> RouteDiagnostic;
    async fn rewire(&mut self, snapshot: &FetchSubscriptionsResponse) -> Result<(), UStatus>;
    async fn unregister_for_delete(&mut self) -> Result<(), UStatus>;
    fn abort_dispatch(&self);
}

#[cfg(feature = "experimental-loaned-frame")]
struct CopyMinimizedRouteBinding<I, E>
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxFrame + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    ingress: ZeroCopyFrameEndpoint<I>,
    egress: ZeroCopyFrameEndpoint<E>,
    tx: mpsc::Sender<I::Rx>,
    options: CopyMinimizedRouteOptions,
    registrations: Vec<ZeroCopyEndpointRegistration<I>>,
    dispatch_task: JoinHandle<()>,
    data_plane_health: Arc<Mutex<DataPlaneHealth>>,
}

#[cfg(feature = "experimental-loaned-frame")]
impl<I, E> CopyMinimizedRouteBinding<I, E>
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxFrame + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    async fn new(
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
        filters: Vec<RouteFilter>,
        message_queue_size: usize,
        options: CopyMinimizedRouteOptions,
        data_plane_health: Arc<Mutex<DataPlaneHealth>>,
    ) -> Result<Self, UStatus> {
        let (tx, mut rx) = mpsc::channel::<I::Rx>(message_queue_size);
        let route = data_plane_route_for_parts(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        );
        let egress_transport = egress.transport.clone();
        let dispatch_health = data_plane_health.clone();
        let ingress_name = ingress.name.clone();
        let ingress_authority = ingress.authority.clone();
        let egress_name = egress.name.clone();
        let egress_authority = egress.authority.clone();
        let alignment = options.alignment;
        let dispatch_task = tokio::spawn(async move {
            let mut recent_frame_ids = HashSet::new();
            let mut recent_frame_order = VecDeque::new();
            tracing::debug!(
                ingress = %ingress_name,
                ingress_authority = %ingress_authority,
                ingress_mode = ?crate::TransportMode::ZeroCopy,
                egress = %egress_name,
                egress_authority = %egress_authority,
                egress_mode = ?crate::TransportMode::ZeroCopy,
                route_kind = ?RouteKind::CopyMinimizedZeroCopyToZeroCopy,
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

                let loaned = ZeroCopyLoanedFrame::new(frame);
                let spec = match loan_spec_for_copy_minimized(&loaned, alignment) {
                    Ok(spec) => spec,
                    Err(err) => {
                        tracing::warn!(
                            ingress = %ingress_name,
                            ingress_authority = %ingress_authority,
                            egress = %egress_name,
                            egress_authority = %egress_authority,
                            route_kind = ?RouteKind::CopyMinimizedZeroCopyToZeroCopy,
                            ?err,
                            "egress_payload_layout_rejected"
                        );
                        record_data_plane_failure(
                            &dispatch_health,
                            DataPlaneFailureKind::CopyMinimizedPayloadLayout,
                            route.clone(),
                            format!("{err:?}"),
                        );
                        continue;
                    }
                };

                let send_result = match egress_transport.loan_tx(spec).await {
                    Ok(mut tx) => {
                        let copy_result = copy_loaned_frame_payload_to_tx(&loaned, &mut tx)
                            .map_err(UStatus::from);
                        match copy_result {
                            Ok(_) => egress_transport.send_zero_copy(tx).await,
                            Err(err) => Err(err),
                        }
                    }
                    Err(err) => Err(err),
                };
                match send_result {
                    Ok(()) => tracing::debug!(
                        ingress = %ingress_name,
                        ingress_authority = %ingress_authority,
                        egress = %egress_name,
                        egress_authority = %egress_authority,
                        route_kind = ?RouteKind::CopyMinimizedZeroCopyToZeroCopy,
                        "egress_send_ok"
                    ),
                    Err(err) => {
                        tracing::warn!(
                            ingress = %ingress_name,
                            ingress_authority = %ingress_authority,
                            egress = %egress_name,
                            egress_authority = %egress_authority,
                            route_kind = ?RouteKind::CopyMinimizedZeroCopyToZeroCopy,
                            ?err,
                            "egress_send_failed"
                        );
                        record_data_plane_failure(
                            &dispatch_health,
                            DataPlaneFailureKind::EgressSend,
                            route.clone(),
                            format!("{err:?}"),
                        );
                    }
                }
            }
        });

        let mut binding = Self {
            ingress: ZeroCopyFrameEndpoint::new(
                &ingress.name,
                &ingress.authority,
                ingress.transport.clone(),
            ),
            egress: ZeroCopyFrameEndpoint::new(
                &egress.name,
                &egress.authority,
                egress.transport.clone(),
            ),
            tx,
            options,
            registrations: Vec::new(),
            dispatch_task,
            data_plane_health,
        };
        for route_filter in filters {
            match binding.register_filter(route_filter).await {
                Ok(registration) => binding.registrations.push(registration),
                Err(err) => {
                    for registration in binding.registrations.drain(..) {
                        let _ = registration.unregister().await;
                    }
                    binding.dispatch_task.abort();
                    return Err(err);
                }
            }
        }
        Ok(binding)
    }

    async fn register_filter(
        &self,
        route_filter: RouteFilter,
    ) -> Result<ZeroCopyEndpointRegistration<I>, UStatus> {
        let listener: Arc<dyn UZeroCopyListener<I::Rx>> = Arc::new(CopyMinimizedIngressForwarder {
            tx: self.tx.clone(),
            route: data_plane_route_for_parts(
                &self.ingress.name,
                &self.ingress.authority,
                &self.egress.name,
                &self.egress.authority,
            ),
            queue_policy: self.options.queue_policy,
            data_plane_health: self.data_plane_health.clone(),
        });
        self.ingress
            .transport
            .register_zero_copy_listener(
                &route_filter.source,
                route_filter.sink.as_ref(),
                listener.clone(),
            )
            .await?;
        Ok(ZeroCopyEndpointRegistration {
            transport: self.ingress.transport.clone(),
            source_filter: route_filter.source,
            sink_filter: route_filter.sink,
            listener,
        })
    }
}

#[cfg(feature = "experimental-loaned-frame")]
#[async_trait::async_trait]
impl<I, E> CopyMinimizedRouteOps for CopyMinimizedRouteBinding<I, E>
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxFrame + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    fn diagnostic(&self) -> RouteDiagnostic {
        RouteDiagnostic {
            route: data_plane_route_for_parts(
                &self.ingress.name,
                &self.ingress.authority,
                &self.egress.name,
                &self.egress.authority,
            ),
            ingress_mode: crate::TransportMode::ZeroCopy,
            egress_mode: crate::TransportMode::ZeroCopy,
            route_kind: RouteKind::CopyMinimizedZeroCopyToZeroCopy,
        }
    }

    async fn rewire(&mut self, snapshot: &FetchSubscriptionsResponse) -> Result<(), UStatus> {
        let filters =
            filters_for_authorities(snapshot, &self.ingress.authority, &self.egress.authority);
        let mut new_registrations = Vec::new();
        for route_filter in filters {
            match self.register_filter(route_filter).await {
                Ok(registration) => new_registrations.push(registration),
                Err(err) => {
                    for registration in new_registrations {
                        let _ = registration.unregister().await;
                    }
                    return Err(err);
                }
            }
        }

        let route = data_plane_route_for_parts(
            &self.ingress.name,
            &self.ingress.authority,
            &self.egress.name,
            &self.egress.authority,
        );
        let old_registrations = std::mem::take(&mut self.registrations);
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
        self.registrations = new_registrations;
        Ok(())
    }

    async fn unregister_for_delete(&mut self) -> Result<(), UStatus> {
        let registrations = std::mem::take(&mut self.registrations);
        let mut remaining_registrations = Vec::new();
        let mut first_err = None;
        for registration in registrations {
            if let Err(err) = registration.unregister().await {
                if first_err.is_none() {
                    first_err = Some(err);
                }
                remaining_registrations.push(registration);
            }
        }
        if let Some(err) = first_err {
            self.registrations = remaining_registrations;
            return Err(err);
        }
        Ok(())
    }

    fn abort_dispatch(&self) {
        self.dispatch_task.abort();
    }
}

#[cfg(feature = "experimental-loaned-frame")]
struct ZeroCopyEndpointRegistration<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    transport: Arc<T>,
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UZeroCopyListener<T::Rx>>,
}

#[cfg(feature = "experimental-loaned-frame")]
impl<T> ZeroCopyEndpointRegistration<T>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
    async fn unregister(&self) -> Result<(), UStatus> {
        self.transport
            .unregister_zero_copy_listener(
                &self.source_filter,
                self.sink_filter.as_ref(),
                self.listener.clone(),
            )
            .await
    }
}

#[cfg(feature = "experimental-loaned-frame")]
struct CopyMinimizedIngressForwarder<Rx>
where
    Rx: UZeroCopyRxFrame + Send + 'static,
{
    tx: mpsc::Sender<Rx>,
    route: DataPlaneRoute,
    queue_policy: RouteQueuePolicy,
    data_plane_health: Arc<Mutex<DataPlaneHealth>>,
}

#[cfg(feature = "experimental-loaned-frame")]
#[async_trait::async_trait]
impl<Rx> UZeroCopyListener<Rx> for CopyMinimizedIngressForwarder<Rx>
where
    Rx: UZeroCopyRxFrame + Send + 'static,
{
    async fn on_receive_zero_copy(&self, frame: Rx) {
        match self.queue_policy {
            RouteQueuePolicy::Backpressure => {
                if self.tx.send(frame).await.is_err() {
                    record_ingress_queue_closed(&self.data_plane_health, &self.route);
                }
            }
            RouteQueuePolicy::DropAndReport => match self.tx.try_send(frame) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    tracing::warn!(
                        ingress = %self.route.ingress_name,
                        ingress_authority = %self.route.ingress_authority,
                        egress = %self.route.egress_name,
                        egress_authority = %self.route.egress_authority,
                        route_kind = ?RouteKind::CopyMinimizedZeroCopyToZeroCopy,
                        "ingress_queue_full_drop"
                    );
                    record_data_plane_failure(
                        &self.data_plane_health,
                        DataPlaneFailureKind::IngressQueueFull,
                        self.route.clone(),
                        "route ingress queue full; frame dropped by drop-and-report policy",
                    );
                }
                Err(TrySendError::Closed(_)) => {
                    record_ingress_queue_closed(&self.data_plane_health, &self.route);
                }
            },
        }
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
                            queue_policy: binding.queue_policy,
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
        #[cfg(feature = "experimental-loaned-frame")]
        self.rewire_copy_minimized_routes(snapshot).await?;
        Ok(())
    }

    fn filters_for_route(
        &self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Vec<RouteFilter> {
        filters_for_snapshot(&self.subscription_snapshot, ingress, egress)
    }

    fn route_key_exists(&self, route_key: &RouteKey) -> bool {
        self.routes.contains_key(route_key) || {
            #[cfg(feature = "experimental-loaned-frame")]
            {
                self.copy_minimized_routes.contains_key(route_key)
            }
            #[cfg(not(feature = "experimental-loaned-frame"))]
            {
                false
            }
        }
    }

    #[cfg(feature = "experimental-loaned-frame")]
    async fn rewire_copy_minimized_routes(
        &mut self,
        snapshot: &FetchSubscriptionsResponse,
    ) -> Result<(), UStatus> {
        for binding in self.copy_minimized_routes.values_mut() {
            binding.rewire(snapshot).await?;
        }
        Ok(())
    }
}

fn filters_for_snapshot(
    snapshot: &FetchSubscriptionsResponse,
    ingress: &OwnedFrameEndpoint,
    egress: &OwnedFrameEndpoint,
) -> Vec<RouteFilter> {
    filters_for_authorities(snapshot, &ingress.authority, &egress.authority)
}

fn filters_for_authorities(
    snapshot: &FetchSubscriptionsResponse,
    ingress_authority: &str,
    egress_authority: &str,
) -> Vec<RouteFilter> {
    let mut filters = vec![RouteFilter {
        source: authority_to_wildcard_filter(ingress_authority),
        sink: Some(authority_to_wildcard_filter(egress_authority)),
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
        let topic_matches_ingress = topic_authority == ingress_authority || topic_authority == "*";
        let subscriber_matches_egress =
            subscriber_authority == egress_authority || subscriber_authority == "*";
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
        #[cfg(feature = "experimental-loaned-frame")]
        for binding in self.copy_minimized_routes.values() {
            binding.abort_dispatch();
        }
    }
}

fn authority_to_wildcard_filter(authority_name: &str) -> UUri {
    UUri::try_from_parts(authority_name, 0xFFFF_FFFF, 0xFF, 0xFFFF)
        .expect("wildcard URI authority must be valid")
}

fn data_plane_route(ingress: &OwnedFrameEndpoint, egress: &OwnedFrameEndpoint) -> DataPlaneRoute {
    data_plane_route_for_parts(
        &ingress.name,
        &ingress.authority,
        &egress.name,
        &egress.authority,
    )
}

fn data_plane_route_for_parts(
    ingress_name: &str,
    ingress_authority: &str,
    egress_name: &str,
    egress_authority: &str,
) -> DataPlaneRoute {
    DataPlaneRoute {
        ingress_name: ingress_name.to_string(),
        ingress_authority: ingress_authority.to_string(),
        egress_name: egress_name.to_string(),
        egress_authority: egress_authority.to_string(),
    }
}

fn route_kind_for_modes(
    ingress_mode: crate::TransportMode,
    egress_mode: crate::TransportMode,
) -> RouteKind {
    match (ingress_mode, egress_mode) {
        (crate::TransportMode::Owned, crate::TransportMode::Owned) => RouteKind::OwnedToOwned,
        (crate::TransportMode::Owned, crate::TransportMode::ZeroCopy) => {
            RouteKind::OwnedToZeroCopyAdapter
        }
        (crate::TransportMode::ZeroCopy, crate::TransportMode::Owned) => {
            RouteKind::ZeroCopyAdapterToOwned
        }
        (crate::TransportMode::ZeroCopy, crate::TransportMode::ZeroCopy) => {
            RouteKind::ZeroCopyAdapterToZeroCopyAdapter
        }
    }
}

fn record_ingress_queue_closed(health: &Arc<Mutex<DataPlaneHealth>>, route: &DataPlaneRoute) {
    tracing::warn!(
        ingress = %route.ingress_name,
        ingress_authority = %route.ingress_authority,
        egress = %route.egress_name,
        egress_authority = %route.egress_authority,
        "ingress_queue_closed"
    );
    record_data_plane_failure(
        health,
        DataPlaneFailureKind::IngressQueueClosed,
        route.clone(),
        "route ingress queue closed before frame could be forwarded",
    );
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
        UOwnedListener, UOwnedTransport, UTxLoanSpec,
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
        fail_send: Mutex<bool>,
        fail_on_unregister: Mutex<bool>,
        loan_alignments: Mutex<Vec<usize>>,
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

        #[cfg(feature = "experimental-loaned-frame")]
        fn fail_sends() -> Self {
            Self {
                fail_send: Mutex::new(true),
                ..Default::default()
            }
        }

        #[cfg(feature = "experimental-loaned-frame")]
        fn fail_on_unregister() -> Self {
            Self {
                fail_on_unregister: Mutex::new(true),
                ..Default::default()
            }
        }

        #[cfg(feature = "experimental-loaned-frame")]
        fn listener_count(&self) -> usize {
            self.listeners
                .lock()
                .expect("listeners lock poisoned")
                .len()
        }

        #[cfg(feature = "experimental-loaned-frame")]
        fn loan_alignments(&self) -> Vec<usize> {
            self.loan_alignments
                .lock()
                .expect("loan_alignments lock poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl UZeroCopyTransport for MemoryZeroCopyTransport {
        type Tx = UVecTxBuffer;
        type Rx = UOwnedFrame;

        async fn loan_tx(&self, spec: UTxLoanSpec) -> Result<Self::Tx, UStatus> {
            self.loan_alignments
                .lock()
                .expect("loan_alignments lock poisoned")
                .push(spec.payload_alignment());
            UVecTxBuffer::with_alignment(
                spec.metadata().clone(),
                spec.payload_len(),
                spec.payload_alignment(),
            )
            .map_err(UStatus::from)
        }

        async fn send_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
            if *self.fail_send.lock().expect("fail_send lock poisoned") {
                return Err(UStatus::fail_with_code(UCode::UNAVAILABLE, "send failed"));
            }
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
        UFrameBuilder::notification(
            point_to_point_source(source_authority),
            point_to_point_sink(sink_authority),
        )
        .build_with_raw_payload("streamed")
        .expect("valid notification frame")
    }

    fn point_to_point_source(authority: &str) -> UUri {
        UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid source URI")
    }

    fn point_to_point_sink(authority: &str) -> UUri {
        UUri::try_from_parts(authority, 0x4220, 1, 0).expect("valid sink URI")
    }

    #[cfg(feature = "experimental-loaned-frame")]
    fn point_to_point_metadata(source_authority: &str, sink_authority: &str) -> UFrameMetadata {
        UFrameMetadata::notification(
            point_to_point_source(source_authority),
            point_to_point_sink(sink_authority),
        )
    }

    #[cfg(feature = "experimental-loaned-frame")]
    fn stable_point_to_point_frame(source_authority: &str, sink_authority: &str) -> UOwnedFrame {
        UOwnedFrame::from_payload_as::<StableContainerPayload<VehiclePose>, VehiclePose>(
            point_to_point_metadata(source_authority, sink_authority),
            &VehiclePose { x: 3, y: 5 },
        )
        .expect("stable-container payload should encode as owned bytes")
    }

    #[cfg(feature = "experimental-loaned-frame")]
    fn malformed_stable_point_to_point_frame(
        source_authority: &str,
        sink_authority: &str,
    ) -> UOwnedFrame {
        let encoding = PayloadEncoding::custom(
            up_rust::StableContainerPayloadInfo::ENCODING_ID,
            "application/vnd.uprotocol.stable-container;variant=fixed;size=8;align=4",
        );
        UOwnedFrame::new(
            point_to_point_metadata(source_authority, sink_authority).with_encoding(encoding),
            vec![0_u8; std::mem::size_of::<VehiclePose>()],
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
            queue_policy: RouteQueuePolicy::Backpressure,
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
    async fn ingress_forwarder_drop_and_report_records_full_route_queue() {
        let (tx, _rx) = mpsc::channel(1);
        let data_plane_health = Arc::new(Mutex::new(DataPlaneHealth::default()));
        let forwarder = IngressForwarder {
            tx,
            route: DataPlaneRoute {
                ingress_name: "in".to_string(),
                ingress_authority: "authority-a".to_string(),
                egress_name: "out".to_string(),
                egress_authority: "authority-b".to_string(),
            },
            queue_policy: RouteQueuePolicy::DropAndReport,
            data_plane_health: data_plane_health.clone(),
        };

        forwarder.on_receive_owned(frame("authority-a")).await;
        forwarder.on_receive_owned(frame("authority-a")).await;

        let health = data_plane_health
            .lock()
            .expect("data-plane health lock poisoned")
            .clone();
        assert_eq!(health.ingress_queue_full_drops, 1);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::IngressQueueFull
        );
    }

    #[test]
    fn route_options_default_to_backpressure() {
        assert_eq!(
            RouteOptions::default().queue_policy,
            RouteQueuePolicy::Backpressure
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
    async fn route_diagnostics_report_owned_route_kinds() {
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        let owned_in = Arc::new(MemoryOwnedTransport::default());
        let owned_out = Arc::new(MemoryOwnedTransport::default());
        let zc_in = Arc::new(MemoryZeroCopyTransport::default());
        let zc_out = Arc::new(MemoryZeroCopyTransport::default());

        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_owned("owned-in-a", "authority-a", owned_in.clone()),
                &OwnedFrameEndpoint::from_owned("owned-out-b", "authority-b", owned_out.clone()),
            )
            .await
            .expect("owned to owned route should register");
        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_owned("owned-in-c", "authority-c", owned_in.clone()),
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "zc-out-d",
                    "authority-d",
                    zc_out.clone(),
                ),
            )
            .await
            .expect("owned to zero-copy adapter route should register");
        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "zc-in-e",
                    "authority-e",
                    zc_in.clone(),
                ),
                &OwnedFrameEndpoint::from_owned("owned-out-f", "authority-f", owned_out.clone()),
            )
            .await
            .expect("zero-copy adapter to owned route should register");
        streamer
            .add_route_ref(
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "zc-in-g",
                    "authority-g",
                    zc_in,
                ),
                &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                    "zc-out-h",
                    "authority-h",
                    zc_out,
                ),
            )
            .await
            .expect("zero-copy adapter to zero-copy adapter route should register");

        let kinds: HashSet<_> = streamer
            .route_diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.route_kind)
            .collect();

        assert!(kinds.contains(&RouteKind::OwnedToOwned));
        assert!(kinds.contains(&RouteKind::OwnedToZeroCopyAdapter));
        assert!(kinds.contains(&RouteKind::ZeroCopyAdapterToOwned));
        assert!(kinds.contains(&RouteKind::ZeroCopyAdapterToZeroCopyAdapter));
    }

    #[cfg(feature = "experimental-loaned-frame")]
    fn zero_copy_endpoint(
        name: &str,
        authority: &str,
        transport: Arc<MemoryZeroCopyTransport>,
    ) -> ZeroCopyFrameEndpoint<MemoryZeroCopyTransport> {
        ZeroCopyFrameEndpoint::new(name, authority, transport)
    }

    #[cfg(feature = "experimental-loaned-frame")]
    #[tokio::test]
    async fn copy_minimized_route_forwards_and_reports_diagnostics() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let in_ep = zero_copy_endpoint("in", "authority-a", ingress.clone());
        let out_ep = zero_copy_endpoint("out", "authority-b", egress.clone());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_copy_minimized_route_ref_with_options(
                &in_ep,
                &out_ep,
                CopyMinimizedRouteOptions {
                    alignment: 8,
                    queue_policy: RouteQueuePolicy::Backpressure,
                },
            )
            .await
            .expect("copy-minimized route should register");

        let diagnostics = streamer.route_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].route_kind,
            RouteKind::CopyMinimizedZeroCopyToZeroCopy
        );
        assert_eq!(diagnostics[0].ingress_mode, crate::TransportMode::ZeroCopy);
        assert_eq!(diagnostics[0].egress_mode, crate::TransportMode::ZeroCopy);

        ingress
            .inject(point_to_point_frame("authority-a", "authority-b"))
            .await;
        yield_to_forwarder().await;

        let sent = egress.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].payload_bytes(), b"streamed");
        assert_eq!(egress.loan_alignments(), vec![8]);

        streamer
            .delete_copy_minimized_route_ref(&in_ep, &out_ep)
            .await
            .expect("copy-minimized route should delete");
        ingress
            .inject(point_to_point_frame("authority-a", "authority-b"))
            .await;
        yield_to_forwarder().await;
        assert_eq!(egress.sent().len(), 1);
    }

    #[cfg(feature = "experimental-loaned-frame")]
    #[tokio::test]
    async fn copy_minimized_route_rejects_underaligned_stable_container_payload() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let in_ep = zero_copy_endpoint("in", "authority-a", ingress.clone());
        let out_ep = zero_copy_endpoint("out", "authority-b", egress.clone());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_copy_minimized_route_ref_with_options(
                &in_ep,
                &out_ep,
                CopyMinimizedRouteOptions {
                    alignment: 1,
                    queue_policy: RouteQueuePolicy::Backpressure,
                },
            )
            .await
            .expect("copy-minimized route should register");

        ingress
            .inject(stable_point_to_point_frame("authority-a", "authority-b"))
            .await;
        yield_to_forwarder().await;

        assert!(egress.sent().is_empty());
        assert!(egress.loan_alignments().is_empty());
        let health = streamer.data_plane_health();
        assert_eq!(health.copy_minimized_payload_layout_failures, 1);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::CopyMinimizedPayloadLayout
        );
    }

    #[cfg(feature = "experimental-loaned-frame")]
    #[tokio::test]
    async fn copy_minimized_route_rejects_malformed_stable_container_metadata() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let in_ep = zero_copy_endpoint("in", "authority-a", ingress.clone());
        let out_ep = zero_copy_endpoint("out", "authority-b", egress.clone());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_copy_minimized_route_ref_with_options(
                &in_ep,
                &out_ep,
                CopyMinimizedRouteOptions {
                    alignment: std::mem::align_of::<VehiclePose>(),
                    queue_policy: RouteQueuePolicy::Backpressure,
                },
            )
            .await
            .expect("copy-minimized route should register");

        ingress
            .inject(malformed_stable_point_to_point_frame(
                "authority-a",
                "authority-b",
            ))
            .await;
        yield_to_forwarder().await;

        assert!(egress.sent().is_empty());
        assert!(egress.loan_alignments().is_empty());
        let health = streamer.data_plane_health();
        assert_eq!(health.copy_minimized_payload_layout_failures, 1);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::CopyMinimizedPayloadLayout
        );
    }

    #[cfg(feature = "experimental-loaned-frame")]
    #[tokio::test]
    async fn copy_minimized_route_refresh_keeps_route_and_suppresses_duplicates() {
        let ingress = Arc::new(MemoryZeroCopyTransport::fail_on_unregister());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let in_ep = zero_copy_endpoint("in", "authority-a", ingress.clone());
        let out_ep = zero_copy_endpoint("out", "authority-b", egress.clone());
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_copy_minimized_route_ref(&in_ep, &out_ep)
            .await
            .expect("copy-minimized route should register");
        streamer
            .refresh_subscriptions()
            .await
            .expect("refresh should keep route installed");
        assert_eq!(ingress.listener_count(), 2);

        ingress
            .inject(point_to_point_frame("authority-a", "authority-b"))
            .await;
        yield_to_forwarder().await;

        assert_eq!(egress.sent().len(), 1);
        assert_eq!(streamer.route_diagnostics().len(), 1);
        let health = streamer.data_plane_health();
        assert_eq!(health.route_rewire_unregister_failures, 1);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::RouteRewireUnregister
        );
    }

    #[cfg(feature = "experimental-loaned-frame")]
    #[tokio::test]
    async fn copy_minimized_egress_failure_updates_data_plane_health() {
        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::fail_sends());
        let in_ep = zero_copy_endpoint("in", "authority-a", ingress.clone());
        let out_ep = zero_copy_endpoint("out", "authority-b", egress);
        let mut streamer = UStreamer::new("test", 8, subscription_source())
            .await
            .expect("streamer should build");

        streamer
            .add_copy_minimized_route_ref(&in_ep, &out_ep)
            .await
            .expect("copy-minimized route should register");
        ingress
            .inject(point_to_point_frame("authority-a", "authority-b"))
            .await;
        yield_to_forwarder().await;

        let health = streamer.data_plane_health();
        assert_eq!(health.egress_send_failures, 1);
        assert_eq!(
            health.last_failure.expect("last failure recorded").kind,
            DataPlaneFailureKind::EgressSend
        );
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
