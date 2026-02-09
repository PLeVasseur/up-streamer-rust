//! Ingress listener adapter that receives messages and feeds the egress queue.

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast::Sender;
use tracing::{debug, error};
use up_rust::{UListener, UMessage, UPayloadFormat};

const FORWARDING_LISTENER_TAG: &str = "ForwardingListener:";
const FORWARDING_LISTENER_FN_ON_RECEIVE_TAG: &str = "on_receive():";

#[derive(Clone)]
pub(crate) struct ForwardingListener {
    forwarding_id: String,
    sender: Sender<Arc<UMessage>>,
}

impl ForwardingListener {
    pub(crate) fn new(forwarding_id: &str, sender: Sender<Arc<UMessage>>) -> Self {
        Self {
            forwarding_id: forwarding_id.to_string(),
            sender,
        }
    }
}

#[async_trait]
impl UListener for ForwardingListener {
    async fn on_receive(&self, msg: UMessage) {
        debug!(
            "{}:{}:{} Received message: {:?}",
            self.forwarding_id,
            FORWARDING_LISTENER_TAG,
            FORWARDING_LISTENER_FN_ON_RECEIVE_TAG,
            &msg
        );

        if msg.attributes.payload_format.enum_value_or_default()
            == UPayloadFormat::UPAYLOAD_FORMAT_SHM
        {
            debug!(
                "{}:{}:{} Received message with type UPAYLOAD_FORMAT_SHM, which is not supported. A pointer to shared memory will not be usable on another device. UAttributes: {:#?}",
                self.forwarding_id,
                FORWARDING_LISTENER_TAG,
                FORWARDING_LISTENER_FN_ON_RECEIVE_TAG,
                &msg.attributes
            );
            return;
        }

        if let Err(e) = self.sender.send(Arc::new(msg)) {
            error!(
                "{}:{}:{} Unable to send message to worker pool: {e:?}",
                self.forwarding_id, FORWARDING_LISTENER_TAG, FORWARDING_LISTENER_FN_ON_RECEIVE_TAG,
            );
        }
    }
}
