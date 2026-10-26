// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Opt-in observational evidence for integration tests. Never part of a wire format.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use up_rust::{UCode, UFrameMetadata, UFrameView, UMessage, UStatus, UTxBuffer};

/// Prefix before each JSON observation emitted to standard output.
pub const MARKER: &str = "FLOW_MESSAGE_EVIDENCE ";

/// A successful API operation or actual bridge handoff, scoped to one run/row.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// Observation schema version; capture currently emits version 1.
    pub schema: u8,
    /// Explicit run identity from `UPROTOCOL_FLOW_EVIDENCE_RUN`.
    pub run: String,
    /// Explicit process/role label from `UPROTOCOL_FLOW_EVIDENCE_ACTOR`.
    pub actor: String,
    /// Observed operation boundary, such as `tx`, `rx`, `ingress` or `egress`.
    pub stage: String,
    /// Route label for a bridge handoff, when supplied by the caller.
    pub route: Option<String>,
    /// Message UUID in hyphenated form.
    pub id: String,
    /// Correlated request UUID, if carried by the message metadata.
    pub reqid: Option<String>,
    /// Debug name of the SDK message kind.
    pub kind: String,
    /// Source URI rendered without an explicit URI scheme.
    pub source: String,
    /// Sink URI rendered without an explicit URI scheme, if present.
    pub sink: Option<String>,
    /// Explicit payload-encoding identifier, if a payload is present.
    pub encoding: Option<u32>,
    /// Payload presence, kept distinct from a present payload of length zero.
    pub present: bool,
    /// Declared visible payload length, checked against all supplied slices.
    pub length: usize,
    /// Lowercase hexadecimal SHA-256 of the concatenated visible payload slices.
    pub sha256: String,
}

impl Observation {
    /// Captures metadata and hashes payload slices when run evidence is enabled.
    ///
    /// Returns `None` without reading payload slices when the run variable is not
    /// available as UTF-8. An enabled run requires an actor; inconsistent slice
    /// lengths or arithmetic overflow return `InvalidArgument`.
    pub fn capture<'a>(
        stage: &str,
        route: Option<&str>,
        metadata: &UFrameMetadata,
        length: usize,
        slices: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Option<Self>, UStatus> {
        let Ok(run) = std::env::var("UPROTOCOL_FLOW_EVIDENCE_RUN") else {
            return Ok(None);
        };
        let actor = std::env::var("UPROTOCOL_FLOW_EVIDENCE_ACTOR")
            .map_err(|_| invalid("flow evidence requires an explicit actor"))?;
        let mut hash = Sha256::new();
        let mut observed = 0usize;
        for slice in slices {
            observed = observed
                .checked_add(slice.len())
                .ok_or_else(|| invalid("payload length overflow"))?;
            if observed > length {
                return Err(invalid("payload slices exceed declared length"));
            }
            hash.update(slice);
        }
        if observed != length {
            return Err(invalid("payload slices do not match declared length"));
        }
        Ok(Some(Self {
            schema: 1,
            run,
            actor,
            stage: stage.into(),
            route: route.map(str::to_owned),
            id: metadata.id().to_hyphenated_string(),
            reqid: metadata.reqid().map(|value| value.to_hyphenated_string()),
            kind: format!("{:?}", metadata.kind()),
            source: metadata.source().to_uri(false),
            sink: metadata.sink().map(|value| value.to_uri(false)),
            encoding: metadata.payload_encoding().map(|value| value.id()),
            present: metadata.payload_encoding().is_some(),
            length,
            sha256: format!("{:x}", hash.finalize()),
        }))
    }

    /// Projects a classic message's metadata and captures its visible payload.
    ///
    /// Returns `None` when evidence is disabled, or an error if metadata projection
    /// or the enabled capture contract fails.
    pub fn message(
        stage: &str,
        route: Option<&str>,
        message: &UMessage,
    ) -> Result<Option<Self>, UStatus> {
        if std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN").is_none() {
            return Ok(None);
        }
        let metadata = up_rust::frame::metadata::try_project_umessage_to_frame_metadata(message)
            .map_err(|error| invalid(error.to_string()))?;
        let payload = message.payload();
        let bytes = payload.as_deref().unwrap_or_default();
        Self::capture(stage, route, &metadata, bytes.len(), [bytes])
    }

    /// Captures a frame view without joining its payload slices into a new buffer.
    pub fn frame(
        stage: &str,
        route: Option<&str>,
        frame: &(impl UFrameView + ?Sized),
    ) -> Result<Option<Self>, UStatus> {
        if std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN").is_none() {
            return Ok(None);
        }
        Self::capture(
            stage,
            route,
            frame.metadata(),
            frame.payload_len(),
            frame.payload_slices(),
        )
    }

    /// Captures an initialized send buffer without assigning a bridge route label.
    pub fn loan(stage: &str, loan: &impl UTxBuffer) -> Result<Option<Self>, UStatus> {
        if std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN").is_none() {
            return Ok(None);
        }
        Self::capture(
            stage,
            None,
            loan.metadata(),
            loan.payload().len(),
            [loan.payload()],
        )
    }

    /// Prints one prefixed JSON record, or nothing for a disabled observation.
    pub fn emit(record: Option<Self>) {
        if let Some(record) = record {
            println!(
                "{MARKER}{}",
                serde_json::to_string(&record).expect("serializable evidence")
            );
        }
    }
}

