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

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, canonicalize};
use std::path::PathBuf;
use std::str::FromStr;
use tracing::{debug, error, warn};
use up_rust::core::usubscription::{
    FetchSubscribersRequest, FetchSubscribersResponse, FetchSubscriptionsRequest,
    FetchSubscriptionsResponse, NotificationsRequest, ResetRequest, ResetResponse, SubscriberInfo,
    Subscription, SubscriptionRequest, SubscriptionResponse, USubscription, UnsubscribeRequest,
};
use up_rust::{UCode, UStatus, UUri};

const STATIC_RESOURCE_ID: u32 = 0x8001;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SubscriptionIdentityKey {
    topic: UUri,
    subscriber: UUri,
}

pub struct USubscriptionStaticFile {
    static_file: String,
}

impl USubscriptionStaticFile {
    pub fn new(static_file: String) -> Self {
        Self { static_file }
    }

    fn unsupported_operation_status(operation: &str) -> UStatus {
        UStatus::fail_with_code(
            UCode::UNIMPLEMENTED,
            format!("{operation} is not supported by USubscriptionStaticFile (read-only backend)"),
        )
    }

    fn canonicalized_static_file_path(&self) -> Result<PathBuf, UStatus> {
        let subscription_json_file = PathBuf::from(self.static_file.clone());
        debug!("subscription_json_file: {subscription_json_file:?}");

        let canonicalized_result = canonicalize(subscription_json_file);
        debug!("canonicalize: {canonicalized_result:?}");

        canonicalized_result.map_err(|error| {
            UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                format!("Static subscription file not found: {error:?}"),
            )
        })
    }

    fn read_static_config_json(&self) -> Result<Value, UStatus> {
        let subscription_json_file = self.canonicalized_static_file_path()?;
        let data = fs::read_to_string(subscription_json_file).map_err(|error| {
            UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                format!("Unable to read file: {error:?}"),
            )
        })?;

        serde_json::from_str(&data).map_err(|error| {
            UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                format!("Unable to parse JSON: {error:?}"),
            )
        })
    }

    #[allow(clippy::mutable_key_type)]
    fn parse_static_subscriptions(&self) -> Result<Vec<Subscription>, UStatus> {
        let value = self.read_static_config_json()?;
        let Some(entries) = value.as_object() else {
            return Err(UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                "Static subscription file must be a JSON object mapping topic URI keys to arrays of subscriber URI strings",
            ));
        };

        let mut subscriptions_by_key: HashMap<SubscriptionIdentityKey, Subscription> =
            HashMap::new();

        for (topic_key, subscriber_values) in entries {
            let mut topic = match UUri::from_str(topic_key) {
                Ok(uri) => uri,
                Err(error) => {
                    error!("Error deserializing topic '{topic_key}': {error}");
                    continue;
                }
            };

            if topic.resource_id != STATIC_RESOURCE_ID {
                warn!("Setting fixed resource_id {STATIC_RESOURCE_ID:#06X} for topic '{topic}'");
                topic.resource_id = STATIC_RESOURCE_ID;
            }

            let Some(subscribers) = subscriber_values.as_array() else {
                warn!("Ignoring non-array subscriber list for topic '{topic_key}'");
                continue;
            };

            for subscriber_value in subscribers {
                let Some(subscriber_str) = subscriber_value.as_str() else {
                    warn!("Unable to parse subscriber '{subscriber_value}'");
                    continue;
                };

                let subscriber_uri = match UUri::from_str(subscriber_str) {
                    Ok(uri) => uri,
                    Err(error) => {
                        error!("Error deserializing subscriber '{subscriber_str}': {error}");
                        continue;
                    }
                };

                let subscription_identity = SubscriptionIdentityKey {
                    topic: topic.clone(),
                    subscriber: subscriber_uri.clone(),
                };

                subscriptions_by_key
                    .entry(subscription_identity)
                    .or_insert_with(|| Subscription {
                        topic: Some(topic.clone()).into(),
                        subscriber: Some(SubscriberInfo {
                            uri: Some(subscriber_uri).into(),
                            ..Default::default()
                        })
                        .into(),
                        ..Default::default()
                    });
            }
        }

        Ok(subscriptions_by_key.into_values().collect())
    }
}

#[async_trait]
impl USubscription for USubscriptionStaticFile {
    async fn subscribe(
        &self,
        _subscription_request: SubscriptionRequest,
    ) -> Result<SubscriptionResponse, UStatus> {
        Err(Self::unsupported_operation_status("subscribe"))
    }

