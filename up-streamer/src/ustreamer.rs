// Copyright (c) 2024, 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Route lifecycle facade over classic routing and setup-owned frame workers.

use crate::{
    control_plane::{
        route_lifecycle::{AddRouteError, RemoveRouteError, RouteLifecycle},
        route_table::{RouteKey, RouteTable},
    },
    data_plane::{egress_pool::EgressRoutePool, ingress_registry::IngressRouteRegistry},
    routing::subscription_directory::SubscriptionDirectory,
    Endpoint, RouteDiagnostic, SubscriptionSyncHealth,
};
#[cfg(feature = "experimental-copy-minimized-routing")]
use crate::{
    frame_routes::native, CopyMinimizedRouteOptions, PreparedEndpoint, ZeroCopyFrameEndpoint,
};
#[cfg(feature = "owned-frame-transport")]
use crate::{frame_routes::owned, OwnedFrameEndpoint};
#[cfg(any(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
use crate::{
    frame_routes::{self, FrameRouteControl, FrameRouteKey},
    routing::{
        authority_filter::authority_to_wildcard_filter, publish_resolution::PublishRouteResolver,
    },
};
use std::{collections::HashMap, sync::Arc};
use up_rust::{
    core::usubscription::{SubscriptionInfo, USubscription},
    UCode, UStatus, UUri,
};

/// Owns installed forwarding routes, native registrations and subscription state.
///
/// Route creation reserves generic workers and uses configured native egress
/// catalogs. Native traffic never creates routes or boxed per-message futures.
/// Failed explicit detach retains the route for retry; successful deletion drains
/// admitted work. Retained native frames remain independent of route ownership.
pub struct UStreamer {
    name: String,
    route_table: RouteTable,
    route_diagnostics: HashMap<RouteKey, RouteDiagnostic>,
    egress_route_pool: EgressRoutePool,
    ingress_route_registry: IngressRouteRegistry,
    subscription_directory: SubscriptionDirectory,
    usubscription: Arc<dyn USubscription>,
    subscription_sync_health: SubscriptionSyncHealth,
    #[cfg(any(
        feature = "owned-frame-transport",
        feature = "experimental-copy-minimized-routing"
    ))]
    message_queue_size: usize,
    #[cfg(any(
        feature = "owned-frame-transport",
        feature = "experimental-copy-minimized-routing"
    ))]
    frame_routes: HashMap<FrameRouteKey, Box<dyn FrameRouteControl>>,
}