fn invalid(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

/// Compare one hop. UUID reconstruction is allowed only at a binding explicitly
/// declaring that loss; bridge-internal comparisons always preserve IDs.
pub fn equivalent(left: &Observation, right: &Observation, preserve_ids: bool) -> bool {
    left.run == right.run
        && left.kind == right.kind
        && left.source == right.source
        && left.sink == right.sink
        && left.encoding == right.encoding
        && left.present == right.present
        && left.length == right.length
        && left.sha256 == right.sha256
        && (!preserve_ids || (left.id == right.id && left.reqid == right.reqid))
}

/// Proves every observed receipt and every bridge egress against actual sends,
/// including multiplicity bounds. Finite receivers need not claim losslessness.
pub fn validate_leg(
    sent: &[Observation],
    received: &[Observation],
    bridge: &[Observation],
    source_preserves_ids: bool,
    sink_preserves_ids: bool,
) -> Result<usize, String> {
    if sent.is_empty() || received.is_empty() {
        return Err("missing source or destination observations".into());
    }
    let ingress: Vec<_> = bridge
        .iter()
        .filter(|r| {
            r.stage == "ingress" && sent.iter().any(|s| equivalent(s, r, source_preserves_ids))
        })
        .collect();
    let egress: Vec<_> = bridge
        .iter()
        .filter(|r| {
            r.stage == "egress" && sent.iter().any(|s| equivalent(s, r, source_preserves_ids))
        })
        .collect();
    if ingress.is_empty() || egress.is_empty() {
        return Err("missing actual bridge handoff".into());
    }
    for record in &egress {
        let entered = ingress
            .iter()
            .filter(|r| r.route == record.route && equivalent(r, record, true))
            .count();
        let forwarded = egress
            .iter()
            .filter(|r| r.route == record.route && equivalent(r, record, true))
            .count();
        let submitted = sent
            .iter()
            .filter(|s| equivalent(s, record, source_preserves_ids))
            .count();
        if entered < forwarded || forwarded > submitted {
            return Err("duplicate or unaccounted bridge forwarding".into());
        }
    }
    for record in received {
        let delivered = received
            .iter()
            .filter(|r| equivalent(r, record, sink_preserves_ids))
            .count();
        let forwarded = egress
            .iter()
            .filter(|r| equivalent(r, record, sink_preserves_ids))
            .count();
        if forwarded == 0 || delivered > forwarded {
            return Err("receipt has no matching bridge egress or exceeds its multiplicity".into());
        }
    }
    Ok(egress.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(stage: &str) -> Observation {
        Observation {
            schema: 1,
            run: "row".into(),
            actor: "test".into(),
            stage: stage.into(),
            route: Some("a->b".into()),
            id: "id".into(),
            reqid: None,
            kind: "Publish".into(),
            source: "//a/1/1/8001".into(),
            sink: None,
            encoding: Some(1),
            present: true,
            length: 3,
            sha256: "abc".into(),
        }
    }

    #[test]
    fn direct_delivery_without_bridge_cannot_pass() {
        assert!(validate_leg(&[observation("tx")], &[observation("rx")], &[], true, true).is_err());
    }

    #[test]
    fn authentic_handoffs_preserve_payload_and_metadata() {
        assert_eq!(
            validate_leg(
                &[observation("tx")],
                &[observation("rx")],
                &[observation("ingress"), observation("egress")],
                true,
                true
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn changed_payload_is_not_a_receipt() {
        let mut wrong = observation("rx");
        wrong.sha256 = "different".into();
        assert!(validate_leg(
            &[observation("tx")],
            &[wrong],
            &[observation("ingress"), observation("egress")],
            true,
            true
        )
        .is_err());
    }

    #[test]
    fn feedback_cannot_inflate_source_multiplicity() {
        assert!(validate_leg(
            &[observation("tx")],
            &[observation("rx")],
            &[
                observation("ingress"),
                observation("egress"),
                observation("ingress"),
                observation("egress")
            ],
            true,
            true
        )
        .is_err());
    }

    #[test]
    fn foreign_run_and_wrong_route_are_rejected() {
        let mut wrong = observation("egress");
        wrong.route = Some("a->c".into());
        assert!(validate_leg(
            &[observation("tx")],
            &[observation("rx")],
            &[observation("ingress"), wrong],
            true,
            true
        )
        .is_err());
        let mut wrong = observation("rx");
        wrong.run = "other-row".into();
        assert!(validate_leg(
            &[observation("tx")],
            &[wrong],
            &[observation("ingress"), observation("egress")],
            true,
            true
        )
        .is_err());
    }
}
