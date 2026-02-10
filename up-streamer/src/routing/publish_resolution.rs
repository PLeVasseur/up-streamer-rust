//! Publish-source filter derivation and dedupe policy.

use std::collections::HashMap;
use tracing::{debug, warn};
use up_rust::UUri;

use crate::observability::{events, fields};
use crate::routing::subscription_cache::SubscriptionLookup;
use crate::routing::uri_identity_key::UriIdentityKey;

pub(crate) type SourceFilterLookup = HashMap<UriIdentityKey, UUri>;

const COMPONENT: &str = "publish_resolution";

/// Resolves publish source filters for route listeners under one ingress->egress pair.
pub(crate) struct PublishRouteResolver;

impl PublishRouteResolver {
    /// Returns `true` when a subscription topic can originate from the ingress authority.
    fn topic_matches_ingress_authority(ingress_authority: &str, topic: &UUri) -> bool {
        topic.authority_name == "*" || topic.authority_name == ingress_authority
    }

    /// Builds a single publish source filter for a subscriber topic when applicable.
    fn derive_source_filter_for_topic(
        ingress_authority: &str,
        egress_authority: &str,
        topic: &UUri,
    ) -> Option<UUri> {
        if !Self::topic_matches_ingress_authority(ingress_authority, topic) {
            debug!(
                event = events::PUBLISH_SOURCE_FILTER_SKIPPED,
                component = COMPONENT,
                in_authority = ingress_authority,
                out_authority = egress_authority,
                source_filter = %fields::format_uri(topic),
                reason = "topic_authority_mismatch",
                "skipping publish source filter due to topic authority mismatch"
            );
            return None;
        }

        match UUri::try_from_parts(
            ingress_authority,
            topic.ue_id,
            topic.uentity_major_version(),
            topic.resource_id(),
        ) {
            Ok(source_uri) => Some(source_uri),
            Err(err) => {
                warn!(
                    event = events::PUBLISH_SOURCE_FILTER_BUILD_FAILED,
                    component = COMPONENT,
                    in_authority = ingress_authority,
                    out_authority = egress_authority,
                    source_filter = %fields::format_uri(topic),
                    err = %err,
                    "unable to build publish source filter"
                );
                None
            }
        }
    }

    /// Derives deduplicated publish source filters for all matching subscribers.
    pub(crate) fn derive_source_filters(
        ingress_authority: &str,
        egress_authority: &str,
        subscribers: &SubscriptionLookup,
    ) -> SourceFilterLookup {
        let mut source_filters = HashMap::new();

        for subscriber in subscribers.values() {
            if let Some(source_uri) = Self::derive_source_filter_for_topic(
                ingress_authority,
                egress_authority,
                &subscriber.topic,
            ) {
                source_filters
                    .entry(UriIdentityKey::from(&source_uri))
                    .or_insert(source_uri);
            }
        }

        source_filters
    }
}

#[cfg(test)]
mod tests {
    use super::PublishRouteResolver;
    use crate::routing::subscription_cache::{
        SubscriptionIdentityKey, SubscriptionInformation, SubscriptionLookup,
    };
    use std::collections::HashMap;
    use std::str::FromStr;
    use up_rust::core::usubscription::SubscriberInfo;
    use up_rust::UUri;

    fn subscription_info(topic: &str, subscriber: &str) -> SubscriptionInformation {
        SubscriptionInformation {
            topic: UUri::from_str(topic).expect("valid topic UUri"),
            subscriber: SubscriberInfo {
                uri: Some(UUri::from_str(subscriber).expect("valid subscriber UUri")).into(),
                ..Default::default()
            },
        }
    }

    fn subscription_lookup(subscriptions: Vec<SubscriptionInformation>) -> SubscriptionLookup {
        let mut lookup = HashMap::new();

        for subscription in subscriptions {
            lookup.insert(SubscriptionIdentityKey::from(&subscription), subscription);
        }

        lookup
    }

    #[test]
    fn resolver_blocks_mismatched_topic_authority() {
        let topic = UUri::from_str("//authority-a/5BA0/1/8001").expect("valid topic UUri");

        let source = PublishRouteResolver::derive_source_filter_for_topic(
            "authority-c",
            "authority-b",
            &topic,
        );

        assert!(source.is_none());
    }

    #[test]
    fn resolver_allows_wildcard_topic_authority() {
        let topic = UUri::from_str("//*/5BA0/1/8001").expect("valid wildcard topic UUri");

        let source = PublishRouteResolver::derive_source_filter_for_topic(
            "authority-c",
            "authority-b",
            &topic,
        )
        .expect("wildcard topic should resolve");

        assert_eq!(source.authority_name, "authority-c");
        assert_eq!(source.ue_id, topic.ue_id);
        assert_eq!(
            source.uentity_major_version(),
            topic.uentity_major_version()
        );
        assert_eq!(source.resource_id(), topic.resource_id());
    }

    #[test]
    fn resolver_dedupes_sources_across_subscribers() {
        let subscribers = subscription_lookup(vec![
            subscription_info("//authority-a/5BA0/1/8001", "//authority-b/5678/1/1234"),
            subscription_info("//authority-a/5BA0/1/8001", "//authority-b/5679/1/1234"),
            subscription_info("//authority-z/5BA0/1/8001", "//authority-b/567A/1/1234"),
        ]);

        let filters =
            PublishRouteResolver::derive_source_filters("authority-a", "authority-b", &subscribers);

        assert_eq!(filters.len(), 1);
        let expected =
            UUri::try_from_parts("authority-a", 0x5BA0, 0x1, 0x8001).expect("expected source uri");
        assert!(filters
            .values()
            .any(|source_filter| source_filter == &expected));
    }
}
