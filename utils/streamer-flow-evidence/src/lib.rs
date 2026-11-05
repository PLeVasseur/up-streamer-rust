// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Opt-in observational evidence for integration tests. Never part of a wire format.

mod record_writer;
pub use record_writer::{
    dedicated_output_is_prepared, emit_json_or_fail, emit_json_record,
    emit_native_forward_completed, emit_preencoded_or_fail, emit_preencoded_record,
    prepare_output_from_env, OUTPUT_PATH_ENV,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use up_rust::{UCode, UFrameMetadata, UFrameView, UMessage, UStatus, UTxBuffer};

/// Prefix before each JSON observation in the declared capture or standalone stdout.
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

    /// Captures an admitted borrowed header and its opaque visible payload.
    ///
    /// This does not form a typed payload reference. Disabled evidence returns
    /// before reading bytes; enabled evidence checks metadata/carrier presence.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for an absent actor or inconsistent presence.
    pub fn header(
        stage: &str,
        route: Option<&str>,
        header: &up_rust::RxHeader<'_>,
        payload: Option<&[u8]>,
    ) -> Result<Option<Self>, UStatus> {
        let Ok(run) = std::env::var("UPROTOCOL_FLOW_EVIDENCE_RUN") else {
            return Ok(None);
        };
        let actor = std::env::var("UPROTOCOL_FLOW_EVIDENCE_ACTOR")
            .map_err(|_| invalid("flow evidence requires an explicit actor"))?;
        if payload.is_some() != header.payload_encoding.is_some() {
            return Err(invalid("observed header and carrier presence disagree"));
        }
        let bytes = payload.unwrap_or_default();
        Ok(Some(Self {
            schema: 1,
            run,
            actor,
            stage: stage.into(),
            route: route.map(str::to_owned),
            id: header.id.to_hyphenated_string(),
            reqid: header
                .reqid
                .as_ref()
                .map(|value| value.to_hyphenated_string()),
            kind: format!("{:?}", header.kind),
            source: header.source.to_uri(false),
            sink: header.sink.as_ref().map(|value| value.to_uri(false)),
            encoding: header.payload_encoding.map(|value| value.id()),
            present: payload.is_some(),
            length: bytes.len(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
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
            emit_json_or_fail(MARKER, &record);
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
            source: "//a/1/1/1/8001".into(),
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

#[cfg(test)]
mod borrowed_header_tests {
    use super::*;
    use up_rust::{
        HeaderAdmission, HeaderLimits, PayloadEncoding, StableContainerWire,
        UFrameFieldsMetadataCodec, UUri, UWireMetadataCodec, UWireMetadataContext,
    };

    #[test]
    fn admitted_header_matches_existing_capture_and_rejects_wrong_presence() {
        let old_run = std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_RUN");
        let old_actor = std::env::var_os("UPROTOCOL_FLOW_EVIDENCE_ACTOR");
        std::env::set_var("UPROTOCOL_FLOW_EVIDENCE_RUN", "borrowed-header-unit");
        std::env::set_var("UPROTOCOL_FLOW_EVIDENCE_ACTOR", "bridge");
        for (encoding, payload) in [
            (None, None),
            (Some(PayloadEncoding::PROTOBUF), Some(&b""[..])),
            (
                Some(PayloadEncoding::PROTOBUF),
                Some(&b"actual payload"[..]),
            ),
            (
                Some(PayloadEncoding::CONTRACT_DEFINED),
                Some(&b"opaque"[..]),
            ),
        ] {
            let source = UUri::try_from_parts("source", 0x0001_1234, 1, 0x8001).unwrap();
            let mut builder = UFrameMetadata::publish(source);
            if let Some(encoding) = encoding {
                builder = builder.with_payload_encoding(encoding);
            }
            if encoding == Some(PayloadEncoding::CONTRACT_DEFINED) {
                builder =
                    builder.with_native_type_token(up_rust::NativeTypeToken::from_u32(0x51ab_1234));
            }
            let metadata = builder.build().unwrap();
            let context =
                UWireMetadataContext::from_wire::<StableContainerWire, UFrameFieldsMetadataCodec>();
            let bytes = UFrameFieldsMetadataCodec
                .encode_frame_metadata(context, &metadata)
                .unwrap();
            let admission =
                HeaderAdmission::for_wire::<StableContainerWire>(HeaderLimits::default()).unwrap();
            let header =
                up_rust::parse_header(admission.prefix(), &bytes, &HeaderLimits::default())
                    .unwrap();
            let expected = Observation::capture(
                "ingress",
                Some("native-a -> native-b"),
                &metadata,
                payload.map_or(0, <[u8]>::len),
                [payload.unwrap_or_default()],
            )
            .unwrap();
            assert_eq!(
                Observation::header("ingress", Some("native-a -> native-b"), &header, payload)
                    .unwrap(),
                expected
            );
            let wrong = if payload.is_some() {
                None
            } else {
                Some(&b""[..])
            };
            assert!(Observation::header("ingress", None, &header, wrong).is_err());
        }
        match old_run {
            Some(value) => std::env::set_var("UPROTOCOL_FLOW_EVIDENCE_RUN", value),
            None => std::env::remove_var("UPROTOCOL_FLOW_EVIDENCE_RUN"),
        }
        match old_actor {
            Some(value) => std::env::set_var("UPROTOCOL_FLOW_EVIDENCE_ACTOR", value),
            None => std::env::remove_var("UPROTOCOL_FLOW_EVIDENCE_ACTOR"),
        }
    }
}

/// Observation of an initialized payload borrowed from the actual egress loan.
///
/// This is recorded before native publication, so a later failed put does not
/// erase the copy. `egress` names the configured endpoint, not an ingress route.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PayloadCopyObservation {
    schema: u32,
    run: String,
    egress: String,
    id: String,
    boundary: String,
    copied_payload_bytes: usize,
    payload_slice_copies: u8,
    present: bool,
    sha256: String,
}
impl PayloadCopyObservation {
    /// Hashes the synchronously borrowed initialized egress payload.
    ///
    /// No typed reference is formed and no bytes are retained. Absent and
    /// present-empty payloads have different presence and copy counts.
    pub fn capture(run: &str, egress: &str, id: &up_rust::UUID, payload: Option<&[u8]>) -> Self {
        let bytes = payload.unwrap_or_default();
        Self {
            schema: 1,
            run: run.into(),
            egress: egress.into(),
            id: id.to_hyphenated_string(),
            boundary: "ingress_to_egress_loan".into(),
            copied_payload_bytes: bytes.len(),
            payload_slice_copies: u8::from(payload.is_some()),
            present: payload.is_some(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }
    /// Emits one raw copy record; formatting and hashing costs belong to the process.
    pub fn emit(&self) {
        emit_json_or_fail("FLOW_PAYLOAD_COPY ", self);
    }
}
#[cfg(test)]
mod copied_payload_tests {
    use super::*;
    #[test]
    fn actual_copy_record_preserves_presence_extent_and_independent_known_digest() {
        let id = up_rust::UUID::build();
        let absent = PayloadCopyObservation::capture("run", "egress", &id, None);
        let empty = PayloadCopyObservation::capture("run", "egress", &id, Some(b""));
        let copied = PayloadCopyObservation::capture("run", "egress", &id, Some(b"abc"));
        assert_eq!(
            (
                absent.present,
                absent.copied_payload_bytes,
                absent.payload_slice_copies
            ),
            (false, 0, 0)
        );
        assert_eq!(
            (
                empty.present,
                empty.copied_payload_bytes,
                empty.payload_slice_copies
            ),
            (true, 0, 1)
        );
        assert_eq!(
            (
                copied.present,
                copied.copied_payload_bytes,
                copied.payload_slice_copies
            ),
            (true, 3, 1)
        );
        assert_eq!(
            copied.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(absent.sha256, empty.sha256);
        assert_ne!(absent, empty);
        assert_eq!(
            serde_json::from_str::<PayloadCopyObservation>(
                &serde_json::to_string(&copied).unwrap()
            )
            .unwrap(),
            copied
        );
    }
}
