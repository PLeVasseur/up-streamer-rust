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

use crate::control_plane::route_lifecycle::{AddRouteError, RemoveRouteError, RouteLifecycle};
use crate::control_plane::route_table::{RouteKey, RouteTable};
#[cfg(feature = "experimental-copy-minimized-routing")]
use crate::copy_minimized::{copy_frame_payload_to_tx, loan_spec_for_copy_minimized};
use crate::data_plane::egress_pool::EgressRoutePool;
use crate::data_plane::ingress_registry::IngressRouteRegistry;
use crate::endpoint::Endpoint;
#[cfg(feature = "owned-frame-transport")]
use crate::endpoint::OwnedFrameEndpoint;
#[cfg(feature = "experimental-copy-minimized-routing")]
use crate::endpoint::ZeroCopyFrameEndpoint;
use crate::observability::events;
#[cfg(any(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
use crate::routing::authority_filter::authority_to_wildcard_filter;
#[cfg(any(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
use crate::routing::publish_resolution::PublishRouteResolver;
use crate::routing::subscription_directory::SubscriptionDirectory;
use crate::subscription_sync_health::SubscriptionSyncHealth;
#[cfg(feature = "experimental-copy-minimized-routing")]
use crate::CopyMinimizedRouteOptions;
use crate::RouteDiagnostic;
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
use tokio::sync::mpsc;
use tracing::{debug, error, warn};
use up_rust::core::usubscription::{SubscriptionInfo, USubscription};
#[cfg(feature = "owned-frame-transport")]
use up_rust::{
    try_project_frame_to_umessage, try_project_umessage_to_frame_metadata, UOwnedFrame,
    UOwnedListener,
};
use up_rust::{UCode, UStatus, UUri};
#[cfg(feature = "experimental-copy-minimized-routing")]
use up_rust::{UWire, UWireMetadataCodec, UWireTransport, UZeroCopyTransportCore};
#[cfg(feature = "experimental-copy-minimized-routing")]
use up_rust::{UZeroCopyListener, UZeroCopyRxLease, UZeroCopyTransport};

const COMPONENT: &str = "ustreamer";

#[cfg(feature = "owned-frame-transport")]
type OwnedListenerFilter = (UUri, Option<UUri>);

#[cfg(feature = "experimental-copy-minimized-routing")]
type ZeroCopyListenerFilter = (UUri, Option<UUri>);

#[cfg(feature = "owned-frame-transport")]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct OwnedRouteKey {
    ingress_name: String,
    ingress_authority: String,
    egress_name: String,
    egress_authority: String,
}

