// Copyright (c) 2024, 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Subscription refresh health regressions retained across route-worker migration.

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
    async fn reset(&self, _reason: ResetReason, _message: Option<String>) -> Result<(), UStatus> {
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
        "//authority-a/5BA0/1/1/8001",
        "//authority-b/5678/1/1/1234",
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
    let usubscription: Arc<dyn USubscription> = Arc::new(SequencedUSubscription::new(vec![Err(
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
