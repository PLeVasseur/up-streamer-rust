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

use crate::control_plane::route_lifecycle::{
    insert_forwarding_rule, remove_forwarding_rule, ForwardingRules,
};
use crate::control_plane::route_table::build_forwarding_rule;
use crate::endpoint::Endpoint;
use crate::runtime::subscription_runtime::fetch_subscriptions;
use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use subscription_cache::SubscriptionCache;
use tokio::sync::Mutex;
use tracing::{debug, error};
use up_rust::core::usubscription::{FetchSubscriptionsRequest, SubscriberInfo, USubscription};
use up_rust::{UCode, UStatus, UTransport, UUri};

const USTREAMER_TAG: &str = "UStreamer:";
const USTREAMER_FN_NEW_TAG: &str = "new():";
const USTREAMER_FN_ADD_FORWARDING_RULE_TAG: &str = "add_forwarding_rule():";
const USTREAMER_FN_DELETE_FORWARDING_RULE_TAG: &str = "delete_forwarding_rule():";

pub(crate) fn uauthority_to_uuri(authority_name: &str) -> UUri {
    UUri {
        authority_name: authority_name.to_string(),
        ue_id: 0xFFFF_FFFF,
        ue_version_major: 0xFF,
        resource_id: 0xFFFF,
        ..Default::default()
    }
}

pub struct UStreamer {
    name: String,
    registered_forwarding_rules: ForwardingRules,
    transport_forwarders: crate::data_plane::egress_pool::TransportForwarders,
    forwarding_listeners: crate::data_plane::ingress_registry::ForwardingListeners,
    subscription_cache: Arc<Mutex<SubscriptionCache>>,
}

impl UStreamer {
    pub fn new(
        name: &str,
        message_queue_size: u16,
        usubscription: Arc<dyn USubscription>,
    ) -> Result<Self, UStatus> {
        let name = format!("{USTREAMER_TAG}:{name}:");
        debug!(
            "{}:{}:{} UStreamer created",
            &name, USTREAMER_TAG, USTREAMER_FN_NEW_TAG
        );

        let uuri: UUri = UUri {
            authority_name: "*".to_string(),
            ue_id: 0x0000_FFFF,
            ue_version_major: 0xFF,
            resource_id: 0xFFFF,
            ..Default::default()
        };

        let subscriber_info = SubscriberInfo {
            uri: Some(uuri).into(),
            ..Default::default()
        };

        let mut fetch_request = FetchSubscriptionsRequest {
            request: None,
            ..Default::default()
        };
        fetch_request.set_subscriber(subscriber_info);

        let subscriptions = fetch_subscriptions(usubscription, fetch_request);
        let subscription_cache = match SubscriptionCache::new(subscriptions) {
            Ok(cache) => Arc::new(Mutex::new(cache)),
            Err(e) => {
                return Err(UStatus::fail_with_code(
                    UCode::INVALID_ARGUMENT,
                    format!(
                        "{}:{}:{} Unable to create SubscriptionCache: {:?}",
                        name, USTREAMER_TAG, USTREAMER_FN_NEW_TAG, e
                    ),
                ))
            }
        };

        Ok(Self {
            name: name.to_string(),
            registered_forwarding_rules: Mutex::new(HashSet::new()),
            transport_forwarders: crate::data_plane::egress_pool::TransportForwarders::new(
                message_queue_size as usize,
            ),
            forwarding_listeners: crate::data_plane::ingress_registry::ForwardingListeners::new(),
            subscription_cache,
        })
    }

    #[inline(always)]
    fn forwarding_id(r#in: &Endpoint, out: &Endpoint) -> String {
        format!(
            "[in.name: {}, in.authority: {:?} ; out.name: {}, out.authority: {:?}]",
            r#in.name, r#in.authority, out.name, out.authority
        )
    }

    #[inline(always)]
    fn fail_due_to_same_authority(&self, r#in: &Endpoint, out: &Endpoint) -> Result<(), UStatus> {
        let err = Err(UStatus::fail_with_code(
            UCode::INVALID_ARGUMENT,
            format!(
                "{} are the same. Unable to delete.",
                Self::forwarding_id(r#in, out)
            ),
        ));
        error!(
            "{}:{}:{} forwarding rule failed: {:?}",
            self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG, err
        );
        err
    }

    pub async fn add_forwarding_rule(
        &mut self,
        r#in: Endpoint,
        out: Endpoint,
    ) -> Result<(), UStatus> {
        debug!(
            "{}:{}:{} Adding forwarding rule for {}",
            self.name,
            USTREAMER_TAG,
            USTREAMER_FN_ADD_FORWARDING_RULE_TAG,
            Self::forwarding_id(&r#in, &out)
        );

        if r#in.authority == out.authority {
            return self.fail_due_to_same_authority(&r#in, &out);
        }

        let forwarding_rule = build_forwarding_rule(&r#in, &out);
        let inserted =
            insert_forwarding_rule(&self.registered_forwarding_rules, forwarding_rule.clone())
                .await;

        if !inserted {
            return Err(UStatus::fail_with_code(
                UCode::ALREADY_EXISTS,
                "already exists",
            ));
        }

        let out_sender = self
            .transport_forwarders
            .insert(out.transport.clone())
            .await;

        if let Err(err) = self
            .forwarding_listeners
            .insert(
                r#in.transport.clone(),
                &r#in.authority,
                &out.authority,
                &Self::forwarding_id(&r#in, &out),
                out_sender,
                self.subscription_cache.clone(),
            )
            .await
        {
            remove_forwarding_rule(&self.registered_forwarding_rules, &forwarding_rule).await;
            self.transport_forwarders
                .remove(out.transport.clone())
                .await;
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                err.to_string(),
            ));
        }

        Ok(())
    }

    pub async fn delete_forwarding_rule(
        &mut self,
        r#in: Endpoint,
        out: Endpoint,
    ) -> Result<(), UStatus> {
        debug!(
            "{}:{}:{} Deleting forwarding rule for {}",
            self.name,
            USTREAMER_TAG,
            USTREAMER_FN_DELETE_FORWARDING_RULE_TAG,
            Self::forwarding_id(&r#in, &out)
        );

        if r#in.authority == out.authority {
            return self.fail_due_to_same_authority(&r#in, &out);
        }

        let forwarding_rule = build_forwarding_rule(&r#in, &out);
        let removed =
            remove_forwarding_rule(&self.registered_forwarding_rules, &forwarding_rule).await;

        if !removed {
            return Err(UStatus::fail_with_code(UCode::NOT_FOUND, "not found"));
        }

        self.transport_forwarders
            .remove(out.transport.clone())
            .await;
        self.forwarding_listeners
            .remove(
                r#in.transport,
                &r#in.authority,
                &out.authority,
                self.subscription_cache.clone(),
            )
            .await;

        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct ComparableTransport {
    transport: Arc<dyn UTransport>,
}

impl ComparableTransport {
    pub fn new(transport: Arc<dyn UTransport>) -> Self {
        Self { transport }
    }
}

impl Hash for ComparableTransport {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.transport).hash(state);
    }
}

impl PartialEq for ComparableTransport {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.transport, &other.transport)
    }
}

impl Eq for ComparableTransport {}

impl Debug for ComparableTransport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComparableTransport")
            .finish_non_exhaustive()
    }
}
