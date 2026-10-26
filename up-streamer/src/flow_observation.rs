// SPDX-License-Identifier: Apache-2.0
//! Optional test observations at actual route handoff boundaries.

#[cfg(feature = "flow-evidence")]
pub(crate) type Record = Option<streamer_flow_evidence::Observation>;
#[cfg(not(feature = "flow-evidence"))]
pub(crate) struct Record;

pub(crate) fn message(stage: &str, route: &str, value: &up_rust::UMessage) -> Record {
    #[cfg(feature = "flow-evidence")]
    {
        checked(streamer_flow_evidence::Observation::message(
            stage,
            Some(route),
            value,
        ))
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (stage, route, value);
        Record
    }
}

#[cfg(any(
    feature = "owned-frame-transport",
    feature = "experimental-copy-minimized-routing"
))]
pub(crate) fn frame(
    stage: &str,
    route: &str,
    value: &(impl up_rust::UFrameView + ?Sized),
) -> Record {
    #[cfg(feature = "flow-evidence")]
    {
        checked(streamer_flow_evidence::Observation::frame(
            stage,
            Some(route),
            value,
        ))
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (stage, route, value);
        Record
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
pub(crate) fn loan(route: &str, value: &impl up_rust::UTxBuffer) -> Record {
    #[cfg(feature = "flow-evidence")]
    {
        let mut record = checked(streamer_flow_evidence::Observation::loan("egress", value));
        if let Some(record) = &mut record {
            record.route = Some(route.to_owned());
        }
        record
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (route, value);
        Record
    }
}

pub(crate) fn emit(record: Record) {
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    #[cfg(not(feature = "flow-evidence"))]
    let _ = record;
}

/// Record actual payload movement separately from loan provenance and hashes.
#[cfg(feature = "experimental-copy-minimized-routing")]
pub(crate) fn copied(route: &str, metadata: &up_rust::UFrameMetadata, bytes: usize, slices: usize) {
    #[cfg(feature = "flow-evidence")]
    if let (Ok(run), Ok(actor)) = (
        std::env::var("UPROTOCOL_FLOW_EVIDENCE_RUN"),
        std::env::var("UPROTOCOL_FLOW_EVIDENCE_ACTOR"),
    ) {
        println!(
            "FLOW_PAYLOAD_COPY {}",
            serde_json::json!({
                "schema":1, "run":run, "actor":actor, "route":route,
                "id":metadata.id().to_hyphenated_string(), "copied_payload_bytes":bytes,
                "payload_slice_copies":slices, "boundary":"ingress_to_egress_loan",
            })
        );
    }
    #[cfg(not(feature = "flow-evidence"))]
    let _ = (route, metadata, bytes, slices);
}

#[cfg(feature = "flow-evidence")]
fn checked(result: Result<Record, up_rust::UStatus>) -> Record {
    result.unwrap_or_else(|error| {
        eprintln!("FLOW_EVIDENCE_ERROR {error}");
        None
    })
}
