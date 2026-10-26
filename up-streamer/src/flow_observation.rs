// Copyright (c) 2026 Contributors to the Eclipse Foundation
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

#[cfg(feature = "owned-frame-transport")]
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

pub(crate) fn emit(record: Record) {
    #[cfg(feature = "flow-evidence")]
    streamer_flow_evidence::Observation::emit(record);
    #[cfg(not(feature = "flow-evidence"))]
    let _ = record;
}

#[cfg(feature = "flow-evidence")]
fn checked(result: Result<Record, up_rust::UStatus>) -> Record {
    result.unwrap_or_else(|error| {
        eprintln!("FLOW_EVIDENCE_ERROR {error}");
        None
    })
}

#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "flow-evidence"
))]
pub(crate) fn prepared<F: up_rust::Frame>(
    stage: &str,
    route: &str,
    value: &up_rust::HeaderLease<F>,
) -> Record {
    match value.header() {
        Ok(header) => checked(streamer_flow_evidence::Observation::header(
            stage,
            Some(route),
            &header,
            value.payload().map(|payload| payload.bytes()),
        )),
        Err(error) => {
            eprintln!("FLOW_EVIDENCE_ERROR {error}");
            None
        }
    }
}

#[cfg(feature = "experimental-copy-minimized-routing")]
pub(crate) fn prepared_copy_observer(
    name: &str,
    authority: &str,
) -> Option<std::sync::Arc<dyn crate::PreparedForwardObserver>> {
    #[cfg(feature = "flow-evidence")]
    {
        std::env::var("UPROTOCOL_FLOW_EVIDENCE_RUN")
            .ok()
            .map(|run| {
                std::sync::Arc::new(NativeCopyObserver {
                    run,
                    egress: format!("native:{name}@{authority}"),
                }) as std::sync::Arc<dyn crate::PreparedForwardObserver>
            })
    }
    #[cfg(not(feature = "flow-evidence"))]
    {
        let _ = (name, authority);
        None
    }
}
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "flow-evidence"
))]
struct NativeCopyObserver {
    run: String,
    egress: String,
}
#[cfg(all(
    feature = "experimental-copy-minimized-routing",
    feature = "flow-evidence"
))]
impl crate::PreparedForwardObserver for NativeCopyObserver {
    fn ingress(&self, _: &up_rust::RxHeader<'_>, _: Option<&[u8]>) {
        // Actual route ingress is already observed by its forwarding handler.
    }
    fn copied_metadata(&self, _: &up_rust::UUID, _: &[u8]) {
        // This observer claims payload-copy evidence only. The existing custom
        // observer API separately exposes actual initialized metadata bytes.
    }
    fn copied_payload(&self, id: &up_rust::UUID, payload: Option<&[u8]>) {
        streamer_flow_evidence::PayloadCopyObservation::capture(
            &self.run,
            &self.egress,
            id,
            payload,
        )
        .emit();
    }
    fn completed(&self, id: &up_rust::UUID, result: Result<(), up_rust::SendError>) {
        eprintln!(
            "FLOW_NATIVE_FORWARD_COMPLETED {}",
            serde_json::json!({
                "schema": 1, "run": self.run, "egress": self.egress,
                "id": id.to_hyphenated_string(), "success": result.is_ok(),
                "error": result.err().map(|error| format!("{error:?}")),
            })
        );
    }
}
