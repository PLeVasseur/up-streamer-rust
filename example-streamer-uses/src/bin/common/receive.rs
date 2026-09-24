// SPDX-License-Identifier: Apache-2.0
//! Passive role readiness means registration has completed, not that polling will start.
#![allow(dead_code)]

use super::payloads::NativeContext;
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use up_rust::{
    UCode, UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus, UUri, UZeroCopyListener,
    UZeroCopyRxLease, UZeroCopyTransport,
};

struct OwnedChannel(mpsc::UnboundedSender<UOwnedFrame>);

#[async_trait]
impl UOwnedListener for OwnedChannel {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        let _ = self.0.send(frame);
    }
}

struct LoanChannel<Rx>(mpsc::UnboundedSender<Rx>);

#[async_trait]
impl<Rx: UZeroCopyRxLease + Send + 'static> UZeroCopyListener<Rx> for LoanChannel<Rx> {
    async fn on_receive_zero_copy(&self, frame: Rx) {
        let _ = self.0.send(frame);
    }
}

pub(crate) async fn owned(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
) -> Result<UOwnedFrame, UStatus> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let listener: Arc<dyn UOwnedListener> = Arc::new(OwnedChannel(sender));
    transport
        .register_owned_listener(source, sink, listener.clone())
        .await?;
    println!("READY listener_registered");
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
    transport: &Arc<T>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
) -> Result<T::Rx, UStatus>
where
    T: UZeroCopyTransport + up_rust::UHasWire + Send + Sync + 'static,
    T::Rx: UZeroCopyRxLease + Send + 'static,
{
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let listener: Arc<dyn UZeroCopyListener<T::Rx>> = Arc::new(LoanChannel(sender));
    transport
        .register_validated_zero_copy_listener(source, sink, listener.clone())
        .await?;
    println!("READY listener_registered");
    let result = receive(&mut receiver, timeout_ms).await;
    let cleanup = transport
        .unregister_validated_zero_copy_listener(source, sink, listener)
        .await;
    let frame = result?;
    cleanup?;
    super::payloads::verify_received_frame(transport.as_ref(), &frame)?;
    Ok(frame)
}

pub(crate) async fn zero_copy_payload<T>(
    transport: &Arc<T>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
) -> Result<Vec<u8>, UStatus>
where
    T: UZeroCopyTransport + up_rust::UHasWire + Send + Sync + 'static,
    T::Rx: UZeroCopyRxLease + Send + 'static,
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
}
