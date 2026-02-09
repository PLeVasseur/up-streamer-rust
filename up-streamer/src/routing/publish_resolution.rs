//! Publish-source filter derivation and dedupe policy.

use std::collections::HashSet;
use subscription_cache::SubscriptionInformation;
use tracing::{debug, warn};
use up_rust::UUri;

pub(crate) fn derive_publish_source_filter(
    in_authority: &str,
    out_authority: &str,
    topic: &UUri,
    tag: &str,
    action: &str,
) -> Option<UUri> {
    if topic.authority_name != "*" && topic.authority_name != in_authority {
        debug!(
            "{tag}:{action} skipping publish listener {action} for in_authority='{in_authority}', out_authority='{out_authority}', topic_authority='{}', topic={topic:?}",
            topic.authority_name
        );
        return None;
    }

    match UUri::try_from_parts(
        in_authority,
        topic.ue_id,
        topic.uentity_major_version(),
        topic.resource_id(),
    ) {
        Ok(source_uri) => Some(source_uri),
        Err(err) => {
            warn!(
                "{tag}:{action} unable to build publish source URI for in_authority='{in_authority}', out_authority='{out_authority}', topic={topic:?}: {err}"
            );
            None
        }
    }
}

#[allow(clippy::mutable_key_type)]
pub(crate) fn derive_publish_source_filters(
    in_authority: &str,
    out_authority: &str,
    subscribers: &HashSet<SubscriptionInformation>,
    tag: &str,
    action: &str,
) -> HashSet<UUri> {
    let mut source_filters = HashSet::new();

    for subscriber in subscribers {
        if let Some(source_uri) = derive_publish_source_filter(
            in_authority,
            out_authority,
            &subscriber.topic,
            tag,
            action,
        ) {
            source_filters.insert(source_uri);
        }
    }

    source_filters
}

#[cfg(test)]
mod tests {
    use super::{derive_publish_source_filter, derive_publish_source_filters};
    use std::collections::HashSet;
    use std::str::FromStr;
    use subscription_cache::SubscriptionInformation;
    use up_rust::core::usubscription::{
        EventDeliveryConfig, SubscribeAttributes, SubscriberInfo, SubscriptionStatus,
    };
    use up_rust::UUri;

    fn subscription_info(topic: &str, subscriber: &str) -> SubscriptionInformation {
        SubscriptionInformation {
            topic: UUri::from_str(topic).expect("valid topic UUri"),
            subscriber: SubscriberInfo {
                uri: Some(UUri::from_str(subscriber).expect("valid subscriber UUri")).into(),
                ..Default::default()
            },
            status: SubscriptionStatus::default(),
            attributes: SubscribeAttributes::default(),
            config: EventDeliveryConfig::default(),
        }
    }

    #[test]
    fn derive_publish_source_filter_blocks_mismatched_authority() {
        let topic = UUri::from_str("//authority-a/5BA0/1/8001").expect("valid topic UUri");

        let source = derive_publish_source_filter(
            "authority-c",
            "authority-b",
            &topic,
            "routing-test",
            "insert",
        );

        assert!(source.is_none());
    }

    #[test]
    fn derive_publish_source_filter_allows_wildcard_topic_authority() {
        let topic = UUri::from_str("//*/5BA0/1/8001").expect("valid wildcard topic UUri");

        let source = derive_publish_source_filter(
            "authority-c",
            "authority-b",
            &topic,
            "routing-test",
            "insert",
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
    #[allow(clippy::mutable_key_type)]
    fn derive_publish_source_filters_dedupes_sources_across_subscribers() {
        let mut subscribers = HashSet::new();
        subscribers.insert(subscription_info(
            "//authority-a/5BA0/1/8001",
            "//authority-b/5678/1/1234",
        ));
        subscribers.insert(subscription_info(
            "//authority-a/5BA0/1/8001",
            "//authority-b/5679/1/1234",
        ));
        subscribers.insert(subscription_info(
            "//authority-z/5BA0/1/8001",
            "//authority-b/567A/1/1234",
        ));

        let filters = derive_publish_source_filters(
            "authority-a",
            "authority-b",
            &subscribers,
            "routing-test",
            "insert",
        );

        assert_eq!(filters.len(), 1);
        let expected =
            UUri::try_from_parts("authority-a", 0x5BA0, 0x1, 0x8001).expect("expected source uri");
        assert!(filters.contains(&expected));
    }
}
