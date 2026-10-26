// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Selected-wire RPC exchange flow with explicitly configured receive/send policy.
#![allow(dead_code)]

use super::{flow, payloads::NativeContext};
use configurable_streamer_wire_support::prepared::{PreparedEndpoint, PreparedSession};
use std::{future::Future, sync::Arc, time::Duration};
use up_rust::{UCode, UFrameMetadata, UFrameView, UOwnedTransport, UStatus, UUri};

#[derive(Clone)]
pub(crate) struct ClientOptions {
    pub response_source: UUri,
    pub response_sink: UUri,
    pub timeout_ms: u64,
    pub send_count: u64,
    pub interval: Duration,
    pub receive_settle: Duration,
    pub alignment: usize,
    pub native: Option<NativeContext>,
}

impl ClientOptions {
    fn sends(&self) -> Result<flow::SendOptions, UStatus> {
        Ok(flow::SendOptions {
            count: usize::try_from(self.send_count.max(1))
                .map_err(|_| flow::invalid("send count exceeds target usize"))?,
            interval: Some(self.interval),
        })
    }
}

pub(crate) async fn owned(
    transport: &Arc<dyn UOwnedTransport>,
    options: &ClientOptions,
    request: impl Fn(u32) -> Result<(UFrameMetadata, Vec<u8>), UStatus>,
) -> Result<(), UStatus> {
    super::startup::wait_for_start().await?;
    let sends = options.sends()?;
    exchanges(options, request, |metadata, payload| async move {
        let receiver = transport.clone();
        let receive_options = options.clone();
        let task = tokio::spawn(async move {
            let frame = flow::receive_owned_pull(
                &receiver,
                &receive_options.response_source,
                Some(&receive_options.response_sink),
                receive_options.timeout_ms,
                receive_options.native.as_ref(),
            )
            .await?;
            Ok(frame.payload_bytes().to_vec())
        });
        tokio::time::sleep(options.receive_settle).await;
        let sent = flow::send_owned_repeated(transport, metadata, &payload, sends).await;
        finish(task, sent).await
    })
    .await
}

pub(crate) async fn native<T>(
    transport: &Arc<PreparedSession<T>>,
    options: &ClientOptions,
    request: impl Fn(u32) -> Result<(UFrameMetadata, Vec<u8>), UStatus>,
) -> Result<(), UStatus>
where
    T: PreparedEndpoint,
{
    let sends = options.sends()?;
    let mut receiver = super::receive::register_native(
        transport,
        &options.response_source,
        Some(&options.response_sink),
    )
    .await?;
    let result = async {
        super::startup::wait_for_start().await?;
        tokio::time::sleep(options.receive_settle).await;
        let mut exchange = 0u64;
        loop {
            let sequence =
                u32::try_from(exchange.saturating_add(1).min(u64::from(u32::MAX))).unwrap();
            let (metadata, payload) = request(sequence)?;
            let request_id = metadata.id().clone();
            // Registration and the expected correlation ID exist before publication;
            // immediate responses cannot race listener creation. One receiver is
            // reused across exchanges, with no task allocated for each request.
            flow::send_native_repeated(transport, metadata, &payload, options.alignment, sends)
                .await?;
            let deadline = tokio::time::Instant::now()
                .checked_add(Duration::from_millis(options.timeout_ms))
                .ok_or_else(|| flow::invalid("response deadline overflow"))?;
            let response = loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(UStatus::fail_with_code(
                        UCode::DeadlineExceeded,
                        "correlated response timed out",
                    ));
                }
                let frame = receiver
                    .receive(
                        u64::try_from(remaining.as_millis())
                            .unwrap_or(u64::MAX)
                            .max(1),
                    )
                    .await?;
                if frame.metadata().reqid() == Some(&request_id) {
                    break frame;
                }
            };
            let bytes = super::payloads::copy_payload_bytes(&response)?;
            if payload != bytes {
                return Err(flow::invalid("correlated response payload mismatch"));
            }
            println!(
                "FLOW observed_payload_bytes={} role=rpc_client",
                bytes.len()
            );
            exchange = exchange.checked_add(1).ok_or_else(|| {
                UStatus::fail_with_code(UCode::OutOfRange, "RPC exchange counter exhausted")
            })?;
            if options.send_count != 0 {
                return Ok(());
            }
            tokio::time::sleep(options.interval).await;
        }
    }
    .await;
    let cleanup = receiver.stop().await;
    result?;
    cleanup
}

async fn exchanges<F: Future<Output = Result<Vec<u8>, UStatus>>>(
    options: &ClientOptions,
    request: impl Fn(u32) -> Result<(UFrameMetadata, Vec<u8>), UStatus>,
    mut exchange: impl FnMut(UFrameMetadata, Vec<u8>) -> F,
) -> Result<(), UStatus> {
    let mut exchanges = 0_u64;
    loop {
        let sequence = exchanges.saturating_add(1).min(u64::from(u32::MAX)) as u32;
        let (metadata, payload) = request(sequence)?;
        let response = exchange(metadata, payload.clone()).await?;
        if payload != response {
            return Err(flow::invalid(format!(
                "response payload mismatch: expected {} bytes, observed {} bytes",
                payload.len(),
                response.len()
            )));
        }
        println!(
            "FLOW observed_payload_bytes={} role=rpc_client",
            response.len()
        );
        exchanges = exchanges.checked_add(1).ok_or_else(|| {
            UStatus::fail_with_code(UCode::OutOfRange, "RPC exchange counter exhausted")
        })?;
        if options.send_count != 0 {
            return Ok(());
        }
        tokio::time::sleep(options.interval).await;
    }
}

async fn finish(
    task: tokio::task::JoinHandle<Result<Vec<u8>, UStatus>>,
    sent: Result<(), UStatus>,
) -> Result<Vec<u8>, UStatus> {
    if let Err(error) = sent {
        task.abort();
        let _ = task.await;
        return Err(error);
    }
    task.await.map_err(|error| {
        UStatus::fail_with_code(
            UCode::Internal,
            format!("response receive task failed: {error}"),
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn failed_send_cancels_and_reaps_pending_response_task() {
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<Result<Vec<u8>, UStatus>>().await
        });
        let error = finish(
            task,
            Err(UStatus::fail_with_code(UCode::Unavailable, "send rejected")),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), UCode::Unavailable);
        assert!(dropped.load(Ordering::SeqCst));
    }
}
