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
pub(crate) async fn send_loan<T: up_rust::UZeroCopyTransport + ?Sized>(
    transport: &T,
    loan: T::Tx,
) -> Result<(), UStatus> {
    #[cfg(feature = "flow-evidence")]
    let record = streamer_flow_evidence::Observation::loan("tx", &loan)?;
    transport.send_validated_zero_copy(loan).await?;
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    Ok(())
}
