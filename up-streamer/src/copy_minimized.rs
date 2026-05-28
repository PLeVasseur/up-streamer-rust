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
    copy_loaned_frame_payload_to_tx, zero_copy::UZeroCopyTransport, LoanedFrame, PayloadLayout,
    StableContainerPayloadInfo, UCode, UStatus, UTxLoanSpec,
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
    let spec = loan_spec_for_copy_minimized(frame, alignment)?;
    let mut tx = transport.loan_tx(spec).await?;
    copy_loaned_frame_payload_to_tx(frame, &mut tx).map_err(UStatus::from)?;
    transport.send_zero_copy(tx).await
}

pub(crate) fn loan_spec_for_copy_minimized(
    frame: &(impl LoanedFrame + ?Sized),
    alignment: usize,
) -> Result<UTxLoanSpec, UStatus> {
    validate_stable_container_copy_minimized(frame, alignment)?;

    let metadata = frame.metadata().clone();
    if !frame.has_payload() {
        return UTxLoanSpec::no_payload(metadata);
    }
    if frame.payload_len() == 0 {
        return UTxLoanSpec::present_empty_payload(metadata);
    }

    let layout = PayloadLayout::new(frame.payload_len(), alignment).map_err(UStatus::from)?;
    UTxLoanSpec::payload(metadata, layout)
}

fn validate_stable_container_copy_minimized(
    frame: &(impl LoanedFrame + ?Sized),
    alignment: usize,
) -> Result<(), UStatus> {
    let Some(encoding) = frame.metadata().encoding() else {
        return Ok(());
    };
    let Some(custom) = encoding.custom_encoding() else {
        return Ok(());
    };
    if custom.id() != StableContainerPayloadInfo::ENCODING_ID {
        return Ok(());
    }

    let info = StableContainerPayloadInfo::parse(encoding).map_err(|error| {
        UStatus::fail_with_code(
            UCode::INVALID_ARGUMENT,
            format!("invalid stable-container metadata on copy-minimized route: {error}"),
        )
    })?;
    if frame.payload_len() != info.size {
        return Err(UStatus::fail_with_code(
            UCode::INVALID_ARGUMENT,
            format!(
                "stable-container payload length {} does not match advertised size {}",
                frame.payload_len(),
                info.size
            ),
        ));
    }
    if alignment < info.alignment {
        return Err(UStatus::fail_with_code(
            UCode::INVALID_ARGUMENT,
            format!(
                "copy-minimized stable-container route requires egress alignment at least {}, configured {}",
                info.alignment, alignment
            ),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Mutex;
    use up_rust::{
        zero_copy::{UVecTxBuffer, UZeroCopyTransport},
        UFrameBuilder, UOwnedFrame, UStatus, UTxLoanSpec, UUri,
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

        async fn loan_tx(&self, spec: UTxLoanSpec) -> Result<Self::Tx, UStatus> {
            UVecTxBuffer::with_alignment(
                spec.metadata().clone(),
                spec.payload_len(),
                spec.payload_alignment(),
            )
            .map_err(UStatus::from)
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
