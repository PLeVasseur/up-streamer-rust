//! Runtime helper for spawning worker forwarding loops.

use std::sync::Arc;
use std::thread;
use tokio::runtime::Builder;
use tokio::sync::broadcast::Receiver;
use up_rust::{UMessage, UTransport};

const FORWARDER_RUNTIME_THREAD_NAME: &str = "up-streamer-forwarder-runtime";

pub(crate) fn spawn_message_forwarding_loop<F, Fut>(
    out_transport: Arc<dyn UTransport>,
    message_receiver: Receiver<Arc<UMessage>>,
    run_loop: F,
) where
    F: FnOnce(Arc<dyn UTransport>, Receiver<Arc<UMessage>>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    thread::Builder::new()
        .name(FORWARDER_RUNTIME_THREAD_NAME.to_string())
        .spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create forwarder Tokio runtime");

            runtime.block_on(run_loop(out_transport, message_receiver));
        })
        .expect("Failed to spawn forwarder runtime thread");
}
