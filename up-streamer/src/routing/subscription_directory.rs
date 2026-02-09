//! Subscription-directory adapter used by routing and data-plane flows.

use std::collections::HashMap;
use std::sync::Arc;
use subscription_cache::{SubscriptionCache, SubscriptionLookup};
use tokio::sync::Mutex;
use tracing::warn;

#[derive(Clone)]
/// Route-subscriber directory facade over the subscription cache.
pub(crate) struct SubscriptionDirectory {
    cache: Arc<Mutex<SubscriptionCache>>,
}

impl SubscriptionDirectory {
    /// Creates a directory facade over a shared subscription cache.
    pub(crate) fn new(cache: Arc<Mutex<SubscriptionCache>>) -> Self {
        Self { cache }
    }

    /// Looks up subscribers for one egress authority with wildcard matching.
    pub(crate) async fn lookup_route_subscribers(
        &self,
        out_authority: &str,
        tag: &str,
        action: &str,
    ) -> SubscriptionLookup {
        match self
            .cache
            .lock()
            .await
            .fetch_cache_entry_with_wildcard(out_authority)
        {
            Some(subscribers) => subscribers,
            None => {
                warn!("{tag}:{action} no subscribers found for out_authority: {out_authority:?}");
                HashMap::new()
            }
        }
    }
}
