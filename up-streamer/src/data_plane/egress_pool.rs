//! Egress forwarder pool and refcounted transport ownership.

use crate::data_plane::egress_worker::TransportForwarder;
use crate::ustreamer::ComparableTransport;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast::Sender;
use tokio::sync::Mutex;
use tracing::{debug, warn};
use up_rust::{UMessage, UTransport};

const TRANSPORT_FORWARDERS_TAG: &str = "TransportForwarders:";
const TRANSPORT_FORWARDERS_FN_INSERT_TAG: &str = "insert:";
const TRANSPORT_FORWARDERS_FN_REMOVE_TAG: &str = "remove:";

pub(crate) type TransportForwardersContainer =
    Mutex<HashMap<ComparableTransport, (usize, Arc<TransportForwarder>, Sender<Arc<UMessage>>)>>;

pub(crate) struct TransportForwarders {
    message_queue_size: usize,
    pub(crate) forwarders: TransportForwardersContainer,
}

impl TransportForwarders {
    pub(crate) fn new(message_queue_size: usize) -> Self {
        Self {
            message_queue_size,
            forwarders: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn insert(
        &mut self,
        out_transport: Arc<dyn UTransport>,
    ) -> Sender<Arc<UMessage>> {
        let out_comparable_transport = ComparableTransport::new(out_transport.clone());

        let mut transport_forwarders = self.forwarders.lock().await;

        let (active, _, sender) = transport_forwarders
            .entry(out_comparable_transport)
            .or_insert_with(|| {
                debug!(
                    "{TRANSPORT_FORWARDERS_TAG}:{TRANSPORT_FORWARDERS_FN_INSERT_TAG} Inserting..."
                );
                let (tx, rx) = tokio::sync::broadcast::channel(self.message_queue_size);
                (0, Arc::new(TransportForwarder::new(out_transport, rx)), tx)
            });
        *active += 1;
        sender.clone()
    }

    pub(crate) async fn remove(&mut self, out_transport: Arc<dyn UTransport>) {
        let out_comparable_transport = ComparableTransport::new(out_transport.clone());

        let mut transport_forwarders = self.forwarders.lock().await;

        let active_num = {
            let Some((active, _, _)) = transport_forwarders.get_mut(&out_comparable_transport)
            else {
                warn!("{TRANSPORT_FORWARDERS_TAG}:{TRANSPORT_FORWARDERS_FN_REMOVE_TAG} no such out_comparable_transport");
                return;
            };

            *active -= 1;
            *active
        };

        if active_num == 0 {
            let removed = transport_forwarders.remove(&out_comparable_transport);
            debug!("{TRANSPORT_FORWARDERS_TAG}:{TRANSPORT_FORWARDERS_FN_REMOVE_TAG} went to remove TransportForwarder for this transport");
            if removed.is_none() {
                warn!("{TRANSPORT_FORWARDERS_TAG}:{TRANSPORT_FORWARDERS_FN_REMOVE_TAG} was none to remove");
            } else {
                debug!("{TRANSPORT_FORWARDERS_TAG}:{TRANSPORT_FORWARDERS_FN_REMOVE_TAG} had one to remove");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TransportForwarders;
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
    async fn insert_same_transport_reuses_queue_and_increments_refcount() {
        let mut pool = TransportForwarders::new(8);
        let transport: Arc<dyn UTransport> = Arc::new(NoopTransport);

        let sender_a = pool.insert(transport.clone()).await;
        let sender_b = pool.insert(transport).await;

        let forwarders = pool.forwarders.lock().await;
        assert_eq!(forwarders.len(), 1);
        let (active, _, _) = forwarders
            .values()
            .next()
            .expect("single transport forwarder");
        assert_eq!(*active, 2);
        assert!(sender_a.same_channel(&sender_b));
    }

    #[tokio::test]
    async fn remove_drops_forwarder_when_refcount_reaches_zero() {
        let mut pool = TransportForwarders::new(8);
        let transport: Arc<dyn UTransport> = Arc::new(NoopTransport);

        pool.insert(transport.clone()).await;
        pool.insert(transport.clone()).await;

        pool.remove(transport.clone()).await;
        assert_eq!(pool.forwarders.lock().await.len(), 1);

        pool.remove(transport).await;
        assert!(pool.forwarders.lock().await.is_empty());
    }
}