impl UStreamer {
    /// Creates a Streamer and attempts a nonfatal initial subscription refresh.
    pub async fn new(
        name: &str,
        message_queue_size: u16,
        usubscription: Arc<dyn USubscription>,
    ) -> Result<Self, UStatus> {
        let mut streamer = Self {
            name: name.into(),
            route_table: RouteTable::new(),
            route_diagnostics: HashMap::new(),
            egress_route_pool: EgressRoutePool::new(usize::from(message_queue_size.max(1))),
            ingress_route_registry: IngressRouteRegistry::new(),
            subscription_directory: SubscriptionDirectory::empty(),
            usubscription,
            subscription_sync_health: SubscriptionSyncHealth::default(),
            #[cfg(any(
                feature = "owned-frame-transport",
                feature = "experimental-copy-minimized-routing"
            ))]
            message_queue_size: usize::from(message_queue_size.max(1)),
            #[cfg(any(
                feature = "owned-frame-transport",
                feature = "experimental-copy-minimized-routing"
            ))]
            frame_routes: HashMap::new(),
        };
        if let Err(error) = streamer.refresh_subscriptions().await {
            tracing::warn!(event = "subscription_bootstrap_failed", streamer_name = name, %error, "startup subscription refresh deferred");
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
    /// Fetches and applies a complete subscription snapshot, recording actual health.
    pub async fn refresh_subscriptions(&mut self) -> Result<SubscriptionSyncHealth, UStatus> {
        let snapshot = match self
            .usubscription
            .fetch_subscriptions_by_topic(&UUri::any())
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.update_subscription_sync_health(false);
                return Err(error);
            }
        };
        if let Err(error) = self.apply_subscription_snapshot(snapshot).await {
            self.update_subscription_sync_health(false);
            return Err(error);
        }
        Ok(self.update_subscription_sync_health(true))
    }
    /// Returns completed refresh health without triggering a network request.
    pub fn subscription_sync_health(&self) -> SubscriptionSyncHealth {
        self.subscription_sync_health.clone()
    }
    /// Returns sorted identities and copy semantics of currently installed routes.
    pub fn route_diagnostics(&self) -> Vec<RouteDiagnostic> {
        let mut values: Vec<_> = self.route_diagnostics.values().cloned().collect();
        #[cfg(any(
            feature = "owned-frame-transport",
            feature = "experimental-copy-minimized-routing"
        ))]
        values.extend(
            self.frame_routes
                .values()
                .map(|binding| binding.diagnostic().clone()),
        );
        values.sort_by(|a, b| a.route.sort_key().cmp(&b.route.sort_key()));
        values
    }
    fn route_label(ingress: &Endpoint, egress: &Endpoint) -> String {
        format!(
            "[in.name: {}, in.authority: {:?} ; out.name: {}, out.authority: {:?}]",
            ingress.name, ingress.authority, egress.name, egress.authority
        )
    }
    /// Adds a classic compatibility route, rejecting implicit SingleType classic hops.
    pub async fn add_route_ref(
        &mut self,
        ingress: &Endpoint,
        egress: &Endpoint,
    ) -> Result<(), UStatus> {
        self.add_route_ref_with_native_policy(ingress, egress, None)
            .await
    }
    /// Adds a classic route after validating its complete native-route declaration.
    pub async fn add_route_ref_with_native_policy(
        &mut self,
        ingress: &Endpoint,
        egress: &Endpoint,
        policy: Option<&crate::NativeRoutePolicy>,
    ) -> Result<(), UStatus> {
        crate::native_route::admit_classic_boundary(
            (
                &ingress.name,
                crate::NativeRouteApi::Classic,
                ingress.native_profile.as_ref(),
            ),
            (
                &egress.name,
                crate::NativeRouteApi::Classic,
                egress.native_profile.as_ref(),
            ),
            policy,
        )?;
        let label = Self::route_label(ingress, egress);
        let lifecycle = RouteLifecycle::new(
            &self.route_table,
            &self.ingress_route_registry,
            &self.subscription_directory,
        );
        match lifecycle
            .add_route(&mut self.egress_route_pool, ingress, egress, &label)
            .await
        {
            Ok(()) => {
                self.route_diagnostics.insert(
                    RouteKey::from_endpoints(ingress, egress),
                    RouteDiagnostic::utransport_compatibility(ingress, egress),
                );
                if let Some(policy) = policy {
                    policy.record_activation();
                }
                tracing::debug!(event = "route_add_ok", streamer_name = %self.name, route_label = %label, "classic route added");
                Ok(())
            }
            Err(AddRouteError::SameAuthority) => Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            )),
            Err(AddRouteError::AlreadyExists) => Err(UStatus::fail_with_code(
                UCode::AlreadyExists,
                "already exists",
            )),
            Err(AddRouteError::FailedToRegisterIngressRoute(error)) => Err(
                UStatus::fail_with_code(UCode::InvalidArgument, error.to_string()),
            ),
        }
    }
    /// Adds a classic route, consuming endpoint values after registration.
    pub async fn add_route(&mut self, ingress: Endpoint, egress: Endpoint) -> Result<(), UStatus> {
        self.add_route_ref(&ingress, &egress).await
    }
    /// Deletes a previously installed classic compatibility route.
    pub async fn delete_route_ref(
        &mut self,
        ingress: &Endpoint,
        egress: &Endpoint,
    ) -> Result<(), UStatus> {
        let label = Self::route_label(ingress, egress);
        let lifecycle = RouteLifecycle::new(
            &self.route_table,
            &self.ingress_route_registry,
            &self.subscription_directory,
        );
        match lifecycle
            .remove_route(&mut self.egress_route_pool, ingress, egress, &label)
            .await
        {
            Ok(()) => {
                self.route_diagnostics
                    .remove(&RouteKey::from_endpoints(ingress, egress));
                Ok(())
            }
            Err(RemoveRouteError::SameAuthority) => Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "ingress and egress authorities must differ",
            )),
            Err(RemoveRouteError::NotFound) => {
                Err(UStatus::fail_with_code(UCode::NotFound, "not found"))
            }
        }
    }
    /// Deletes a classic route, consuming its endpoint values.
    pub async fn delete_route(
        &mut self,
        ingress: Endpoint,
        egress: Endpoint,
    ) -> Result<(), UStatus> {
        self.delete_route_ref(&ingress, &egress).await
    }
}