#[cfg(feature = "owned-frame-transport")]
impl OwnedRouteKey {
    fn new(ingress: &OwnedFrameEndpoint, egress: &OwnedFrameEndpoint) -> Self {
        Self {
            ingress_name: ingress.name.clone(),
            ingress_authority: ingress.authority.clone(),
            egress_name: egress.name.clone(),
            egress_authority: egress.authority.clone(),
        }
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ZeroCopyRouteKey {
    ingress_name: String,
    ingress_authority: String,
    egress_name: String,
    egress_authority: String,
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl ZeroCopyRouteKey {
    fn new<I, E>(ingress: &ZeroCopyFrameEndpoint<I>, egress: &ZeroCopyFrameEndpoint<E>) -> Self
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        Self {
            ingress_name: ingress.name.clone(),
            ingress_authority: ingress.authority.clone(),
            egress_name: egress.name.clone(),
            egress_authority: egress.authority.clone(),
        }
    }
}

#[cfg(feature = "owned-frame-transport")]
struct OwnedRouteBinding {
    ingress: OwnedFrameEndpoint,
    tx: mpsc::Sender<UOwnedFrame>,
    listener: Arc<OwnedIngressForwarder>,
    registered_filters: Vec<OwnedListenerFilter>,
    dispatch_task: tokio::task::JoinHandle<()>,
    diagnostic: RouteDiagnostic,
}

#[cfg(feature = "owned-frame-transport")]
struct OwnedIngressForwarder {
    tx: mpsc::Sender<UOwnedFrame>,
}

#[cfg(feature = "experimental-copy-minimized-routing")]
struct CopyMinimizedRouteBinding<I, E>
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxLease + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    ingress: ZeroCopyFrameEndpoint<I>,
    _tx: mpsc::Sender<I::Rx>,
    listener: Arc<ZeroCopyIngressForwarder<I::Rx>>,
    registered_filters: Vec<ZeroCopyListenerFilter>,
    dispatch_task: tokio::task::JoinHandle<()>,
    diagnostic: RouteDiagnostic,
    _egress: ZeroCopyFrameEndpoint<E>,
}

#[cfg(feature = "experimental-copy-minimized-routing")]
struct ZeroCopyIngressForwarder<Rx>
where
    Rx: UZeroCopyRxLease + Send + 'static,
{
    tx: mpsc::Sender<Rx>,
}

#[cfg(feature = "owned-frame-transport")]
#[async_trait::async_trait]
impl UOwnedListener for OwnedIngressForwarder {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        if self.tx.send(frame).await.is_err() {
            warn!(
                event = "owned_ingress_queue_closed",
                component = COMPONENT,
                "owned-frame route ingress queue is closed"
            );
        }
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
#[async_trait::async_trait]
impl<Rx> UZeroCopyListener<Rx> for ZeroCopyIngressForwarder<Rx>
where
    Rx: UZeroCopyRxLease + Send + 'static,
{
    async fn on_receive_zero_copy(&self, frame: Rx) {
        if self.tx.send(frame).await.is_err() {
            warn!(
                event = "copy_minimized_ingress_queue_closed",
                component = COMPONENT,
                "copy-minimized route ingress queue is closed"
            );
        }
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
#[async_trait::async_trait]
trait CopyMinimizedRouteOps: Send {
    fn diagnostic(&self) -> RouteDiagnostic;
    async fn unregister_for_delete(&mut self) -> Result<(), UStatus>;
    fn abort_dispatch(&self);
}

pub struct UStreamer {
    name: String,
    #[cfg(any(
        feature = "owned-frame-transport",
        feature = "experimental-copy-minimized-routing"
    ))]
    message_queue_size: usize,
    route_table: RouteTable,
    route_diagnostics: HashMap<RouteKey, RouteDiagnostic>,
    egress_route_pool: EgressRoutePool,
    ingress_route_registry: IngressRouteRegistry,
    subscription_directory: SubscriptionDirectory,
    usubscription: Arc<dyn USubscription>,
    subscription_sync_health: SubscriptionSyncHealth,
    #[cfg(feature = "owned-frame-transport")]
    owned_routes: HashMap<OwnedRouteKey, OwnedRouteBinding>,
    #[cfg(feature = "experimental-copy-minimized-routing")]
    copy_minimized_routes: HashMap<ZeroCopyRouteKey, Box<dyn CopyMinimizedRouteOps>>,
}

impl UStreamer {
    /// Creates a streamer instance with preloaded subscription directory state.
    pub async fn new(
        name: &str,
        message_queue_size: u16,
        usubscription: Arc<dyn USubscription>,
    ) -> Result<Self, UStatus> {
        let name = name.to_string();
        #[cfg(any(
            feature = "owned-frame-transport",
            feature = "experimental-copy-minimized-routing"
        ))]
        let message_queue_size = usize::from(message_queue_size.max(1));
        #[cfg(not(any(
            feature = "owned-frame-transport",
            feature = "experimental-copy-minimized-routing"
        )))]
        let route_queue_size = message_queue_size as usize;
        #[cfg(any(
            feature = "owned-frame-transport",
            feature = "experimental-copy-minimized-routing"
        ))]
        let route_queue_size = message_queue_size;
        debug!(
            event = "ustreamer_create",
            component = COMPONENT,
            streamer_name = name.as_str(),
            "UStreamer created"
        );

        let mut streamer = Self {
            name,
            #[cfg(any(
                feature = "owned-frame-transport",
                feature = "experimental-copy-minimized-routing"
            ))]
            message_queue_size,
            route_table: RouteTable::new(),
            route_diagnostics: HashMap::new(),
            egress_route_pool: EgressRoutePool::new(route_queue_size),
            ingress_route_registry: IngressRouteRegistry::new(),
            subscription_directory: SubscriptionDirectory::empty(),
            usubscription,
            subscription_sync_health: SubscriptionSyncHealth::default(),
            #[cfg(feature = "owned-frame-transport")]
            owned_routes: HashMap::new(),
            #[cfg(feature = "experimental-copy-minimized-routing")]
            copy_minimized_routes: HashMap::new(),
        };

        if let Err(err) = streamer.refresh_subscriptions().await {
            warn!(
                event = "subscription_bootstrap_failed",
                component = COMPONENT,
                streamer_name = streamer.name.as_str(),
                err = %err,
                "startup subscription bootstrap failed; deferred-refresh mode active"
            );
        }

        Ok(streamer)
    }

    fn update_subscription_sync_health(&mut self, succeeded: bool) -> SubscriptionSyncHealth {
        let now = std::time::SystemTime::now();
        self.subscription_sync_health.previous_attempt_succeeded =
            self.subscription_sync_health.last_attempt_succeeded;
        self.subscription_sync_health.last_attempt_at = Some(now);
        self.subscription_sync_health.last_attempt_succeeded = Some(succeeded);
        if succeeded {
            self.subscription_sync_health.last_success_at = Some(now);
        }
        self.subscription_sync_health.clone()
    }

    async fn apply_subscription_snapshot(
        &mut self,
        snapshot: Vec<SubscriptionInfo>,
    ) -> Result<(), UStatus> {
        self.subscription_directory.apply_snapshot(snapshot).await
    }

    pub async fn refresh_subscriptions(&mut self) -> Result<SubscriptionSyncHealth, UStatus> {
        let snapshot = match self
            .usubscription
            .fetch_subscriptions_by_topic(&UUri::any())
            .await
        {
            Ok(snapshot) => snapshot,
            Err(err) => {
                self.update_subscription_sync_health(false);
                return Err(err);
            }
        };

        if let Err(err) = self.apply_subscription_snapshot(snapshot).await {
            self.update_subscription_sync_health(false);
            return Err(err);
        }

        Ok(self.update_subscription_sync_health(true))
    }

    pub fn subscription_sync_health(&self) -> SubscriptionSyncHealth {
        self.subscription_sync_health.clone()
    }

    /// Returns diagnostics for currently installed routes.
    pub fn route_diagnostics(&self) -> Vec<RouteDiagnostic> {
        let mut diagnostics: Vec<RouteDiagnostic> =
            self.route_diagnostics.values().cloned().collect();

        #[cfg(feature = "owned-frame-transport")]
        diagnostics.extend(
            self.owned_routes
                .values()
                .map(|binding| binding.diagnostic.clone()),
        );

        #[cfg(feature = "experimental-copy-minimized-routing")]
        diagnostics.extend(
            self.copy_minimized_routes
                .values()
                .map(|binding| binding.diagnostic()),
        );

        diagnostics.sort_by(|left, right| left.route.sort_key().cmp(&right.route.sort_key()));
        diagnostics
    }

    #[inline(always)]
    fn route_label(r#in: &Endpoint, out: &Endpoint) -> String {
        format!(
            "[in.name: {}, in.authority: {:?} ; out.name: {}, out.authority: {:?}]",
            r#in.name, r#in.authority, out.name, out.authority
        )
    }

    #[cfg(feature = "owned-frame-transport")]
    fn owned_route_label(r#in: &OwnedFrameEndpoint, out: &OwnedFrameEndpoint) -> String {
        Self::native_route_label_for_parts(&r#in.name, &r#in.authority, &out.name, &out.authority)
    }

    #[cfg(any(
        feature = "owned-frame-transport",
        feature = "experimental-copy-minimized-routing"
    ))]
    fn native_route_label_for_parts(
        ingress_name: &str,
        ingress_authority: &str,
        egress_name: &str,
        egress_authority: &str,
    ) -> String {
        format!(
            "[in.name: {}, in.authority: {:?} ; out.name: {}, out.authority: {:?}]",
            ingress_name, ingress_authority, egress_name, egress_authority
        )
    }

    #[inline(always)]
    fn fail_due_to_same_authority(
        &self,
        failure_event: &str,
        route_label: &str,
        r#in: &Endpoint,
        out: &Endpoint,
        action: &str,
    ) -> Result<(), UStatus> {
        let err = Err(UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!(
                "{} are the same. Unable to {}.",
                Self::route_label(r#in, out),
                action,
            ),
        ));
        error!(
            event = failure_event,
            component = COMPONENT,
            streamer_name = self.name.as_str(),
            route_label,
            in_authority = r#in.authority.as_str(),
            out_authority = out.authority.as_str(),
            reason = "same_authority",
            err = ?err,
            "route operation failed"
        );
        err
    }

    /// Adds a unidirectional route between ingress and egress endpoints.
    pub async fn add_route_ref(
        &mut self,
        in_ep: &Endpoint,
        out_ep: &Endpoint,
    ) -> Result<(), UStatus> {
        let route_label = Self::route_label(in_ep, out_ep);
        debug!(
            event = events::ROUTE_ADD_START,
            component = COMPONENT,
            streamer_name = self.name.as_str(),
            route_label,
            in_authority = in_ep.authority.as_str(),
            out_authority = out_ep.authority.as_str(),
            "adding route"
        );

        let lifecycle = RouteLifecycle::new(
            &self.route_table,
            &self.ingress_route_registry,
            &self.subscription_directory,
        );
        let route_key = RouteKey::from_endpoints(in_ep, out_ep);
        let diagnostic = RouteDiagnostic::utransport_compatibility(in_ep, out_ep);

        match lifecycle
            .add_route(&mut self.egress_route_pool, in_ep, out_ep, &route_label)
            .await
        {
            Ok(()) => {
                self.route_diagnostics.insert(route_key, diagnostic);
                debug!(
                    event = events::ROUTE_ADD_OK,
                    component = COMPONENT,
                    streamer_name = self.name.as_str(),
                    route_label,
                    in_authority = in_ep.authority.as_str(),
                    out_authority = out_ep.authority.as_str(),
                    "route add succeeded"
                );
                Ok(())
            }
            Err(AddRouteError::SameAuthority) => self.fail_due_to_same_authority(
                events::ROUTE_ADD_FAILED,
                &route_label,
                in_ep,
                out_ep,
                "add",
            ),
            Err(AddRouteError::AlreadyExists) => {
                error!(
                    event = events::ROUTE_ADD_FAILED,
                    component = COMPONENT,
                    streamer_name = self.name.as_str(),
                    route_label,
                    in_authority = in_ep.authority.as_str(),
                    out_authority = out_ep.authority.as_str(),
                    reason = "already_exists",
                    "route add failed because route already exists"
                );
                Err(UStatus::fail_with_code(
                    UCode::AlreadyExists,
                    "already exists",
                ))
            }
            Err(AddRouteError::FailedToRegisterIngressRoute(err)) => {
                error!(
                    event = events::ROUTE_ADD_FAILED,
                    component = COMPONENT,
                    streamer_name = self.name.as_str(),
                    route_label,
                    in_authority = in_ep.authority.as_str(),
                    out_authority = out_ep.authority.as_str(),
                    reason = "ingress_registration_failed",
                    err = %err,
                    "route add failed during ingress registration"
                );
                Err(UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    err.to_string(),
                ))
            }
        }
    }

    /// Adds a unidirectional route between ingress and egress endpoints.
    pub async fn add_route(&mut self, r#in: Endpoint, out: Endpoint) -> Result<(), UStatus> {
        self.add_route_ref(&r#in, &out).await
    }

    /// Deletes a previously registered unidirectional route.
    pub async fn delete_route_ref(
        &mut self,
        in_ep: &Endpoint,
        out_ep: &Endpoint,
    ) -> Result<(), UStatus> {
        let route_label = Self::route_label(in_ep, out_ep);
        debug!(
            event = events::ROUTE_DELETE_START,
            component = COMPONENT,
            streamer_name = self.name.as_str(),
            route_label,
            in_authority = in_ep.authority.as_str(),
            out_authority = out_ep.authority.as_str(),
            "deleting route"
        );

        let lifecycle = RouteLifecycle::new(
            &self.route_table,
            &self.ingress_route_registry,
            &self.subscription_directory,
        );
        let route_key = RouteKey::from_endpoints(in_ep, out_ep);

        match lifecycle
            .remove_route(&mut self.egress_route_pool, in_ep, out_ep, &route_label)
            .await
        {
            Ok(()) => {
                self.route_diagnostics.remove(&route_key);
                debug!(
                    event = events::ROUTE_DELETE_OK,
                    component = COMPONENT,
                    streamer_name = self.name.as_str(),
                    route_label,
                    in_authority = in_ep.authority.as_str(),
                    out_authority = out_ep.authority.as_str(),
                    "route delete succeeded"
                );
                Ok(())
            }
            Err(RemoveRouteError::SameAuthority) => self.fail_due_to_same_authority(
                events::ROUTE_DELETE_FAILED,
                &route_label,
                in_ep,
                out_ep,
                "delete",
            ),
            Err(RemoveRouteError::NotFound) => {
                error!(
                    event = events::ROUTE_DELETE_FAILED,
                    component = COMPONENT,
                    streamer_name = self.name.as_str(),
                    route_label,
                    in_authority = in_ep.authority.as_str(),
                    out_authority = out_ep.authority.as_str(),
                    reason = "not_found",
                    "route delete failed because route was not found"
                );
                Err(UStatus::fail_with_code(UCode::NotFound, "not found"))
            }
        }
    }

    /// Deletes a previously registered unidirectional route.
    pub async fn delete_route(&mut self, r#in: Endpoint, out: Endpoint) -> Result<(), UStatus> {
        self.delete_route_ref(&r#in, &out).await
    }

    #[cfg(feature = "owned-frame-transport")]
    async fn owned_route_filters(
        &self,
        in_authority: &str,
        out_authority: &str,
    ) -> Vec<OwnedListenerFilter> {
        let mut filters = vec![(
            authority_to_wildcard_filter(in_authority),
            Some(authority_to_wildcard_filter(out_authority)),
        )];
        let (_, subscribers) = self
            .subscription_directory
            .lookup_route_subscribers_with_version(out_authority)
            .await;
        filters.extend(
            PublishRouteResolver::derive_source_filters(in_authority, out_authority, &subscribers)
                .into_values()
                .map(|source| (source, None)),
        );
        filters
    }

    #[cfg(feature = "experimental-copy-minimized-routing")]
    async fn copy_minimized_route_filters(
        &self,
        in_authority: &str,
        out_authority: &str,
    ) -> Vec<ZeroCopyListenerFilter> {
        let mut filters = vec![(
            authority_to_wildcard_filter(in_authority),
            Some(authority_to_wildcard_filter(out_authority)),
        )];
        let (_, subscribers) = self
            .subscription_directory
            .lookup_route_subscribers_with_version(out_authority)
            .await;
        filters.extend(
            PublishRouteResolver::derive_source_filters(in_authority, out_authority, &subscribers)
                .into_values()
                .map(|source| (source, None)),
        );
        filters
    }

    #[cfg(feature = "owned-frame-transport")]
    async fn rollback_owned_registrations(
        ingress: &OwnedFrameEndpoint,
        listener: Arc<OwnedIngressForwarder>,
        registered_filters: &[OwnedListenerFilter],
    ) {
        for (source_filter, sink_filter) in registered_filters {
            if let Err(error) = ingress
                .transport
                .unregister_owned_listener(source_filter, sink_filter.as_ref(), listener.clone())
                .await
            {
                warn!(
                    event = "owned_route_listener_rollback_failed",
                    component = COMPONENT,
                    ingress = ingress.name.as_str(),
                    ingress_authority = ingress.authority.as_str(),
                    err = %error,
                    "failed to roll back owned route listener registration"
                );
            }
        }
    }

    #[cfg(feature = "experimental-copy-minimized-routing")]
    async fn rollback_zero_copy_registrations<I>(
        ingress: &ZeroCopyFrameEndpoint<I>,
        listener: Arc<ZeroCopyIngressForwarder<I::Rx>>,
        registered_filters: &[ZeroCopyListenerFilter],
    ) where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
    {
        for (source_filter, sink_filter) in registered_filters {
            if let Err(error) = ingress
                .transport
                .unregister_zero_copy_listener(
                    source_filter,
                    sink_filter.as_ref(),
                    listener.clone(),
                )
                .await
            {
                warn!(
                    event = "copy_minimized_listener_rollback_failed",
                    component = COMPONENT,
                    ingress = ingress.name.as_str(),
                    ingress_authority = ingress.authority.as_str(),
                    err = %error,
                    "failed to roll back copy-minimized listener registration"
                );
            }
        }
    }

    #[cfg(feature = "owned-frame-transport")]
    async fn owned_dispatch_loop(
        route_label: String,
        egress: OwnedFrameEndpoint,
        mut rx: mpsc::Receiver<UOwnedFrame>,
    ) {
        while let Some(frame) = rx.recv().await {
            let frame = match try_project_frame_to_umessage(
                frame.metadata().clone(),
                frame.payload().cloned(),
            )
            .and_then(|message| {
                let metadata = try_project_umessage_to_frame_metadata(&message)?;
                UOwnedFrame::new(
                    metadata,
                    message.payload().map(bytes::Bytes::copy_from_slice),
                )
            }) {
                Ok(frame) => frame,
                Err(error) => {
                    warn!(
                        event = "owned_route_frame_projection_failed",
                        component = COMPONENT,
                        route_label = route_label.as_str(),
                        err = %error,
                        "dropping owned frame that cannot round-trip through UMessage compatibility"
                    );
                    continue;
                }
            };

            if let Err(error) = egress.transport.send_owned(frame).await {
                warn!(
                    event = "owned_route_egress_send_failed",
                    component = COMPONENT,
                    route_label = route_label.as_str(),
                    egress = egress.name.as_str(),
                    egress_authority = egress.authority.as_str(),
                    err = %error,
                    "owned route egress send failed"
                );
            }
        }
    }

    #[cfg(feature = "experimental-copy-minimized-routing")]
    async fn copy_minimized_dispatch_loop<E, Rx>(
        route_label: String,
        egress_name: String,
        egress_authority: String,
        egress_transport: Arc<E>,
        options: CopyMinimizedRouteOptions,
        mut rx: mpsc::Receiver<Rx>,
    ) where
        E: UZeroCopyTransport + Send + Sync + 'static,
        Rx: UZeroCopyRxLease + Send + 'static,
    {
        while let Some(frame) = rx.recv().await {
            let spec = match loan_spec_for_copy_minimized(&frame, options) {
                Ok(spec) => spec,
                Err(error) => {
                    warn!(
                        event = "copy_minimized_loan_spec_failed",
                        component = COMPONENT,
                        route_label = route_label.as_str(),
                        egress = egress_name.as_str(),
                        egress_authority = egress_authority.as_str(),
                        err = %error,
                        "copy-minimized route rejected ingress frame"
                    );
                    continue;
                }
            };

            let payload_len = spec.payload_len();
            let payload_alignment = spec.payload_alignment();
            let send_result = match egress_transport.loan_tx(spec).await {
                Ok(mut tx) => match copy_frame_payload_to_tx(&frame, &mut tx) {
                    Ok(copy_diagnostics) => egress_transport
                        .send_zero_copy(tx)
                        .await
                        .map(|()| copy_diagnostics),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };

            match send_result {
                Ok((copied_payload_len, payload_slice_count)) => debug!(
                    event = "copy_minimized_egress_send_ok",
                    component = COMPONENT,
                    route_label = route_label.as_str(),
                    egress = egress_name.as_str(),
                    egress_authority = egress_authority.as_str(),
                    payload_len,
                    payload_alignment,
                    copied_payload_len,
                    payload_slice_count,
                    "copy-minimized route sent frame"
                ),
                Err(error) => warn!(
                    event = "copy_minimized_egress_send_failed",
                    component = COMPONENT,
                    route_label = route_label.as_str(),
                    egress = egress_name.as_str(),
                    egress_authority = egress_authority.as_str(),
                    payload_len,
                    payload_alignment,
                    err = %error,
                    "copy-minimized route egress send failed"
                ),
            }
        }
    }

    /// Adds a feature-gated owned/copying route between owned-frame endpoints.
    #[cfg(feature = "owned-frame-transport")]
    pub async fn add_owned_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = OwnedRouteKey::new(ingress, egress);
        if self.owned_routes.contains_key(&route_key) {
            return Err(UStatus::fail_with_code(
                UCode::AlreadyExists,
                "owned route already exists",
            ));
        }

        let route_label = Self::owned_route_label(ingress, egress);
        let filters = self
            .owned_route_filters(&ingress.authority, &egress.authority)
            .await;
        let (tx, rx) = mpsc::channel::<UOwnedFrame>(self.message_queue_size);
        let listener = Arc::new(OwnedIngressForwarder { tx: tx.clone() });
        let mut registered_filters = Vec::with_capacity(filters.len());
        let diagnostic = RouteDiagnostic::owned_frame_compatibility(ingress, egress);

        for (source_filter, sink_filter) in filters {
            if let Err(error) = ingress
                .transport
                .register_owned_listener(&source_filter, sink_filter.as_ref(), listener.clone())
                .await
            {
                Self::rollback_owned_registrations(ingress, listener.clone(), &registered_filters)
                    .await;
                return Err(error);
            }
            registered_filters.push((source_filter, sink_filter));
        }

        let dispatch_task =
            tokio::spawn(Self::owned_dispatch_loop(route_label, egress.clone(), rx));
        self.owned_routes.insert(
            route_key,
            OwnedRouteBinding {
                ingress: ingress.clone(),
                tx,
                listener,
                registered_filters,
                dispatch_task,
                diagnostic,
            },
        );

        Ok(())
    }

    /// Adds a feature-gated owned/copying route between owned-frame endpoints.
    #[cfg(feature = "owned-frame-transport")]
    pub async fn add_owned_route(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.add_owned_route_ref(&ingress, &egress).await
    }

    /// Deletes a feature-gated owned/copying route between owned-frame endpoints.
    #[cfg(feature = "owned-frame-transport")]
    pub async fn delete_owned_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = OwnedRouteKey::new(ingress, egress);
        let binding = self
            .owned_routes
            .remove(&route_key)
            .ok_or_else(|| UStatus::fail_with_code(UCode::NotFound, "owned route not found"))?;

        for (source_filter, sink_filter) in &binding.registered_filters {
            if let Err(error) = binding
                .ingress
                .transport
                .unregister_owned_listener(
                    source_filter,
                    sink_filter.as_ref(),
                    binding.listener.clone(),
                )
                .await
            {
                warn!(
                    event = "owned_route_listener_unregister_failed",
                    component = COMPONENT,
                    ingress = binding.ingress.name.as_str(),
                    ingress_authority = binding.ingress.authority.as_str(),
                    err = %error,
                    "owned route listener unregister failed"
                );
            }
        }
        drop(binding.tx);
        binding.dispatch_task.abort();
        Ok(())
    }

    /// Deletes a feature-gated owned/copying route between owned-frame endpoints.
    #[cfg(feature = "owned-frame-transport")]
    pub async fn delete_owned_route(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.delete_owned_route_ref(&ingress, &egress).await
    }

    /// Adds a feature-gated copy-minimized route between zero-copy endpoints.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_copy_minimized_route_ref<I, E>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.add_copy_minimized_route_ref_with_options(
            ingress,
            egress,
            CopyMinimizedRouteOptions::default(),
        )
        .await
    }

    /// Adds a feature-gated copy-minimized route with explicit options.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_copy_minimized_route_ref_with_options<I, E>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = ZeroCopyRouteKey::new(ingress, egress);
        if self.copy_minimized_routes.contains_key(&route_key) {
            return Err(UStatus::fail_with_code(
                UCode::AlreadyExists,
                "copy-minimized route already exists",
            ));
        }

        let filters = self
            .copy_minimized_route_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = CopyMinimizedRouteBinding::new(
            ingress,
            egress,
            filters,
            self.message_queue_size,
            options,
        )
        .await?;
        self.copy_minimized_routes
            .insert(route_key, Box::new(binding));
        Ok(())
    }

    /// Adds a feature-gated copy-minimized route, consuming endpoint values after registration.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_copy_minimized_route<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.add_copy_minimized_route_ref(&ingress, &egress).await
    }

    /// Adds a feature-gated copy-minimized route with explicit options, consuming endpoint values.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_copy_minimized_route_with_options<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.add_copy_minimized_route_ref_with_options(&ingress, &egress, options)
            .await
    }

    /// Adds a selected-wire copy-minimized route between endpoints with the same static wire `W`.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_selected_wire_copy_minimized_route_ref<I, E, W, C>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<UWireTransport<I, W, C>>,
        egress: &ZeroCopyFrameEndpoint<UWireTransport<E, W, C>>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransportCore + Send + Sync + 'static,
        I::Rx: Send + 'static,
        E: UZeroCopyTransportCore + Send + Sync + 'static,
        W: UWire + Send + Sync + 'static,
        C: UWireMetadataCodec + Clone + Send + Sync + 'static,
    {
        self.add_selected_wire_copy_minimized_route_ref_with_options(
            ingress,
            egress,
            CopyMinimizedRouteOptions::default(),
        )
        .await
    }

    /// Adds a selected-wire copy-minimized route with explicit options.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_selected_wire_copy_minimized_route_ref_with_options<I, E, W, C>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<UWireTransport<I, W, C>>,
        egress: &ZeroCopyFrameEndpoint<UWireTransport<E, W, C>>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransportCore + Send + Sync + 'static,
        I::Rx: Send + 'static,
        E: UZeroCopyTransportCore + Send + Sync + 'static,
        W: UWire + Send + Sync + 'static,
        C: UWireMetadataCodec + Clone + Send + Sync + 'static,
    {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = ZeroCopyRouteKey::new(ingress, egress);
        if self.copy_minimized_routes.contains_key(&route_key) {
            return Err(UStatus::fail_with_code(
                UCode::AlreadyExists,
                "copy-minimized route already exists",
            ));
        }

        let filters = self
            .copy_minimized_route_filters(&ingress.authority, &egress.authority)
            .await
            .into_iter()
            // UWireTransport re-filters decoded metadata; publish frames do not carry a sink URI.
            .map(|(source_filter, _sink_filter)| (source_filter, None))
            .collect::<Vec<_>>();
        let binding = CopyMinimizedRouteBinding::new(
            ingress,
            egress,
            filters,
            self.message_queue_size,
            options,
        )
        .await?;
        self.copy_minimized_routes
            .insert(route_key, Box::new(binding));
        Ok(())
    }

    /// Adds a selected-wire copy-minimized route, consuming endpoint values after registration.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_selected_wire_copy_minimized_route<I, E, W, C>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<UWireTransport<I, W, C>>,
        egress: ZeroCopyFrameEndpoint<UWireTransport<E, W, C>>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransportCore + Send + Sync + 'static,
        I::Rx: Send + 'static,
        E: UZeroCopyTransportCore + Send + Sync + 'static,
        W: UWire + Send + Sync + 'static,
        C: UWireMetadataCodec + Clone + Send + Sync + 'static,
    {
        self.add_selected_wire_copy_minimized_route_ref(&ingress, &egress)
            .await
    }

    /// Adds a selected-wire copy-minimized route with explicit options, consuming endpoint values.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn add_selected_wire_copy_minimized_route_with_options<I, E, W, C>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<UWireTransport<I, W, C>>,
        egress: ZeroCopyFrameEndpoint<UWireTransport<E, W, C>>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransportCore + Send + Sync + 'static,
        I::Rx: Send + 'static,
        E: UZeroCopyTransportCore + Send + Sync + 'static,
        W: UWire + Send + Sync + 'static,
        C: UWireMetadataCodec + Clone + Send + Sync + 'static,
    {
        self.add_selected_wire_copy_minimized_route_ref_with_options(&ingress, &egress, options)
            .await
    }

    /// Deletes a feature-gated copy-minimized route between zero-copy endpoints.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn delete_copy_minimized_route_ref<I, E>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        if ingress.authority == egress.authority {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            ));
        }

        let route_key = ZeroCopyRouteKey::new(ingress, egress);
        let mut binding = self
            .copy_minimized_routes
            .remove(&route_key)
            .ok_or_else(|| {
                UStatus::fail_with_code(UCode::NotFound, "copy-minimized route not found")
            })?;

        if let Err(error) = binding.unregister_for_delete().await {
            self.copy_minimized_routes.insert(route_key, binding);
            return Err(error);
        }
        binding.abort_dispatch();
        Ok(())
    }

    /// Deletes a feature-gated copy-minimized route between zero-copy endpoints.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn delete_copy_minimized_route<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransport + Send + Sync + 'static,
        I::Rx: UZeroCopyRxLease + Send + 'static,
        E: UZeroCopyTransport + Send + Sync + 'static,
    {
        self.delete_copy_minimized_route_ref(&ingress, &egress)
            .await
    }

    /// Deletes a selected-wire copy-minimized route between endpoints with the same static wire `W`.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn delete_selected_wire_copy_minimized_route_ref<I, E, W, C>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<UWireTransport<I, W, C>>,
        egress: &ZeroCopyFrameEndpoint<UWireTransport<E, W, C>>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransportCore + Send + Sync + 'static,
        I::Rx: Send + 'static,
        E: UZeroCopyTransportCore + Send + Sync + 'static,
        W: UWire + Send + Sync + 'static,
        C: UWireMetadataCodec + Clone + Send + Sync + 'static,
    {
        self.delete_copy_minimized_route_ref(ingress, egress).await
    }

    /// Deletes a selected-wire copy-minimized route, consuming endpoint values after deletion.
    #[cfg(feature = "experimental-copy-minimized-routing")]
    pub async fn delete_selected_wire_copy_minimized_route<I, E, W, C>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<UWireTransport<I, W, C>>,
        egress: ZeroCopyFrameEndpoint<UWireTransport<E, W, C>>,
    ) -> Result<(), UStatus>
    where
        I: UZeroCopyTransportCore + Send + Sync + 'static,
        I::Rx: Send + 'static,
        E: UZeroCopyTransportCore + Send + Sync + 'static,
        W: UWire + Send + Sync + 'static,
        C: UWireMetadataCodec + Clone + Send + Sync + 'static,
    {
        self.delete_selected_wire_copy_minimized_route_ref(&ingress, &egress)
            .await
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl<I, E> CopyMinimizedRouteBinding<I, E>
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxLease + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    async fn new(
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
        filters: Vec<ZeroCopyListenerFilter>,
        message_queue_size: usize,
        options: CopyMinimizedRouteOptions,
    ) -> Result<Self, UStatus> {
        let route_label = UStreamer::native_route_label_for_parts(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        );
        let (tx, rx) = mpsc::channel::<I::Rx>(message_queue_size);
        let listener = Arc::new(ZeroCopyIngressForwarder { tx: tx.clone() });
        let mut registered_filters = Vec::with_capacity(filters.len());
        let diagnostic = RouteDiagnostic::copy_minimized(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        );

        for (source_filter, sink_filter) in filters {
            if let Err(error) = ingress
                .transport
                .register_zero_copy_listener(&source_filter, sink_filter.as_ref(), listener.clone())
                .await
            {
                UStreamer::rollback_zero_copy_registrations(
                    ingress,
                    listener.clone(),
                    &registered_filters,
                )
                .await;
                return Err(error);
            }
            registered_filters.push((source_filter, sink_filter));
        }

        let dispatch_task = tokio::spawn(UStreamer::copy_minimized_dispatch_loop(
            route_label,
            egress.name.clone(),
            egress.authority.clone(),
            egress.transport.clone(),
            options,
            rx,
        ));

        Ok(Self {
            ingress: (*ingress).clone(),
            _tx: tx,
            listener,
            registered_filters,
            dispatch_task,
            diagnostic,
            _egress: (*egress).clone(),
        })
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
#[async_trait::async_trait]
impl<I, E> CopyMinimizedRouteOps for CopyMinimizedRouteBinding<I, E>
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxLease + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    fn diagnostic(&self) -> RouteDiagnostic {
        self.diagnostic.clone()
    }

    async fn unregister_for_delete(&mut self) -> Result<(), UStatus> {
        let mut remaining = Vec::new();
        let mut first_error = None;
        for (source_filter, sink_filter) in self.registered_filters.drain(..) {
            if let Err(error) = self
                .ingress
                .transport
                .unregister_zero_copy_listener(
                    &source_filter,
                    sink_filter.as_ref(),
                    self.listener.clone(),
                )
                .await
            {
                warn!(
                    event = "copy_minimized_listener_unregister_failed",
                    component = COMPONENT,
                    ingress = self.ingress.name.as_str(),
                    ingress_authority = self.ingress.authority.as_str(),
                    err = %error,
                    "copy-minimized route listener unregister failed"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
                remaining.push((source_filter, sink_filter));
            }
        }

        if let Some(error) = first_error {
            self.registered_filters = remaining;
            return Err(error);
        }
        Ok(())
    }

    fn abort_dispatch(&self) {
        self.dispatch_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::UStreamer;
    use crate::SubscriptionSyncHealth;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use up_rust::communication::SubscriptionStatus;
    use up_rust::core::usubscription::{ResetReason, SubscriptionInfo, USubscription};
    use up_rust::{UCode, UStatus, UUri};

    struct SequencedUSubscription {
        responses: Mutex<VecDeque<Result<Vec<SubscriptionInfo>, UStatus>>>,
    }

    impl SequencedUSubscription {
        fn new(responses: Vec<Result<Vec<SubscriptionInfo>, UStatus>>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
            }
        }

        fn unsupported(operation: &str) -> UStatus {
            UStatus::fail_with_code(
                UCode::Unimplemented,
                format!("{operation} is not used in this test stub"),
            )
        }
    }

    #[async_trait]
    impl USubscription for SequencedUSubscription {
        async fn subscribe(
            &self,
            _topic: &UUri,
            _expiration: Option<u64>,
            _min_sample_period: Option<u32>,
        ) -> Result<SubscriptionStatus, UStatus> {
            Err(Self::unsupported("subscribe"))
        }

        async fn unsubscribe(&self, _topic: &UUri) -> Result<(), UStatus> {
            Err(Self::unsupported("unsubscribe"))
        }

        async fn fetch_subscriptions_by_topic(
            &self,
            _topic: &UUri,
        ) -> Result<Vec<SubscriptionInfo>, UStatus> {
            self.responses
                .lock()
                .expect("provider queue lock should succeed")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(UStatus::fail_with_code(
                        UCode::Unavailable,
                        "no queued snapshot response",
                    ))
                })
        }

        async fn fetch_subscriptions_by_subscriber(
            &self,
            _subscriber: &UUri,
        ) -> Result<Vec<SubscriptionInfo>, UStatus> {
            Err(Self::unsupported("fetch_subscriptions_by_subscriber"))
        }

        async fn register_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
            Ok(())
        }

        async fn unregister_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
            Ok(())
        }

        async fn fetch_subscribers(&self, _topic: &UUri) -> Result<Vec<UUri>, UStatus> {
            Err(Self::unsupported("fetch_subscribers"))
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

    fn subscription(topic: &str, subscriber: &str) -> SubscriptionInfo {
        SubscriptionInfo::new(
            topic.parse::<UUri>().expect("valid topic URI"),
            subscriber.parse::<UUri>().expect("valid subscriber URI"),
            SubscriptionStatus::Subscribed,
            None,
            None,
        )
    }

    fn valid_snapshot() -> Vec<SubscriptionInfo> {
        vec![subscription(
            "//authority-a/5BA0/1/8001",
            "//authority-b/5678/1/1234",
        )]
    }

    #[test]
    fn subscription_sync_health_default_is_empty() {
        let health = SubscriptionSyncHealth::default();
        assert_eq!(health.last_attempt_at, None);
        assert_eq!(health.last_success_at, None);
        assert_eq!(health.last_attempt_succeeded, None);
        assert_eq!(health.previous_attempt_succeeded, None);
    }

    #[tokio::test]
    async fn startup_fetch_failure_is_non_fatal_and_sets_first_failed_attempt() {
        let usubscription: Arc<dyn USubscription> =
            Arc::new(SequencedUSubscription::new(vec![Err(
                UStatus::fail_with_code(UCode::Unavailable, "simulated bootstrap failure"),
            )]));

        let streamer = UStreamer::new("startup-failure", 16, usubscription)
            .await
            .expect("startup should not fail when snapshot fetch fails");

        let health = streamer.subscription_sync_health();
        assert!(health.last_attempt_at.is_some());
        assert_eq!(health.last_success_at, None);
        assert_eq!(health.last_attempt_succeeded, Some(false));
        assert_eq!(health.previous_attempt_succeeded, None);
    }

    #[tokio::test]
    async fn refresh_success_returns_health_and_rolls_previous_attempt_state() {
        let usubscription: Arc<dyn USubscription> = Arc::new(SequencedUSubscription::new(vec![
            Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "first bootstrap failure",
            )),
            Ok(valid_snapshot()),
        ]));

        let mut streamer = UStreamer::new("refresh-success", 16, usubscription)
            .await
            .expect("startup should be non-fatal");

        let returned_health = streamer
            .refresh_subscriptions()
            .await
            .expect("refresh should succeed");
        let accessor_health = streamer.subscription_sync_health();

        assert_eq!(returned_health, accessor_health);
        assert!(returned_health.last_attempt_at.is_some());
        assert!(returned_health.last_success_at.is_some());
        assert_eq!(returned_health.last_attempt_succeeded, Some(true));
        assert_eq!(returned_health.previous_attempt_succeeded, Some(false));
    }

    #[tokio::test]
    async fn failed_refresh_updates_health_visible_via_accessor() {
        let usubscription: Arc<dyn USubscription> = Arc::new(SequencedUSubscription::new(vec![
            Ok(valid_snapshot()),
            Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "refresh failure",
            )),
        ]));

        let mut streamer = UStreamer::new("refresh-failure", 16, usubscription)
            .await
            .expect("startup should succeed");

        assert!(streamer.refresh_subscriptions().await.is_err());

        let health = streamer.subscription_sync_health();
        assert!(health.last_attempt_at.is_some());
        assert!(health.last_success_at.is_some());
        assert_eq!(health.last_attempt_succeeded, Some(false));
        assert_eq!(health.previous_attempt_succeeded, Some(true));
    }

    #[tokio::test]
    async fn repeated_failed_refresh_tracks_previous_failed_attempt_after_prior_success() {
        let usubscription: Arc<dyn USubscription> = Arc::new(SequencedUSubscription::new(vec![
            Ok(valid_snapshot()),
            Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "first refresh failure",
            )),
            Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "second refresh failure",
            )),
        ]));

        let mut streamer = UStreamer::new("refresh-repeated-failure", 16, usubscription)
            .await
            .expect("startup should succeed");

        assert!(streamer.refresh_subscriptions().await.is_err());
        assert!(streamer.refresh_subscriptions().await.is_err());

        let health = streamer.subscription_sync_health();
        assert_eq!(health.last_attempt_succeeded, Some(false));
        assert_eq!(health.previous_attempt_succeeded, Some(false));
        assert!(health.last_success_at.is_some());
    }
}
