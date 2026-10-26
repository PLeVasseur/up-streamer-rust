// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Observational helpers for shared role flows and explicit endpoint setup.

use up_rust::{UMessage, UStatus, UTransport};

pub async fn send(
    transport: &(impl UTransport + ?Sized),
    message: UMessage,
) -> Result<(), UStatus> {
    #[cfg(all(feature = "flow-evidence", feature = "selected-wire-common"))]
    let message = if super::cloudevents::enabled() {
        super::cloudevents::message(message, "tx")?
    } else {
        message
    };
    #[cfg(feature = "flow-evidence")]
    let record = streamer_flow_evidence::Observation::message("tx", None, &message)?;
    transport.send(message).await?;
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    Ok(())
}

pub fn received(message: &UMessage) {
    #[cfg(all(feature = "flow-evidence", feature = "selected-wire-common"))]
    if super::cloudevents::enabled() {
        if let Err(error) = super::cloudevents::message(message.clone(), "rx") {
            eprintln!("CLOUDEVENT_ERROR {error}");
        }
    }
    #[cfg(feature = "flow-evidence")]
    match streamer_flow_evidence::Observation::message("rx", None, message) {
        Ok(record) => streamer_flow_evidence::Observation::emit(record),
        Err(error) => eprintln!("FLOW_EVIDENCE_ERROR {error}"),
    }
    #[cfg(not(feature = "flow-evidence"))]
    let _ = message;
}

#[cfg(feature = "selected-wire-common")]
pub fn received_frame(frame: &(impl up_rust::UFrameView + ?Sized)) {
    #[cfg(feature = "flow-evidence")]
    if super::cloudevents::enabled() {
        if let Err(error) = super::cloudevents::received_frame(frame) {
            eprintln!("CLOUDEVENT_ERROR {error}");
        }
    }
    #[cfg(feature = "flow-evidence")]
    match streamer_flow_evidence::Observation::frame("rx", None, frame) {
        Ok(record) => streamer_flow_evidence::Observation::emit(record),
        Err(error) => eprintln!("FLOW_EVIDENCE_ERROR {error}"),
    }
    #[cfg(not(feature = "flow-evidence"))]
    let _ = frame;
}

#[cfg(feature = "selected-wire-common")]
pub async fn send_owned(
    transport: &(impl up_rust::UOwnedTransport + ?Sized),
    frame: up_rust::UOwnedFrame,
) -> Result<(), UStatus> {
    #[cfg(feature = "flow-evidence")]
    let frame = if super::cloudevents::enabled() {
        let (metadata, payload) =
            super::cloudevents::frame(frame.metadata(), frame.payload().cloned(), "tx")?;
        up_rust::UOwnedFrame::new(metadata, payload).map_err(|error| {
            UStatus::fail_with_code(up_rust::UCode::InvalidArgument, error.to_string())
        })?
    } else {
        frame
    };
    #[cfg(feature = "flow-evidence")]
    let record = streamer_flow_evidence::Observation::frame("tx", None, &frame)?;
    transport.send_owned(frame).await?;
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    Ok(())
}

#[cfg(feature = "selected-wire-common")]
pub fn send_prepared<T: configurable_streamer_wire_support::prepared::PreparedEndpoint>(
    transport: &configurable_streamer_wire_support::prepared::PreparedSession<T>,
    metadata: &up_rust::UFrameMetadata,
    payload: &[u8],
) -> Result<(), UStatus> {
    #[cfg(feature = "flow-evidence")]
    let converted = if super::cloudevents::enabled() {
        Some(super::cloudevents::frame(
            metadata,
            Some(bytes::Bytes::copy_from_slice(payload)),
            "tx",
        )?)
    } else {
        None
    };
    #[cfg(feature = "flow-evidence")]
    let (metadata, payload) = converted
        .as_ref()
        .map_or((metadata, payload), |(metadata, payload)| {
            (metadata, payload.as_deref().unwrap_or_default())
        });
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
