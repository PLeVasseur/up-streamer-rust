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
    PayloadEncoding, StableContainerPayloadInfo, UCode, UFrameView, UStatus, UTxBuffer, UTxLoanSpec,
};

/// Options for feature-gated copy-minimized Streamer routes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyMinimizedRouteOptions {
    /// Requested egress payload alignment for transmit loans.
    pub payload_alignment: usize,
}

impl Default for CopyMinimizedRouteOptions {
    fn default() -> Self {
        Self {
            payload_alignment: 1,
        }
    }
}

pub(crate) fn loan_spec_for_copy_minimized(
    frame: &(impl UFrameView + ?Sized),
    options: CopyMinimizedRouteOptions,
) -> Result<UTxLoanSpec, UStatus> {
    validate_stable_container_copy_minimized(frame, options.payload_alignment)?;

    let metadata = frame.metadata().clone();
    if !frame.has_payload() {
        return UTxLoanSpec::no_payload(metadata);
    }
    if frame.payload_len() == 0 {
        return UTxLoanSpec::present_empty_payload(metadata);
    }

    UTxLoanSpec::payload(metadata, frame.payload_len(), options.payload_alignment)
}

pub(crate) fn copy_frame_payload_to_tx(
    frame: &(impl UFrameView + ?Sized),
    tx: &mut (impl UTxBuffer + ?Sized),
) -> Result<(usize, usize), UStatus> {
    let payload_len = frame.payload_len();
    let target = tx.payload_mut();
    if target.len() != payload_len {
        return Err(UStatus::fail_with_code(
            UCode::Internal,
            format!(
                "copy-minimized egress loan length {} does not match ingress payload length {payload_len}",
                target.len()
            ),
        ));
    }

    let mut copied = 0_usize;
    let mut slice_count = 0_usize;
    for slice in frame.payload_slices() {
        let end = copied.checked_add(slice.len()).ok_or_else(|| {
            UStatus::fail_with_code(
                UCode::Internal,
                "copy-minimized payload slice lengths overflow usize",
            )
        })?;
        if end > target.len() {
            return Err(UStatus::fail_with_code(
                UCode::Internal,
                "copy-minimized payload slices exceed egress loan length",
            ));
        }
        target[copied..end].copy_from_slice(slice);
        copied = end;
        slice_count += 1;
    }

    if copied != target.len() {
        return Err(UStatus::fail_with_code(
            UCode::Internal,
            format!(
                "copy-minimized payload slices yielded {copied} bytes but egress loan length is {}",
                target.len()
            ),
        ));
    }

    Ok((copied, slice_count))
}

fn validate_stable_container_copy_minimized(
    frame: &(impl UFrameView + ?Sized),
    alignment: usize,
) -> Result<(), UStatus> {
    let Some(encoding) = frame.metadata().payload_encoding() else {
        return Ok(());
    };
    if !is_stable_container_encoding(encoding) {
        return Ok(());
    }

    let info = StableContainerPayloadInfo::parse(encoding).map_err(UStatus::from)?;
    if frame.payload_len() != info.size {
        return Err(UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!(
                "stable-container payload length {} does not match advertised size {}",
                frame.payload_len(),
                info.size
            ),
        ));
    }
    if alignment < info.alignment {
        return Err(UStatus::fail_with_code(
            UCode::InvalidArgument,
            format!(
                "copy-minimized stable-container route requires egress alignment at least {}, configured {alignment}",
                info.alignment
            ),
        ));
    }

    Ok(())
}

fn is_stable_container_encoding(encoding: &PayloadEncoding) -> bool {
    encoding
        .custom_identity()
        .is_some_and(|(id, _)| id == StableContainerPayloadInfo::ENCODING_ID)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use up_rust::{
        try_project_umessage_to_frame_metadata, PayloadEncoding, UMessageBuilder, UPayloadFormat,
        UUri, UVecRxLease, UVecTxBuffer,
    };

    fn payload_frame(payload: &'static [u8]) -> UVecRxLease {
        let message = UMessageBuilder::publish(
            UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
        )
        .build_with_payload(Bytes::from_static(payload), UPayloadFormat::Protobuf)
        .expect("message");
        let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
        UVecRxLease::new(metadata, Some(payload.to_vec())).expect("frame")
    }

    fn stable_payload_frame(
        payload_len: usize,
        advertised_size: usize,
        alignment: usize,
    ) -> UVecRxLease {
        let message = UMessageBuilder::publish(
            UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
        )
        .build_with_payload(Bytes::from(vec![0; payload_len]), UPayloadFormat::Raw)
        .expect("message");
        let metadata = try_project_umessage_to_frame_metadata(&message)
            .expect("metadata")
            .into_attributes();
        let encoding = PayloadEncoding::custom(
            StableContainerPayloadInfo::ENCODING_ID,
            format!(
                "application/vnd.uprotocol.stable-container; type=test.Stable; variant=fixed; size={advertised_size}; align={alignment}"
            ),
        )
        .expect("stable-container encoding");
        let metadata = up_rust::UFrameMetadata::new(metadata, Some(encoding)).expect("metadata");
        UVecRxLease::new(metadata, Some(vec![0; payload_len])).expect("frame")
    }

    #[test]
    fn copies_ordered_payload_slices_into_tx_loan() {
        let frame = payload_frame(b"payload");
        let mut tx = UVecTxBuffer::new(frame.metadata().clone(), frame.payload_len());

        let diagnostics = copy_frame_payload_to_tx(&frame, &mut tx).expect("copy succeeds");

        assert_eq!(diagnostics, (7, 1));
        assert_eq!(tx.payload(), b"payload");
    }

    #[test]
    fn stable_container_alignment_must_be_satisfied() {
        let frame = stable_payload_frame(8, 8, 8);

        let error = loan_spec_for_copy_minimized(
            &frame,
            CopyMinimizedRouteOptions {
                payload_alignment: 4,
            },
        )
        .expect_err("alignment should be rejected");

        assert_eq!(error.get_code(), UCode::InvalidArgument);

        let spec = loan_spec_for_copy_minimized(
            &frame,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .expect("alignment should pass");
        assert_eq!(spec.payload_len(), 8);
        assert_eq!(spec.payload_alignment(), 8);
    }

    #[test]
    fn stable_container_payload_len_must_match_metadata() {
        let frame = stable_payload_frame(4, 8, 1);

        let error = loan_spec_for_copy_minimized(&frame, CopyMinimizedRouteOptions::default())
            .expect_err("length should be rejected");

        assert_eq!(error.get_code(), UCode::InvalidArgument);
    }
}
