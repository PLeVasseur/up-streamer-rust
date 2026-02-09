//! Subscription-directory adapter used by routing and data-plane flows.

use std::collections::HashSet;
use std::sync::Arc;
use subscription_cache::{SubscriptionCache, SubscriptionInformation};
use tokio::sync::Mutex;
use tracing::warn;

#[allow(clippy::mutable_key_type)]
pub(crate) async fn resolve_subscribers_for_authority(
    subscription_cache: &Arc<Mutex<SubscriptionCache>>,
    out_authority: &str,
    tag: &str,
    action: &str,
) -> HashSet<SubscriptionInformation> {
    match subscription_cache
        .lock()
        .await
        .fetch_cache_entry_with_wildcard(out_authority)
    {
        Some(subscribers) => subscribers,
        None => {
            warn!("{tag}:{action} no subscribers found for out_authority: {out_authority:?}");
            HashSet::new()
        }
    }
}
