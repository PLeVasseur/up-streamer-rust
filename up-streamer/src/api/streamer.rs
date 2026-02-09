//! API facade helpers for the outward [`crate::UStreamer`] contract.

use crate::{Endpoint, UStreamer};
use std::sync::Arc;
use up_rust::core::usubscription::USubscription;
use up_rust::UStatus;

pub fn new(
    name: &str,
    message_queue_size: u16,
    usubscription: Arc<dyn USubscription>,
) -> Result<UStreamer, UStatus> {
    UStreamer::new(name, message_queue_size, usubscription)
}

pub async fn add_forwarding_rule(
    streamer: &mut UStreamer,
    r#in: Endpoint,
    out: Endpoint,
) -> Result<(), UStatus> {
    streamer.add_forwarding_rule(r#in, out).await
}

pub async fn delete_forwarding_rule(
    streamer: &mut UStreamer,
    r#in: Endpoint,
    out: Endpoint,
) -> Result<(), UStatus> {
    streamer.delete_forwarding_rule(r#in, out).await
}
