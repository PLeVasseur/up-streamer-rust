//! Forwarding-rule lifecycle primitives for the control plane.

use crate::control_plane::route_table::ForwardingRule;
use std::collections::HashSet;
use tokio::sync::Mutex;

pub(crate) type ForwardingRules = Mutex<HashSet<ForwardingRule>>;

pub(crate) async fn insert_forwarding_rule(
    registered_forwarding_rules: &ForwardingRules,
    forwarding_rule: ForwardingRule,
) -> bool {
    let mut registered = registered_forwarding_rules.lock().await;
    registered.insert(forwarding_rule)
}

pub(crate) async fn remove_forwarding_rule(
    registered_forwarding_rules: &ForwardingRules,
    forwarding_rule: &ForwardingRule,
) -> bool {
    let mut registered = registered_forwarding_rules.lock().await;
    registered.remove(forwarding_rule)
}

#[cfg(test)]
mod tests {
    use super::{insert_forwarding_rule, remove_forwarding_rule, ForwardingRules};
    use crate::control_plane::route_table::ForwardingRule;
    use crate::ustreamer::ComparableTransport;
    use async_trait::async_trait;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use up_rust::{UCode, UListener, UMessage, UStatus, UTransport, UUri};

    struct NoopTransport;

    #[async_trait]
    impl UTransport for NoopTransport {
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            Ok(())
        }

        async fn receive(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
        ) -> Result<UMessage, UStatus> {
            Err(UStatus::fail_with_code(
                UCode::UNIMPLEMENTED,
                "not used in tests",
            ))
        }

        async fn register_listener(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            Ok(())
        }

        async fn unregister_listener(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            Ok(())
        }
    }

    fn forwarding_rule(in_authority: &str, out_authority: &str) -> ForwardingRule {
        let in_transport: Arc<dyn UTransport> = Arc::new(NoopTransport);
        let out_transport: Arc<dyn UTransport> = Arc::new(NoopTransport);
        (
            in_authority.to_string(),
            out_authority.to_string(),
            ComparableTransport::new(in_transport),
            ComparableTransport::new(out_transport),
        )
    }

    #[tokio::test]
    async fn insert_forwarding_rule_returns_false_for_duplicate() {
        let rules: ForwardingRules = Mutex::new(HashSet::new());
        let rule = forwarding_rule("authority-a", "authority-b");

        assert!(insert_forwarding_rule(&rules, rule.clone()).await);
        assert!(!insert_forwarding_rule(&rules, rule).await);
    }

    #[tokio::test]
    async fn remove_forwarding_rule_is_idempotent() {
        let rules: ForwardingRules = Mutex::new(HashSet::new());
        let rule = forwarding_rule("authority-a", "authority-b");

        assert!(insert_forwarding_rule(&rules, rule.clone()).await);
        assert!(remove_forwarding_rule(&rules, &rule).await);
        assert!(!remove_forwarding_rule(&rules, &rule).await);
    }
}