#[cfg(any(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
impl UStreamer {
    fn worker_config(&self) -> up_rust::RegistrationConfig {
        up_rust::RegistrationConfig {
            capacity: self.message_queue_size,
            overflow: up_rust::OverflowPolicy::Block {
                deadline: std::time::Duration::from_secs(5),
            },
        }
    }
    async fn frame_filters(&self, ingress: &str, egress: &str) -> Vec<(UUri, Option<UUri>)> {
        let mut filters = vec![(
            authority_to_wildcard_filter(ingress),
            Some(authority_to_wildcard_filter(egress)),
        )];
        let (_, subscribers) = self
            .subscription_directory
            .lookup_route_subscribers_with_version(egress)
            .await;
        filters.extend(
            PublishRouteResolver::derive_source_filters(ingress, egress, &subscribers)
                .into_values()
                .map(|source| (source, None)),
        );
        filters
    }
    fn vacant(&self, key: &FrameRouteKey) -> Result<(), UStatus> {
        if self.frame_routes.contains_key(key) {
            Err(UStatus::fail_with_code(
                UCode::AlreadyExists,
                "frame route already exists",
            ))
        } else {
            Ok(())
        }
    }
    async fn remove_frame_route(&mut self, key: &FrameRouteKey) -> Result<(), UStatus> {
        self.frame_routes
            .get_mut(key)
            .ok_or_else(|| UStatus::fail_with_code(UCode::NotFound, "frame route not found"))?
            .stop()
            .await?;
        self.frame_routes.remove(key);
        Ok(())
    }
}