    async fn fetch_subscriptions(
        &self,
        fetch_subscriptions_request: FetchSubscriptionsRequest,
    ) -> Result<FetchSubscriptionsResponse, UStatus> {
        debug!("fetch_subscriptions request: {fetch_subscriptions_request:?}");

        let subscriptions = self.parse_static_subscriptions()?;
        debug!("Finished reading subscriptions\n{subscriptions:#?}");

        Ok(FetchSubscriptionsResponse {
            subscriptions,
            ..Default::default()
        })
    }

    async fn unsubscribe(&self, _unsubscribe_request: UnsubscribeRequest) -> Result<(), UStatus> {
        Err(Self::unsupported_operation_status("unsubscribe"))
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

    #[allow(clippy::mutable_key_type)]
    async fn fetch_subscribers(
        &self,
        fetch_subscribers_request: FetchSubscribersRequest,
    ) -> Result<FetchSubscribersResponse, UStatus> {
        let requested_topic = fetch_subscribers_request.topic.as_ref().ok_or_else(|| {
            UStatus::fail_with_code(
                UCode::INVALID_ARGUMENT,
                "fetch_subscribers requires a topic",
            )
        })?;

        let mut canonical_topic = requested_topic.clone();
        canonical_topic.resource_id = STATIC_RESOURCE_ID;
        let requested_topic_identity = canonical_topic;

        let subscriptions = self.parse_static_subscriptions()?;
        let mut subscribers_by_key: HashMap<UUri, SubscriberInfo> = HashMap::new();

        for subscription in subscriptions {
            let Some(topic) = subscription.topic.as_ref() else {
                continue;
            };
            if topic != &requested_topic_identity {
                continue;
            }

            let Some(subscriber) = subscription.subscriber.as_ref() else {
                continue;
            };
            let Some(subscriber_uri) = subscriber.uri.as_ref() else {
                continue;
            };

            subscribers_by_key
                .entry(subscriber_uri.clone())
                .or_insert_with(|| subscriber.clone());
        }

        Ok(FetchSubscribersResponse {
            subscribers: subscribers_by_key.into_values().collect(),
            ..Default::default()
        })
    }

    async fn reset(&self, _reset_request: ResetRequest) -> Result<ResetResponse, UStatus> {
        Ok(ResetResponse::default())
    }
}

#[cfg(test)]
mod tests {
    use super::USubscriptionStaticFile;
    use std::collections::HashSet;
    use std::fs;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use up_rust::core::usubscription::{FetchSubscribersRequest, USubscription};
    use up_rust::UUri;

    static TEST_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn write_static_config(contents: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let counter = TEST_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!(
            "usubscription-static-file-test-{}-{}.json",
            std::process::id(),
            counter
        ));

        fs::write(&path, contents).expect("static test config written");
        path
    }

    #[tokio::test]
    #[allow(clippy::mutable_key_type)]
    async fn fetch_subscribers_dedupes_duplicate_subscribers_after_topic_normalization() {
        let static_path = write_static_config(
            r#"{
                "//authority-a/5BA0/1/8001": [
                    "//authority-b/5678/1/1234",
                    "//authority-b/5678/1/1234",
                    "//authority-c/5678/1/1234"
                ],
                "//authority-a/5BA0/1/8002": [
                    "//authority-z/5678/1/1234"
                ]
            }"#,
        );

        let backend = USubscriptionStaticFile::new(static_path.to_string_lossy().to_string());

        let response = backend
            .fetch_subscribers(FetchSubscribersRequest {
                topic: Some(UUri::from_str("//authority-a/5BA0/1/FFFF").expect("valid topic"))
                    .into(),
                ..Default::default()
            })
            .await
            .expect("fetch_subscribers should succeed");

        fs::remove_file(&static_path).expect("remove static config file");

        let subscriber_uris: HashSet<UUri> = response
            .subscribers
            .into_iter()
            .filter_map(|subscriber| subscriber.uri.into_option())
            .collect();

        assert_eq!(subscriber_uris.len(), 3);
        assert!(subscriber_uris
            .contains(&UUri::from_str("//authority-b/5678/1/1234").expect("valid subscriber")));
        assert!(subscriber_uris
            .contains(&UUri::from_str("//authority-c/5678/1/1234").expect("valid subscriber")));
        assert!(subscriber_uris
            .contains(&UUri::from_str("//authority-z/5678/1/1234").expect("valid subscriber")));
    }
}
