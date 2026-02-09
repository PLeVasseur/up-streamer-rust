//! API facade helpers for constructing [`crate::Endpoint`].

use crate::Endpoint;
use std::sync::Arc;
use tracing::debug;
use up_rust::UTransport;

const ENDPOINT_TAG: &str = "Endpoint:";
const ENDPOINT_FN_NEW_TAG: &str = "new():";

#[inline(always)]
pub(crate) fn build_endpoint(
    name: &str,
    authority: &str,
    transport: Arc<dyn UTransport>,
) -> Endpoint {
    debug!(
        "{}:{} Creating Endpoint from: ({:?})",
        ENDPOINT_TAG, ENDPOINT_FN_NEW_TAG, authority,
    );

    Endpoint {
        name: name.to_string(),
        authority: authority.to_string(),
        transport,
    }
}

#[cfg(test)]
mod tests {
    use super::build_endpoint;
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

    #[test]
    fn build_endpoint_populates_fields() {
        let transport: Arc<dyn UTransport> = Arc::new(NoopTransport);
        let endpoint = build_endpoint("left", "left-authority", transport);

        assert_eq!(endpoint.name, "left");
        assert_eq!(endpoint.authority, "left-authority");
    }
}
