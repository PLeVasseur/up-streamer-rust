//! Ingress-route listener adapter that receives messages and feeds egress dispatch.

use crate::observability::{events, fields};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast::Sender;
use tracing::{debug, error};
use up_rust::{UListener, UMessage, UPayloadFormat};

const COMPONENT: &str = "ingress_listener";

#[derive(Clone)]
pub(crate) struct IngressRouteListener {
    route_id: String,
    sender: Sender<Arc<UMessage>>,
}

impl IngressRouteListener {
    pub(crate) fn new(route_id: &str, sender: Sender<Arc<UMessage>>) -> Self {
        Self {
            route_id: route_id.to_string(),
            sender,
        }
    }
}

#[async_trait]
impl UListener for IngressRouteListener {
    async fn on_receive(&self, msg: UMessage) {
        let route_label = self.route_id.as_str();

        debug!(
            event = events::INGRESS_RECEIVE,
            component = COMPONENT,
            route_label,
            msg_id = %fields::format_message_id(&msg),
            msg_type = %fields::format_message_type(&msg),
            src = %fields::format_source_uri(&msg),
            sink = %fields::format_sink_uri(&msg),
            "received ingress message"
        );

        if msg.attributes.payload_format.enum_value_or_default()
            == UPayloadFormat::UPAYLOAD_FORMAT_SHM
        {
            debug!(
                event = events::INGRESS_DROP_UNSUPPORTED_PAYLOAD,
                component = COMPONENT,
                route_label,
                msg_id = %fields::format_message_id(&msg),
                msg_type = %fields::format_message_type(&msg),
                src = %fields::format_source_uri(&msg),
                sink = %fields::format_sink_uri(&msg),
                reason = "unsupported_payload_format_shm",
                "dropping unsupported shared-memory payload"
            );
            return;
        }

        if let Err(e) = self.sender.send(Arc::new(msg)) {
            error!(
                event = events::INGRESS_SEND_TO_POOL_FAILED,
                component = COMPONENT,
                route_label,
                err = ?e,
                "unable to send message to egress pool"
            );
        }
    }
}
