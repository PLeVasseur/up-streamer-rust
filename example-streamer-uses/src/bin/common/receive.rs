// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Passive role readiness means registration has completed, not that polling will start.
#![allow(dead_code)]

use super::payloads::NativeContext;
use async_trait::async_trait;
use configurable_streamer_wire_support::prepared::{
    PreparedEndpoint, PreparedReceived, PreparedSession,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use up_rust::{
    PreparedTransportCore, UCode, UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus, UUri,
};

struct OwnedChannel(mpsc::UnboundedSender<UOwnedFrame>);

pub(crate) fn listener_ready() {
    println!("READY listener_registered");
}

pub(crate) fn zenoh_listener_ready() {
    println!("READY zenoh_listener_registered");
    listener_ready();
}

#[async_trait]
impl UOwnedListener for OwnedChannel {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        super::proof::received_frame(&frame);
        let _ = self.0.send(frame);
    }
}

pub(crate) type NativeFrame<T> =
    PreparedReceived<<<T as PreparedEndpoint>::Core as PreparedTransportCore>::Rx>;
pub(crate) struct NativeReception<T: PreparedEndpoint> {
    session: Arc<PreparedSession<T>>,
    inbox: Arc<up_rust::RegistrationInbox<<T::Core as PreparedTransportCore>::Rx>>,
    subscription: Option<<T::Core as PreparedTransportCore>::Subscription>,
    source: UUri,
    sink: Option<UUri>,
}
impl<T: PreparedEndpoint> NativeReception<T> {
    pub(crate) async fn receive(&mut self, timeout_ms: u64) -> Result<NativeFrame<T>, UStatus> {
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .ok_or_else(|| {
                UStatus::fail_with_code(UCode::InvalidArgument, "native receive deadline overflow")
            })?;
        loop {
            let raw = tokio::time::timeout_at(deadline, self.inbox.recv())
                .await
                .map_err(|_| {
                    UStatus::fail_with_code(
                        UCode::DeadlineExceeded,
                        "registered native receive timed out",
                    )
                })?
                .ok_or_else(|| {
                    UStatus::fail_with_code(UCode::Unavailable, "registered native receiver closed")
                })?;
            let frame = match self.session.admit(raw) {
                Ok(frame) => frame,
                Err(_) => continue,
            };
            let header = frame.header().map_err(|error| {
                UStatus::fail_with_code(UCode::InvalidArgument, error.to_string())
            })?;
            if !self.source.matches(&header.source)
                || !match (self.sink.as_ref(), header.sink.as_ref()) {
                    (None, None) => true,
                    (Some(filter), Some(actual)) => filter.matches(actual),
                    _ => false,
                }
            {
                continue;
            }
            let frame = self.session.decode_compatibility_frame(frame)?;
            super::proof::received_frame(&frame);
            super::payloads::verify_received_frame(self.session.as_ref(), &frame)?;
            return Ok(frame);
        }
    }
    pub(crate) async fn stop(mut self) -> Result<(), UStatus> {
        self.inbox.close();
        if let Some(subscription) = &mut self.subscription {
            self.session
                .core()
                .detach(subscription)
                .map_err(|error| UStatus::fail_with_code(UCode::Internal, error.to_string()))?;
        }
        self.subscription.take();
        Ok(())
    }
}
impl<T: PreparedEndpoint> Drop for NativeReception<T> {
    fn drop(&mut self) {
        self.inbox.close();
        if let Some(subscription) = &mut self.subscription {
            let _ = self.session.core().detach(subscription);
        }
    }
}
pub(crate) async fn register_native<T: PreparedEndpoint>(
    session: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
) -> Result<NativeReception<T>, UStatus> {
    let inbox = Arc::new(
        up_rust::RegistrationInbox::new(up_rust::RegistrationConfig {
            capacity: 4,
            overflow: up_rust::OverflowPolicy::Block {
                deadline: Duration::from_secs(5),
            },
        })
        .map_err(|error| UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string()))?,
    );
    let subscription = session
        .core()
        .attach(source, sink, inbox.clone())
        .await
        .map_err(|error| UStatus::fail_with_code(UCode::FailedPrecondition, error.to_string()))?;
    Ok(NativeReception {
        session: Arc::clone(session),
        inbox,
        subscription: Some(subscription),
        source: source.clone(),
        sink: sink.cloned(),
    })
}

pub(crate) async fn owned(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
) -> Result<UOwnedFrame, UStatus> {
    owned_with_ready(transport, source, sink, timeout_ms, native, || {
        println!("READY listener_registered");
    })
    .await
}

