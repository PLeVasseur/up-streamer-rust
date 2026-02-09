//! Forwarding-rule identity tuple construction for the control plane.

use crate::endpoint::Endpoint;
use crate::control_plane::transport_identity::TransportIdentityKey;

pub(crate) type ForwardingRule = (String, String, TransportIdentityKey, TransportIdentityKey);

#[inline(always)]
pub(crate) fn build_forwarding_rule(r#in: &Endpoint, out: &Endpoint) -> ForwardingRule {
    (
        r#in.authority.clone(),
        out.authority.clone(),
        TransportIdentityKey::new(r#in.transport.clone()),
        TransportIdentityKey::new(out.transport.clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::build_forwarding_rule;
    use crate::Endpoint;
    use async_trait::async_trait;
    use std::sync::Arc;
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

    #[tokio::test]
    async fn build_forwarding_rule_uses_transport_identity_in_tuple() {
        let shared_transport: Arc<dyn UTransport> = Arc::new(NoopTransport);
        let another_transport: Arc<dyn UTransport> = Arc::new(NoopTransport);

        let in_endpoint = Endpoint::new("in", "authority-a", shared_transport.clone());
        let out_endpoint_a = Endpoint::new("out-a", "authority-b", another_transport.clone());
        let out_endpoint_b = Endpoint::new("out-b", "authority-b", another_transport);
        let out_endpoint_c = Endpoint::new("out-c", "authority-b", Arc::new(NoopTransport));

        let rule_a = build_forwarding_rule(&in_endpoint, &out_endpoint_a);
        let rule_b = build_forwarding_rule(&in_endpoint, &out_endpoint_b);
        let rule_c = build_forwarding_rule(&in_endpoint, &out_endpoint_c);

        assert!(rule_a == rule_b);
        assert!(rule_a != rule_c);
    }
}
