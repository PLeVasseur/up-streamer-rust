// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Selected-wire RPC exchange flow with explicitly configured receive/send policy.

use super::{flow, payloads::NativeContext};
use configurable_streamer_wire_support::prepared::{PreparedEndpoint, PreparedSession};
use std::{sync::Arc, time::Duration};
use up_rust::{UCode, UFrameMetadata, UFrameView, UOwnedTransport, UStatus, UUri};

#[derive(Clone)]
pub struct ClientOptions {
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

pub async fn owned(
    transport: &Arc<dyn UOwnedTransport>,
    options: &ClientOptions,
    request: impl Fn(u32) -> Result<(UFrameMetadata, Vec<u8>), UStatus>,
) -> Result<(), UStatus> {
    let sends = options.sends()?;
    let mut receiver = super::receive::register_owned(
        transport,
        &options.response_source,
        Some(&options.response_sink),
    )
    .await?;
    let result = async {
        super::startup::wait_for_start().await?;
        let mut exchange = 0u64;
        loop {
            let sequence =
                u32::try_from(exchange.saturating_add(1).min(u64::from(u32::MAX))).unwrap();
            let (metadata, payload) = request(sequence)?;
            // One registration spans every exchange. Arm exact correlation before
            // sending so an immediate reply cannot race creation of its waiter.
            receiver.expect_response(metadata.id());
            let deadline = tokio::time::Instant::now()
                .checked_add(Duration::from_millis(options.timeout_ms))
                .ok_or_else(|| flow::invalid("response deadline overflow"))?;
            // Preserve the existing per-exchange pause; it no longer establishes
            // registration readiness and does not renew the response deadline.
            tokio::time::sleep(options.receive_settle).await;
            flow::send_owned_repeated(transport, metadata, &payload, sends).await?;
            let response = receiver.receive_until(deadline).await?;
            if let Some(native) = &options.native {
                native.verify_owned(response.metadata(), response.payload_bytes())?;
            }
            if response.payload_bytes() != payload {
                return Err(flow::invalid("correlated response payload mismatch"));
            }
            println!(
                "FLOW observed_payload_bytes={} role=rpc_client",
                response.payload_bytes().len()
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

pub async fn native<T>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    use up_rust::{PayloadEncoding, UOwnedFrame, UOwnedListener, UOwnedTransportImpl, UUID};

    struct Loopback {
        listener: Mutex<Option<Arc<dyn UOwnedListener>>>,
        registrations: AtomicUsize,
        removals: AtomicUsize,
        sends: AtomicUsize,
        fail_on: usize,
    }

    #[async_trait::async_trait]
    impl UOwnedTransportImpl for Loopback {
        async fn register_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.registrations.fetch_add(1, Ordering::SeqCst);
            *self.listener.lock().unwrap() = Some(listener);
            Ok(())
        }
        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.listener.lock().unwrap().take();
            self.removals.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn send_validated_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
            let listener = self
                .listener
                .lock()
                .unwrap()
                .clone()
                .expect("response registration precedes send");
            let send = self.sends.fetch_add(1, Ordering::SeqCst) + 1;
            if send == self.fail_on {
                return Err(UStatus::fail_with_code(UCode::Unavailable, "send rejected"));
            }
            let reply = |id, bytes: Vec<u8>| {
                UOwnedFrame::with_payload(
                    UFrameMetadata::response(
                        frame.metadata().sink().unwrap().clone(),
                        frame.metadata().source().clone(),
                        id,
                    )
                    .with_payload_encoding(PayloadEncoding::RAW)
                    .build()
                    .unwrap(),
                    bytes,
                )
                .unwrap()
            };
            // Synchronous delivery inside send proves setup and correlation are
            // already armed. An unrelated ID and a duplicate cannot win the call.
            listener
                .on_receive_owned(reply(UUID::build(), b"unrelated".to_vec()))
                .await;
            listener
                .on_receive_owned(reply(
                    frame.metadata().id().clone(),
                    frame.payload_bytes().to_vec(),
                ))
                .await;
            listener
                .on_receive_owned(reply(frame.metadata().id().clone(), b"duplicate".to_vec()))
                .await;
            Ok(())
        }
    }

    async fn run(send_count: u64, fail_on: usize) -> (Result<(), UStatus>, Arc<Loopback>) {
        let raw = Arc::new(Loopback {
            listener: Mutex::new(None),
            registrations: AtomicUsize::new(0),
            removals: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
            fail_on,
        });
        let transport: Arc<dyn UOwnedTransport> = raw.clone();
        let method = UUri::try_from("//rpc-role/2/1/1").unwrap();
        let reply = UUri::try_from("//rpc-role/1/1/0").unwrap();
        let options = ClientOptions {
            response_source: method.clone(),
            response_sink: reply.clone(),
            timeout_ms: 1000,
            send_count,
            interval: Duration::ZERO,
            receive_settle: Duration::ZERO,
            alignment: 8,
            native: None,
        };
        let result = owned(&transport, &options, |sequence| {
            Ok((
                UFrameMetadata::request(method.clone(), reply.clone(), Duration::from_secs(1))
                    .with_payload_encoding(PayloadEncoding::RAW)
                    .build()
                    .unwrap(),
                sequence.to_le_bytes().to_vec(),
            ))
        })
        .await;
        (result, raw)
    }

    #[tokio::test]
    async fn owned_rpc_registers_before_immediate_reply_and_checks_correlation() {
        let (result, transport) = run(1, 0).await;
        result.unwrap();
        assert_eq!(transport.registrations.load(Ordering::SeqCst), 1);
        assert_eq!(transport.removals.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_owned_send_closes_and_reaps_its_response_registration() {
        let (result, transport) = run(1, 1).await;
        assert_eq!(result.unwrap_err().code(), UCode::Unavailable);
        assert_eq!(transport.removals.load(Ordering::SeqCst), 1);
        assert!(transport.listener.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn owned_rpc_reuses_one_registration_across_exchanges() {
        let (result, transport) = run(0, 3).await;
        assert_eq!(result.unwrap_err().code(), UCode::Unavailable);
        assert_eq!(transport.sends.load(Ordering::SeqCst), 3);
        assert_eq!(transport.registrations.load(Ordering::SeqCst), 1);
        assert_eq!(transport.removals.load(Ordering::SeqCst), 1);
    }
}
