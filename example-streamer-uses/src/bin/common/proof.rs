// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Observational helpers; role construction and control flow remain in each binary.
#![allow(dead_code)]

use up_rust::{UMessage, UStatus, UTransport};

pub(crate) async fn send(
    transport: &(impl UTransport + ?Sized),
    message: UMessage,
) -> Result<(), UStatus> {
    #[cfg(feature = "flow-evidence")]
    let record = streamer_flow_evidence::Observation::message("tx", None, &message)?;
    transport.send(message).await?;
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    Ok(())
}

pub(crate) fn received(message: &UMessage) {
    #[cfg(feature = "flow-evidence")]
    match streamer_flow_evidence::Observation::message("rx", None, message) {
        Ok(record) => streamer_flow_evidence::Observation::emit(record),
        Err(error) => eprintln!("FLOW_EVIDENCE_ERROR {error}"),
    }
    #[cfg(not(feature = "flow-evidence"))]
    let _ = message;
}

#[cfg(feature = "selected-wire-common")]
pub(crate) fn received_frame(frame: &(impl up_rust::UFrameView + ?Sized)) {
    #[cfg(feature = "flow-evidence")]
    match streamer_flow_evidence::Observation::frame("rx", None, frame) {
        Ok(record) => streamer_flow_evidence::Observation::emit(record),
        Err(error) => eprintln!("FLOW_EVIDENCE_ERROR {error}"),
    }
    #[cfg(not(feature = "flow-evidence"))]
    let _ = frame;
}

#[cfg(feature = "selected-wire-common")]
pub(crate) async fn send_owned(
    transport: &(impl up_rust::UOwnedTransport + ?Sized),
    frame: up_rust::UOwnedFrame,
) -> Result<(), UStatus> {
    #[cfg(feature = "flow-evidence")]
    let record = streamer_flow_evidence::Observation::frame("tx", None, &frame)?;
    transport.send_owned(frame).await?;
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    Ok(())
}

#[cfg(feature = "selected-wire-common")]
pub(crate) fn send_prepared<T: configurable_streamer_wire_support::prepared::PreparedEndpoint>(
    transport: &configurable_streamer_wire_support::prepared::PreparedSession<T>,
    metadata: &up_rust::UFrameMetadata,
    payload: &[u8],
) -> Result<(), UStatus> {
    #[cfg(feature = "flow-evidence")]
    let record =
        streamer_flow_evidence::Observation::frame("tx", None, &SendView { metadata, payload })?;
    transport
        .send(up_rust::TxHeaderParts::from(metadata), Some(payload))
        .map_err(|error| UStatus::fail_with_code(up_rust::UCode::Internal, error.to_string()))?;
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    Ok(())
}

#[cfg(all(feature = "selected-wire-common", feature = "flow-evidence"))]
struct SendView<'a> {
    metadata: &'a up_rust::UFrameMetadata,
    payload: &'a [u8],
}
#[cfg(all(feature = "selected-wire-common", feature = "flow-evidence"))]
impl up_rust::UFrameView for SendView<'_> {
    type PayloadReader<'a>
        = std::io::Cursor<&'a [u8]>
    where
        Self: 'a;
    type PayloadSlices<'a>
        = std::option::IntoIter<&'a [u8]>
    where
        Self: 'a;
    fn metadata(&self) -> &up_rust::UFrameMetadata {
        self.metadata
    }
    fn payload_len(&self) -> usize {
        self.payload.len()
    }
    fn has_payload(&self) -> bool {
        true
    }
    fn payload_reader(&self) -> Self::PayloadReader<'_> {
        std::io::Cursor::new(self.payload)
    }
    fn payload_slices(&self) -> Self::PayloadSlices<'_> {
        Some(self.payload).into_iter()
    }
    fn try_contiguous_payload(&self) -> Option<&[u8]> {
        Some(self.payload)
    }
}
