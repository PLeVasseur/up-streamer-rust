//! Egress worker abstraction that forwards queued messages on output transports.

use crate::runtime::worker_runtime::{
    spawn_route_dispatch_loop, DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME,
};
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::broadcast::{error::RecvError, Receiver};
use tracing::{debug, info, trace, warn};
use up_rust::{UMessage, UTransport, UUID};

const EGRESS_ROUTE_WORKER_TAG: &str = "EgressRouteWorker:";
const EGRESS_ROUTE_WORKER_FN_RUN_LOOP_TAG: &str = "run_loop():";
const EGRESS_ROUTE_RUNTIME_THREAD_NAME_PREFIX: &str = "up-egress-";
const EGRESS_ROUTE_RUNTIME_THREAD_NAME_MAX_LEN: usize = 15;

/// Worker state that owns the spawned route-dispatch thread handle.
pub(crate) struct EgressRouteWorker {
    join_handle: std::thread::JoinHandle<()>,
}

impl EgressRouteWorker {
    /// Spawns a dedicated runtime thread for one egress transport dispatch loop.
    pub(crate) fn new(
        out_transport: Arc<dyn UTransport>,
        message_receiver: Receiver<Arc<UMessage>>,
    ) -> Self {
        let out_transport_clone = out_transport.clone();
        let message_receiver_clone = message_receiver.resubscribe();
        let route_id = UUID::build().to_hyphenated_string();
        let runtime_thread_name = Self::build_runtime_thread_name(&route_id);
        let route_id_for_loop = route_id.clone();

        let join_handle = spawn_route_dispatch_loop(
            runtime_thread_name,
            out_transport_clone,
            message_receiver_clone,
            move |out_transport, message_receiver| async move {
                trace!("Within blocked runtime");
                Self::route_dispatch_loop(route_id_for_loop, out_transport, message_receiver).await;
            },
        );

        Self { join_handle }
    }

    /// Returns the backing runtime thread ID for diagnostics.
    pub(crate) fn thread_id(&self) -> std::thread::ThreadId {
        self.join_handle.thread().id()
    }

    fn build_runtime_thread_name(route_id: &str) -> String {
        let suffix_len = EGRESS_ROUTE_RUNTIME_THREAD_NAME_MAX_LEN
            - EGRESS_ROUTE_RUNTIME_THREAD_NAME_PREFIX.len();
        let suffix: String = route_id
            .chars()
            .filter(|ch| ch.is_ascii_hexdigit())
            .take(suffix_len)
            .collect();

        if suffix.len() == suffix_len {
            format!("{EGRESS_ROUTE_RUNTIME_THREAD_NAME_PREFIX}{suffix}")
        } else {
            DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME.to_string()
        }
    }

    /// Executes the dispatch loop by forwarding each received message to egress transport.
    pub(crate) async fn route_dispatch_loop(
        id: String,
        out_transport: Arc<dyn UTransport>,
        mut message_receiver: Receiver<Arc<UMessage>>,
    ) {
        loop {
            match message_receiver.recv().await {
                Ok(msg) => {
                    debug!(
                        "{}:{}:{} Attempting send of message: {:?}",
                        id, EGRESS_ROUTE_WORKER_TAG, EGRESS_ROUTE_WORKER_FN_RUN_LOOP_TAG, msg
                    );
                    let send_res = out_transport.send(msg.deref().clone()).await;
                    if let Err(err) = send_res {
                        warn!(
                            "{}:{}:{} Sending on out_transport failed: {:?}",
                            id, EGRESS_ROUTE_WORKER_TAG, EGRESS_ROUTE_WORKER_FN_RUN_LOOP_TAG, err
                        );
                    } else {
                        debug!(
                            "{}:{}:{} Sending on out_transport succeeded",
                            id, EGRESS_ROUTE_WORKER_TAG, EGRESS_ROUTE_WORKER_FN_RUN_LOOP_TAG
                        );
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    warn!(
                        "{}:{}:{} Receiver lagged and skipped {} queued messages",
                        id, EGRESS_ROUTE_WORKER_TAG, EGRESS_ROUTE_WORKER_FN_RUN_LOOP_TAG, skipped
                    );
                }
                Err(RecvError::Closed) => {
                    info!(
                        "{}:{}:{} Receiver closed; stopping dispatch loop",
                        id, EGRESS_ROUTE_WORKER_TAG, EGRESS_ROUTE_WORKER_FN_RUN_LOOP_TAG
                    );
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EgressRouteWorker, EGRESS_ROUTE_RUNTIME_THREAD_NAME_MAX_LEN,
        EGRESS_ROUTE_RUNTIME_THREAD_NAME_PREFIX,
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use up_rust::{UCode, UListener, UMessage, UStatus, UTransport, UUri};

    #[derive(Default)]
    struct CountingTransport {
        send_count: AtomicUsize,
    }

    impl CountingTransport {
        fn sent_count(&self) -> usize {
            self.send_count.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl UTransport for CountingTransport {
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            self.send_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn receive(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
        ) -> Result<UMessage, UStatus> {
            Err(UStatus::fail_with_code(
                UCode::UNIMPLEMENTED,
                "receive is not used by egress worker tests",
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
    async fn route_dispatch_loop_exits_on_closed_receiver() {
        let transport = Arc::new(CountingTransport::default());
        let out_transport: Arc<dyn UTransport> = transport.clone();
        let (sender, receiver) = broadcast::channel(8);
        drop(sender);

        EgressRouteWorker::route_dispatch_loop("closed-loop".to_string(), out_transport, receiver)
            .await;

        assert_eq!(transport.sent_count(), 0);
    }

    #[tokio::test]
    async fn route_dispatch_loop_does_not_forward_after_close() {
        let transport = Arc::new(CountingTransport::default());
        let out_transport: Arc<dyn UTransport> = transport.clone();
        let (sender, receiver) = broadcast::channel(8);

        sender
            .send(Arc::new(UMessage::default()))
            .expect("queue should accept pre-close message");
        drop(sender);

        EgressRouteWorker::route_dispatch_loop(
            "close-forwarding".to_string(),
            out_transport,
            receiver,
        )
        .await;

        assert_eq!(transport.sent_count(), 1);
    }

    #[tokio::test]
    async fn route_dispatch_loop_continues_after_lagged_receive() {
        let transport = Arc::new(CountingTransport::default());
        let out_transport: Arc<dyn UTransport> = transport.clone();
        let (sender, receiver) = broadcast::channel(1);

        sender
            .send(Arc::new(UMessage::default()))
            .expect("queue should accept first message");
        sender
            .send(Arc::new(UMessage::default()))
            .expect("queue should accept second message");
        drop(sender);

        EgressRouteWorker::route_dispatch_loop("lagged-loop".to_string(), out_transport, receiver)
            .await;

        assert_eq!(transport.sent_count(), 1);
    }

    #[test]
    fn build_runtime_thread_name_keeps_prefix_and_linux_safe_length() {
        let thread_name = EgressRouteWorker::build_runtime_thread_name("abcdef0123456789");

        assert!(thread_name.starts_with(EGRESS_ROUTE_RUNTIME_THREAD_NAME_PREFIX));
        assert_eq!(thread_name.len(), EGRESS_ROUTE_RUNTIME_THREAD_NAME_MAX_LEN);
    }

    #[test]
    fn build_runtime_thread_name_uses_fallback_for_short_non_hex_ids() {
        let thread_name = EgressRouteWorker::build_runtime_thread_name("zzz");

        assert_eq!(
            thread_name,
            crate::runtime::worker_runtime::DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME
        );
    }
}
