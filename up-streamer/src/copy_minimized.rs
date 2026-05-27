/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use up_rust::{
    copy_loaned_frame_payload_to_tx, zero_copy::UZeroCopyTransport, LoanedFrame, UStatus,
};

/// Sends a loaned frame through a zero-copy transport with one payload copy.
///
/// This experimental helper reserves an egress transmit loan using the loaned
/// frame's metadata and visible payload length, then copies ordered ingress
/// payload slices directly into that loan. It avoids materializing an
/// intermediate [`up_rust::UOwnedFrame`] payload buffer, but it is still a
/// payload-byte copy into the egress transport and must not be described as
/// zero-copy-preserving forwarding.
pub async fn send_loaned_frame_copy_minimized<T>(
    transport: &T,
    frame: &(impl LoanedFrame + ?Sized),
    alignment: usize,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + ?Sized,
{
    let mut tx = transport
        .reserve(frame.metadata().clone(), frame.payload_len(), alignment)
        .await?;
    copy_loaned_frame_payload_to_tx(frame, &mut tx).map_err(UStatus::from)?;
    transport.send_zero_copy(tx).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Mutex;
    use up_rust::{
        zero_copy::{UVecTxBuffer, UZeroCopyTransport},
        UFrameBuilder, UOwnedFrame, UStatus, UUri,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingZeroCopyTransport {
        sent: Mutex<Vec<UOwnedFrame>>,
    }

    #[async_trait]
    impl UZeroCopyTransport for RecordingZeroCopyTransport {
        type Tx = UVecTxBuffer;
        type Rx = UOwnedFrame;

        async fn reserve(
            &self,
            metadata: up_rust::UFrameMetadata,
            payload_len: usize,
            alignment: usize,
        ) -> Result<Self::Tx, UStatus> {
            UVecTxBuffer::with_alignment(metadata, payload_len, alignment).map_err(UStatus::from)
        }

        async fn send_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
            self.sent.lock().await.push(buffer.into_frame());
            Ok(())
        }
    }

    #[tokio::test]
    async fn sends_loaned_frame_without_intermediate_owned_payload_buffer() {
        let transport = Arc::new(RecordingZeroCopyTransport::default());
        let topic = UUri::try_from_parts("vehicle", 0x4210, 1, 0x9000).unwrap();
        let frame = UFrameBuilder::publish(topic)
            .build_with_raw_payload(b"payload".as_slice())
            .unwrap();

        send_loaned_frame_copy_minimized(transport.as_ref(), &frame, 1)
            .await
            .unwrap();

        let sent = transport.sent.lock().await;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].metadata(), frame.metadata());
        assert_eq!(sent[0].payload_bytes(), b"payload");
    }
}