#[cfg(feature = "owned-frame-transport")]
impl UStreamer {
    /// Adds an owned/copying route with matching declared native profiles.
    pub async fn add_owned_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "owned",
        )?;
        self.vacant(&key)?;
        crate::endpoint::native_route_agreement(
            ingress.native_profile.as_ref(),
            egress.native_profile.as_ref(),
        )?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = owned::attach_owned(
            ingress,
            filters,
            self.worker_config(),
            RouteDiagnostic::owned_frame_compatibility(ingress, egress),
            owned::OwnedDestination {
                endpoint: egress.clone(),
                label: format!(
                    "owned:{}@{} -> owned:{}@{}",
                    ingress.name, ingress.authority, egress.name, egress.authority
                ),
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        Ok(())
    }
    /// Adds an owned/copying route, consuming configured endpoint values.
    pub async fn add_owned_route(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.add_owned_route_ref(&ingress, &egress).await
    }
    /// Closes native admission and drains an owned route, retaining failed detach for retry.
    pub async fn delete_owned_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.remove_frame_route(&FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "owned",
        )?)
        .await
    }
    /// Deletes an owned route, consuming its endpoint values.
    pub async fn delete_owned_route(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.delete_owned_route_ref(&ingress, &egress).await
    }
    /// Adds a classic-to-owned adapter with default native-boundary policy.
    pub async fn add_classic_to_owned_route_ref(
        &mut self,
        ingress: &Endpoint,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.add_classic_to_owned_route_ref_with_native_policy(ingress, egress, None)
            .await
    }
    /// Adds a classic-to-owned adapter after complete native-route validation.
    pub async fn add_classic_to_owned_route_ref_with_native_policy(
        &mut self,
        ingress: &Endpoint,
        egress: &OwnedFrameEndpoint,
        policy: Option<&crate::NativeRoutePolicy>,
    ) -> Result<(), UStatus> {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "classic-owned",
        )?;
        self.vacant(&key)?;
        let agreement = crate::native_route::admit_classic_boundary(
            (
                &ingress.name,
                crate::NativeRouteApi::Classic,
                ingress.native_profile.as_ref(),
            ),
            (
                &egress.name,
                crate::NativeRouteApi::OwnedFrame,
                egress.native_profile.as_ref(),
            ),
            policy,
        )?;
        let diagnostic = adapter_diagnostic(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
        );
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = owned::attach_classic(
            ingress,
            filters,
            self.worker_config(),
            diagnostic,
            agreement,
            owned::OwnedDestination {
                endpoint: egress.clone(),
                label: format!(
                    "classic:{}@{} -> owned:{}@{}",
                    ingress.name, ingress.authority, egress.name, egress.authority
                ),
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        if let Some(policy) = policy {
            policy.record_activation();
        }
        Ok(())
    }
    /// Adds an owned-to-classic adapter with default native-boundary policy.
    pub async fn add_owned_to_classic_route_ref(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &Endpoint,
    ) -> Result<(), UStatus> {
        self.add_owned_to_classic_route_ref_with_native_policy(ingress, egress, None)
            .await
    }
    /// Adds an owned-to-classic adapter after complete native-route validation.
    pub async fn add_owned_to_classic_route_ref_with_native_policy(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &Endpoint,
        policy: Option<&crate::NativeRoutePolicy>,
    ) -> Result<(), UStatus> {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "owned-classic",
        )?;
        self.vacant(&key)?;
        let agreement = crate::native_route::admit_classic_boundary(
            (
                &ingress.name,
                crate::NativeRouteApi::OwnedFrame,
                ingress.native_profile.as_ref(),
            ),
            (
                &egress.name,
                crate::NativeRouteApi::Classic,
                egress.native_profile.as_ref(),
            ),
            policy,
        )?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = owned::attach_owned(
            ingress,
            filters,
            self.worker_config(),
            adapter_diagnostic(
                &ingress.name,
                &ingress.authority,
                &egress.name,
                &egress.authority,
            ),
            owned::ClassicDestination {
                endpoint: egress.clone(),
                agreement,
                label: format!(
                    "owned:{}@{} -> classic:{}@{}",
                    ingress.name, ingress.authority, egress.name, egress.authority
                ),
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        if let Some(policy) = policy {
            policy.record_activation();
        }
        Ok(())
    }
}

#[cfg(feature = "owned-frame-transport")]
fn adapter_diagnostic(
    ingress: &str,
    ingress_authority: &str,
    egress: &str,
    egress_authority: &str,
) -> RouteDiagnostic {
    let route_kind = crate::RouteKind::AdapterBacked;
    RouteDiagnostic {
        route: crate::DataPlaneRoute::from_parts(
            ingress,
            ingress_authority,
            egress,
            egress_authority,
        ),
        route_kind,
        copy_semantics: route_kind.copy_semantics(),
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
fn egress_catalog<T: PreparedEndpoint>(
    endpoint: &ZeroCopyFrameEndpoint<T>,
    options: CopyMinimizedRouteOptions,
) -> Result<Arc<crate::prepared_endpoint::EgressCatalog<T::Core>>, UStatus> {
    let catalog = endpoint.routes.as_ref().ok_or_else(|| {
        UStatus::fail_with_code(
            UCode::FailedPrecondition,
            "configure exact egress routes before installing a native route",
        )
    })?;
    catalog
        .check_alignment(options.payload_alignment)
        .map_err(frame_routes::status)?;
    Ok(Arc::clone(catalog))
}

#[cfg(feature = "experimental-copy-minimized-routing")]
impl UStreamer {
    /// Installs a prepared opaque route; the egress catalog must already exist.
    pub async fn add_copy_minimized_route_ref<I: PreparedEndpoint, E: PreparedEndpoint>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.add_copy_minimized_route_ref_with_options(
            ingress,
            egress,
            CopyMinimizedRouteOptions::default(),
        )
        .await
    }
    /// Installs native routing with explicit alignment bounded by setup reservations.
    pub async fn add_copy_minimized_route_ref_with_options<
        I: PreparedEndpoint,
        E: PreparedEndpoint,
    >(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus> {
        use up_rust::UWire;
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "native",
        )?;
        self.vacant(&key)?;
        if I::Wire::WIRE_ID.literal_id() != E::Wire::WIRE_ID.literal_id()
            || I::Wire::PAYLOAD_FAMILY_ID.literal_id() != E::Wire::PAYLOAD_FAMILY_ID.literal_id()
            || I::Wire::FORMAT_VERSION != E::Wire::FORMAT_VERSION
        {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "native route must preserve selected wire and payload family",
            ));
        }
        crate::endpoint::native_route_agreement(
            ingress.native_profile.as_ref(),
            egress.native_profile.as_ref(),
        )?;
        for (declared, configured) in [
            (
                ingress.native_profile.as_deref(),
                ingress.transport.native_agreement(),
            ),
            (
                egress.native_profile.as_deref(),
                egress.transport.native_agreement(),
            ),
        ] {
            if declared != configured.map(up_rust::NativeProfileAgreement::profile) {
                return Err(UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    "endpoint native profile differs from selected-wire agreement",
                ));
            }
        }
        let output = egress_catalog(egress, options)?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = native::attach(
            ingress,
            filters,
            self.worker_config(),
            RouteDiagnostic::copy_minimized(
                &ingress.name,
                &ingress.authority,
                &egress.name,
                &egress.authority,
            ),
            || native::NativeForward {
                egress: Arc::clone(&output),
                #[cfg(feature = "flow-evidence")]
                label: std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN").map(|_| {
                    format!(
                        "native:{}@{} -> native:{}@{}",
                        ingress.name, ingress.authority, egress.name, egress.authority,
                    )
                }),
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        Ok(())
    }
    /// Installs a prepared route whose selected wire is equal at compile time.
    ///
    /// Opt-in `flow-evidence` records the admitted borrowed frame and successful
    /// prepared handoff. Formatting/hashing costs belong to the observed process;
    /// copied-loan byte inspection remains the separate prepared observer API.
    pub async fn add_selected_wire_copy_minimized_route_ref<
        I: PreparedEndpoint,
        E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
    >(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.add_copy_minimized_route_ref(ingress, egress).await
    }
    /// Installs a same-wire prepared route with explicit alignment.
    pub async fn add_selected_wire_copy_minimized_route_ref_with_options<
        I: PreparedEndpoint,
        E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
    >(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus> {
        self.add_copy_minimized_route_ref_with_options(ingress, egress, options)
            .await
    }
    /// Closes admission, retries failed physical detach and drains the native route.
    pub async fn delete_copy_minimized_route_ref<I: PreparedEndpoint, E: PreparedEndpoint>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.remove_frame_route(&FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "native",
        )?)
        .await
    }
    /// Deletes the same-wire native route without revoking retained frames.
    pub async fn delete_selected_wire_copy_minimized_route_ref<
        I: PreparedEndpoint,
        E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
    >(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.delete_copy_minimized_route_ref(ingress, egress).await
    }
}

