//! Egress worker abstraction that forwards queued messages on output transports.

use crate::runtime::worker_runtime::spawn_message_forwarding_loop;
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tracing::{debug, info, trace, warn};
use up_rust::{UMessage, UTransport, UUID};

const TRANSPORT_FORWARDER_TAG: &str = "TransportForwarder:";
const TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG: &str = "message_forwarding_loop():";

pub(crate) struct TransportForwarder {}

impl TransportForwarder {
    pub(crate) fn new(
        out_transport: Arc<dyn UTransport>,
        message_receiver: Receiver<Arc<UMessage>>,
    ) -> Self {
        let out_transport_clone = out_transport.clone();
        let message_receiver_clone = message_receiver.resubscribe();

        spawn_message_forwarding_loop(
            out_transport_clone,
            message_receiver_clone,
            move |out_transport, message_receiver| async move {
                trace!("Within blocked runtime");
                Self::message_forwarding_loop(
                    UUID::build().to_hyphenated_string(),
                    out_transport,
                    message_receiver,
                )
                .await;
                info!("Broke out of loop! You probably dropped the UPClientVsomeip");
            },
        );

        Self {}
    }

    pub(crate) async fn message_forwarding_loop(
        id: String,
        out_transport: Arc<dyn UTransport>,
        mut message_receiver: Receiver<Arc<UMessage>>,
    ) {
        while let Ok(msg) = message_receiver.recv().await {
            debug!(
                "{}:{}:{} Attempting send of message: {:?}",
                id,
                TRANSPORT_FORWARDER_TAG,
                TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG,
                msg
            );
            let send_res = out_transport.send(msg.deref().clone()).await;
            if let Err(err) = send_res {
                warn!(
                    "{}:{}:{} Sending on out_transport failed: {:?}",
                    id,
                    TRANSPORT_FORWARDER_TAG,
                    TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG,
                    err
                );
            } else {
                debug!(
                    "{}:{}:{} Sending on out_transport succeeded",
                    id, TRANSPORT_FORWARDER_TAG, TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG
                );
            }
        }
    }
}
