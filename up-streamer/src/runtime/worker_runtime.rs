//! Runtime helper for spawning egress route dispatch loops.

use std::sync::Arc;
use std::thread;
use tokio::runtime::Builder;
use tokio::sync::broadcast::Receiver;
use up_rust::{UMessage, UTransport};

const LINUX_THREAD_NAME_MAX_LEN: usize = 15;
pub(crate) const DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME: &str = "up-egress-route";

fn sanitize_runtime_thread_name(thread_name: String) -> String {
    if thread_name.is_empty() || thread_name.len() > LINUX_THREAD_NAME_MAX_LEN {
        DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME.to_string()
    } else {
        thread_name
    }
}

pub(crate) fn spawn_route_dispatch_loop<F, Fut>(
    thread_name: String,
    out_transport: Arc<dyn UTransport>,
    message_receiver: Receiver<Arc<UMessage>>,
    run_loop: F,
) -> thread::JoinHandle<()>
where
    F: FnOnce(Arc<dyn UTransport>, Receiver<Arc<UMessage>>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let runtime_thread_name = sanitize_runtime_thread_name(thread_name);

    thread::Builder::new()
        .name(runtime_thread_name)
        .spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create egress route Tokio runtime");

            runtime.block_on(run_loop(out_transport, message_receiver));
        })
        .expect("Failed to spawn egress route runtime thread")
}

#[cfg(test)]
mod tests {
    use super::{sanitize_runtime_thread_name, DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME};

    #[test]
    fn sanitize_runtime_thread_name_keeps_valid_name() {
        let name = sanitize_runtime_thread_name("up-egress-12345".to_string());
        assert_eq!(name, "up-egress-12345");
    }

    #[test]
    fn sanitize_runtime_thread_name_uses_fallback_for_empty_name() {
        let name = sanitize_runtime_thread_name(String::new());
        assert_eq!(name, DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME);
    }

    #[test]
    fn sanitize_runtime_thread_name_uses_fallback_for_long_name() {
        let name = sanitize_runtime_thread_name("up-egress-thread-name-too-long".to_string());
        assert_eq!(name, DEFAULT_EGRESS_ROUTE_RUNTIME_THREAD_NAME);
    }
}