#[cfg(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
impl UStreamer {
    /// Adds an owned-to-native adapter using the existing exact egress catalog.
    ///
    /// Opt-in flow evidence observes ingress and successful native handoff using
    /// the submitted owned frame; its formatting and hashing costs are included.
    pub async fn add_owned_to_copy_minimized_route_ref<E: PreparedEndpoint>(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.add_owned_to_copy_minimized_route_ref_with_options(
            ingress,
            egress,
            CopyMinimizedRouteOptions::default(),
        )
        .await
    }
    /// Adds an owned-to-native adapter with explicit pre-reserved alignment.
    ///
    /// Opt-in flow evidence observes ingress and successful native handoff using
    /// the submitted owned frame; its formatting and hashing costs are included.
    pub async fn add_owned_to_copy_minimized_route_ref_with_options<E: PreparedEndpoint>(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus> {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "owned-native",
        )?;
        self.vacant(&key)?;
        crate::endpoint::native_route_agreement(
            ingress.native_profile.as_ref(),
            egress.native_profile.as_ref(),
        )?;
        let output = egress_catalog(egress, options)?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = owned::attach_owned(
            ingress,
            filters,
            self.worker_config(),
            RouteDiagnostic::adapter_backed(
                &ingress.name,
                &ingress.authority,
                &egress.name,
                &egress.authority,
            ),
            owned::NativeDestination {
                egress: output,
                #[cfg(feature = "flow-evidence")]
                label: std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN").map(|_| {
                    format!(
                        "owned:{}@{} -> native:{}@{}",
                        ingress.name, ingress.authority, egress.name, egress.authority,
                    )
                }),
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        Ok(())
    }
    /// Adds a native-to-owned copying adapter, preserving admitted identity and presence.
    pub async fn add_copy_minimized_to_owned_route_ref<I: PreparedEndpoint>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus>
    where
        I::Wire: Send + 'static,
    {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "native-owned",
        )?;
        self.vacant(&key)?;
        crate::endpoint::native_route_agreement(
            ingress.native_profile.as_ref(),
            egress.native_profile.as_ref(),
        )?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let destination = owned::OwnedDestination {
            endpoint: egress.clone(),
            label: format!(
                "native:{}@{} -> owned:{}@{}",
                ingress.name, ingress.authority, egress.name, egress.authority
            ),
        };
        let binding = native::attach(
            ingress,
            filters,
            self.worker_config(),
            RouteDiagnostic::adapter_backed(
                &ingress.name,
                &ingress.authority,
                &egress.name,
                &egress.authority,
            ),
            || owned::HeaderProjection::<I::Wire, _> {
                destination: destination.clone(),
                wire: std::marker::PhantomData,
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        Ok(())
    }
    /// Adds a classic-to-native adapter with default native-boundary policy.
    ///
    /// Opt-in flow evidence observes ingress and successful native handoff using
    /// the submitted owned frame; its formatting and hashing costs are included.
    pub async fn add_classic_to_copy_minimized_route_ref<E: PreparedEndpoint>(
        &mut self,
        ingress: &Endpoint,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus> {
        self.add_classic_to_copy_minimized_route_ref_with_native_policy(
            ingress, egress, options, None,
        )
        .await
    }
    /// Adds a classic-to-native adapter after complete-route native policy validation.
    ///
    /// Opt-in flow evidence observes ingress and successful native handoff using
    /// the submitted owned frame; its formatting and hashing costs are included.
    pub async fn add_classic_to_copy_minimized_route_ref_with_native_policy<E: PreparedEndpoint>(
        &mut self,
        ingress: &Endpoint,
        egress: &ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
        policy: Option<&crate::NativeRoutePolicy>,
    ) -> Result<(), UStatus> {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "classic-native",
        )?;
        self.vacant(&key)?;
        let agreement = crate::native_route::admit_classic_boundary(
            (
                &ingress.name,
                crate::NativeRouteApi::Classic,
                ingress.native_profile.as_ref(),
            ),
            (
                &egress.name,
                crate::NativeRouteApi::NativeLoan,
                egress.native_profile.as_ref(),
            ),
            policy,
        )?;
        let output = egress_catalog(egress, options)?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let binding = owned::attach_classic(
            ingress,
            filters,
            self.worker_config(),
            RouteDiagnostic::adapter_backed(
                &ingress.name,
                &ingress.authority,
                &egress.name,
                &egress.authority,
            ),
            agreement,
            owned::NativeDestination {
                egress: output,
                #[cfg(feature = "flow-evidence")]
                label: std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN").map(|_| {
                    format!(
                        "classic:{}@{} -> native:{}@{}",
                        ingress.name, ingress.authority, egress.name, egress.authority,
                    )
                }),
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        if let Some(policy) = policy {
            policy.record_activation();
        }
        Ok(())
    }
    /// Adds a native-to-classic copying adapter with default native-boundary policy.
    pub async fn add_copy_minimized_to_classic_route_ref<I: PreparedEndpoint>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &Endpoint,
    ) -> Result<(), UStatus>
    where
        I::Wire: Send + 'static,
    {
        self.add_copy_minimized_to_classic_route_ref_with_native_policy(ingress, egress, None)
            .await
    }
    /// Adds a native-to-classic adapter after complete-route native policy validation.
    pub async fn add_copy_minimized_to_classic_route_ref_with_native_policy<I: PreparedEndpoint>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &Endpoint,
        policy: Option<&crate::NativeRoutePolicy>,
    ) -> Result<(), UStatus>
    where
        I::Wire: Send + 'static,
    {
        let key = FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "native-classic",
        )?;
        self.vacant(&key)?;
        let agreement = crate::native_route::admit_classic_boundary(
            (
                &ingress.name,
                crate::NativeRouteApi::NativeLoan,
                ingress.native_profile.as_ref(),
            ),
            (
                &egress.name,
                crate::NativeRouteApi::Classic,
                egress.native_profile.as_ref(),
            ),
            policy,
        )?;
        let filters = self
            .frame_filters(&ingress.authority, &egress.authority)
            .await;
        let destination = owned::ClassicDestination {
            endpoint: egress.clone(),
            agreement,
            label: format!(
                "native:{}@{} -> classic:{}@{}",
                ingress.name, ingress.authority, egress.name, egress.authority
            ),
        };
        let binding = native::attach(
            ingress,
            filters,
            self.worker_config(),
            RouteDiagnostic::adapter_backed(
                &ingress.name,
                &ingress.authority,
                &egress.name,
                &egress.authority,
            ),
            || owned::HeaderProjection::<I::Wire, _> {
                destination: destination.clone(),
                wire: std::marker::PhantomData,
            },
        )
        .await?;
        self.frame_routes.insert(key, binding);
        if let Some(policy) = policy {
            policy.record_activation();
        }
        Ok(())
    }
    /// Deletes an owned-to-native route, retaining failed physical detach for retry.
    pub async fn delete_owned_to_copy_minimized_route_ref<E: PreparedEndpoint>(
        &mut self,
        ingress: &OwnedFrameEndpoint,
        egress: &ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.remove_frame_route(&FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "owned-native",
        )?)
        .await
    }
    /// Deletes a native-to-owned route without revoking retained native frames.
    pub async fn delete_copy_minimized_to_owned_route_ref<I: PreparedEndpoint>(
        &mut self,
        ingress: &ZeroCopyFrameEndpoint<I>,
        egress: &OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.remove_frame_route(&FrameRouteKey::new(
            &ingress.name,
            &ingress.authority,
            &egress.name,
            &egress.authority,
            "native-owned",
        )?)
        .await
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
macro_rules! native_value_methods {
    ($add:ident, $add_ref:ident, $options:ident, $options_ref:ident, $delete:ident, $delete_ref:ident) => {
        impl UStreamer {
            #[doc = "Installs a prepared native route, consuming endpoint values after registration."]
            pub async fn $add<I: PreparedEndpoint, E: PreparedEndpoint>(&mut self, ingress: ZeroCopyFrameEndpoint<I>, egress: ZeroCopyFrameEndpoint<E>) -> Result<(), UStatus> {
                self.$add_ref(&ingress, &egress).await
            }
            #[doc = "Installs a prepared native route with explicit alignment, consuming endpoint values."]
            pub async fn $options<I: PreparedEndpoint, E: PreparedEndpoint>(&mut self, ingress: ZeroCopyFrameEndpoint<I>, egress: ZeroCopyFrameEndpoint<E>, options: CopyMinimizedRouteOptions) -> Result<(), UStatus> {
                self.$options_ref(&ingress, &egress, options).await
            }
            #[doc = "Closes and drains a prepared native route, consuming endpoint values."]
            pub async fn $delete<I: PreparedEndpoint, E: PreparedEndpoint>(&mut self, ingress: ZeroCopyFrameEndpoint<I>, egress: ZeroCopyFrameEndpoint<E>) -> Result<(), UStatus> {
                self.$delete_ref(&ingress, &egress).await
            }
        }
    };
}
#[cfg(feature = "experimental-copy-minimized-routing")]
native_value_methods!(
    add_copy_minimized_route,
    add_copy_minimized_route_ref,
    add_copy_minimized_route_with_options,
    add_copy_minimized_route_ref_with_options,
    delete_copy_minimized_route,
    delete_copy_minimized_route_ref
);

#[cfg(test)]
#[path = "ustreamer_tests.rs"]
mod tests;

#[cfg(feature = "experimental-copy-minimized-routing")]
impl UStreamer {
    /// Adds a same-wire native route, consuming configured endpoints.
    pub async fn add_selected_wire_copy_minimized_route<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: PreparedEndpoint,
        E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
    {
        self.add_selected_wire_copy_minimized_route_ref(&ingress, &egress)
            .await
    }
    /// Adds a same-wire native route with explicit alignment, consuming endpoints.
    pub async fn add_selected_wire_copy_minimized_route_with_options<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus>
    where
        I: PreparedEndpoint,
        E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
    {
        self.add_selected_wire_copy_minimized_route_ref_with_options(&ingress, &egress, options)
            .await
    }
    /// Deletes a same-wire native route, consuming endpoint values.
    pub async fn delete_selected_wire_copy_minimized_route<I, E>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus>
    where
        I: PreparedEndpoint,
        E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
    {
        self.delete_selected_wire_copy_minimized_route_ref(&ingress, &egress)
            .await
    }
}

#[cfg(all(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
impl UStreamer {
    /// Adds an owned-to-native adapter, consuming endpoint values.
    pub async fn add_owned_to_copy_minimized_route<E: PreparedEndpoint>(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.add_owned_to_copy_minimized_route_ref(&ingress, &egress)
            .await
    }
    /// Adds an owned-to-native adapter with explicit alignment, consuming endpoints.
    pub async fn add_owned_to_copy_minimized_route_with_options<E: PreparedEndpoint>(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: ZeroCopyFrameEndpoint<E>,
        options: CopyMinimizedRouteOptions,
    ) -> Result<(), UStatus> {
        self.add_owned_to_copy_minimized_route_ref_with_options(&ingress, &egress, options)
            .await
    }
    /// Adds a native-to-owned adapter, consuming configured endpoints.
    pub async fn add_copy_minimized_to_owned_route<I: PreparedEndpoint>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus>
    where
        I::Wire: Send + 'static,
    {
        self.add_copy_minimized_to_owned_route_ref(&ingress, &egress)
            .await
    }
    /// Deletes an owned-to-native adapter, consuming its endpoint values.
    pub async fn delete_owned_to_copy_minimized_route<E: PreparedEndpoint>(
        &mut self,
        ingress: OwnedFrameEndpoint,
        egress: ZeroCopyFrameEndpoint<E>,
    ) -> Result<(), UStatus> {
        self.delete_owned_to_copy_minimized_route_ref(&ingress, &egress)
            .await
    }
    /// Deletes a native-to-owned adapter, consuming its endpoint values.
    pub async fn delete_copy_minimized_to_owned_route<I: PreparedEndpoint>(
        &mut self,
        ingress: ZeroCopyFrameEndpoint<I>,
        egress: OwnedFrameEndpoint,
    ) -> Result<(), UStatus> {
        self.delete_copy_minimized_to_owned_route_ref(&ingress, &egress)
            .await
    }
}