/// Receive through a registered listener, announcing endpoint-specific readiness
/// only after admission succeeds and removing the listener on receipt or timeout.
pub(crate) async fn owned_with_ready(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
    ready: impl FnOnce(),
) -> Result<UOwnedFrame, UStatus> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let listener: Arc<dyn UOwnedListener> = Arc::new(OwnedChannel(sender));
    transport
        .register_owned_listener(source, sink, listener.clone())
        .await?;
    ready();
    let result = receive(&mut receiver, timeout_ms).await;
    let cleanup = transport
        .unregister_owned_listener(source, sink, listener)
        .await;
    let frame = result?;
    cleanup?;
    if let Some(native) = native {
        native.verify_owned(frame.metadata(), frame.payload_bytes())?;
    }
    Ok(frame)
}

pub(crate) async fn owned_payload(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
) -> Result<Vec<u8>, UStatus> {
    owned(transport, source, sink, timeout_ms, native)
        .await
        .map(|frame| frame.payload_bytes().to_vec())
}

pub(crate) async fn zero_copy<T>(
    transport: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
) -> Result<NativeFrame<T>, UStatus>
where
    T: PreparedEndpoint,
{
    zero_copy_with_ready(transport, source, sink, timeout_ms, || {
        println!("READY listener_registered");
    })
    .await
}

/// Native counterpart to [`owned_with_ready`], retaining the received lease
/// through quiescent unregister and validating its representation before return.
pub(crate) async fn zero_copy_with_ready<T>(
    transport: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    ready: impl FnOnce(),
) -> Result<NativeFrame<T>, UStatus>
where
    T: PreparedEndpoint,
{
    let mut receiver = register_native(transport, source, sink).await?;
    ready();
    let result = receiver.receive(timeout_ms).await;
    let cleanup = receiver.stop().await;
    let frame = result?;
    cleanup?;
    super::payloads::verify_received_frame(transport.as_ref(), &frame)?;
    Ok(frame)
}

pub(crate) async fn zero_copy_payload<T>(
    transport: &Arc<PreparedSession<T>>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus>
where
    T: PreparedEndpoint,
{
    zero_copy(transport, source, sink, timeout_ms)
        .await
        .and_then(|frame| super::payloads::copy_payload_bytes(&frame))
}

async fn receive<T>(
    receiver: &mut mpsc::UnboundedReceiver<T>,
    timeout_ms: u64,
) -> Result<T, UStatus> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), receiver.recv())
        .await
        .map_err(|_| {
            UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for registered receiver",
            )
        })?
        .ok_or_else(|| {
            UStatus::fail_with_code(UCode::Unavailable, "registered receiver channel closed")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use up_rust::{PayloadEncoding, UFrameMetadata, UOwnedTransportImpl};

    struct ImmediateDelivery {
        removed: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl UOwnedTransportImpl for ImmediateDelivery {
        async fn send_validated_owned(&self, _: UOwnedFrame) -> Result<(), UStatus> {
            unreachable!("passive test never sends")
        }

        async fn register_validated_owned_listener(
            &self,
            source: &UUri,
            _: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            let metadata = UFrameMetadata::publish(source.clone())
                .with_payload_encoding(PayloadEncoding::RAW)
                .build()
                .unwrap();
            listener
                .on_receive_owned(
                    UOwnedFrame::with_payload(metadata, b"first frame".to_vec()).unwrap(),
                )
                .await;
            Ok(())
        }

        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.removed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn frame_arriving_during_registration_is_retained_and_listener_removed() {
        let removed = Arc::new(AtomicUsize::new(0));
        let transport: Arc<dyn UOwnedTransport> = Arc::new(ImmediateDelivery {
            removed: removed.clone(),
        });
        let source = UUri::try_from_parts("ready-test", 0x5BA0, 1, 0x8001).unwrap();
        let frame = owned(&transport, &source, None, 1000, None).await.unwrap();
        assert_eq!(frame.payload_bytes(), b"first frame");
        assert_eq!(removed.load(Ordering::SeqCst), 1);
    }

    struct RejectedRegistration;

    #[async_trait]
    impl UOwnedTransportImpl for RejectedRegistration {
        async fn send_validated_owned(&self, _: UOwnedFrame) -> Result<(), UStatus> {
            unreachable!("passive test never sends")
        }

        async fn register_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            Err(UStatus::fail_with_code(
                UCode::Unavailable,
                "admission rejected",
            ))
        }

        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            panic!("failed registration must not be treated as admitted")
        }
    }

    #[tokio::test]
    async fn rejected_registration_never_announces_endpoint_readiness() {
        let transport: Arc<dyn UOwnedTransport> = Arc::new(RejectedRegistration);
        let source = UUri::try_from_parts("ready-test", 0x5BA0, 1, 0x8001).unwrap();
        let error = owned_with_ready(&transport, &source, None, 1000, None, || {
            panic!("readiness must follow successful registration");
        })
        .await
        .unwrap_err();
        assert_eq!(error.code(), UCode::Unavailable);
    }
}
