// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Opt-in CloudEvents protobuf conversion control at application boundaries.
//!
//! This is an allocating interoperability control, separate from the native
//! zero-allocation lanes. Payload codec identity and native agreement are retained.

use bytes::Bytes;
use protobuf::Message;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use up_rust::{
    CloudEvent, NativeProfileAgreement, UCode, UFrameMetadata, UMessage, UMessageBuilder, UStatus,
    UUri,
};

/// Controller opt-in variable for the allocating protobuf CloudEvents control.
pub const MODE_ENV: &str = "UPROTOCOL_CLOUDEVENTS_PROOF";
static NATIVE: OnceLock<NativeProfileAgreement> = OnceLock::new();
static SETUP: OnceLock<Result<(), UStatus>> = OnceLock::new();

fn invalid(error: impl std::fmt::Display) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, error.to_string())
}

/// Enable only the controller's explicit conversion mode.
pub fn enabled() -> bool {
    std::env::var(MODE_ENV).is_ok_and(|value| value == "protobuf-roundtrip-v1")
}

/// Retain the same agreement already admitted by this worker's native setup.
///
/// # Errors
/// Rejects a changed or concurrently installed agreement for this worker.
pub fn install_native(agreement: Option<&NativeProfileAgreement>) -> Result<(), UStatus> {
    if !enabled() {
        return Ok(());
    }
    if let Some(agreement) = agreement {
        if let Some(current) = NATIVE.get() {
            if current.profile() != agreement.profile() {
                return Err(invalid("CloudEvents control native agreement changed"));
            }
        } else {
            NATIVE
                .set(agreement.clone())
                .map_err(|_| invalid("CloudEvents native agreement installed concurrently"))?;
        }
    }
    Ok(())
}

fn serialized_roundtrip(event: &CloudEvent) -> Result<(UMessage, Vec<u8>), UStatus> {
    let bytes = event.write_to_bytes().map_err(invalid)?;
    let parsed = CloudEvent::parse_from_bytes(&bytes).map_err(invalid)?;
    Ok((UMessage::try_from(parsed).map_err(invalid)?, bytes))
}

fn setup_controls() -> Result<(), UStatus> {
    let topic = UUri::try_from("//cloudevents-control/1234/1/8001").map_err(invalid)?;
    let message = UMessageBuilder::publish(topic.clone())
        .build_with_payload(vec![0xff, 0, 0xfe], up_rust::PayloadEncoding::RAW)
        .map_err(invalid)?;
    let template = CloudEvent::try_from(message).map_err(invalid)?;
    let mut rejected = 0;
    for empty in [false, true] {
        let mut text = template.clone();
        text.set_text_data(if empty { "" } else { "legacy text" }.into());
        if UMessage::try_from(text).is_ok() {
            return Err(invalid("legacy CloudEvents text_data was accepted"));
        }
        rejected += 1;
        let mut proto = template.clone();
        let mut value = protobuf::well_known_types::any::Any::new();
        if !empty {
            value.type_url = "type.googleapis.com/legacy.Protobuf".into();
            value.value = vec![8, 1];
        }
        proto.set_proto_data(value);
        if UMessage::try_from(proto).is_ok() {
            return Err(invalid("legacy CloudEvents proto_data was accepted"));
        }
        rejected += 1;
        let mut missing = template.clone();
        missing.attributes.remove("pformat");
        missing.set_binary_data(if empty { vec![] } else { vec![0xff] });
        if UMessage::try_from(missing).is_ok() {
            return Err(invalid(
                "CloudEvents binary_data without pformat was accepted",
            ));
        }
        rejected += 1;
    }
    for proto in [false, true] {
        let mut missing = template.clone();
        missing.attributes.remove("pformat");
        if proto {
            missing.set_proto_data(protobuf::well_known_types::any::Any::new());
        } else {
            missing.set_text_data(String::new());
        }
        if UMessage::try_from(missing).is_ok() {
            return Err(invalid(
                "CloudEvents typed data without pformat was accepted",
            ));
        }
        rejected += 1;
    }
    let absent = UMessageBuilder::publish(topic).build().map_err(invalid)?;
    let event = CloudEvent::try_from(absent).map_err(invalid)?;
    let (restored, _) = serialized_roundtrip(&event)?;
    if restored.payload().is_some() || restored.payload_encoding().is_some() {
        return Err(invalid("CloudEvents absent payload gained presence"));
    }
    let mut opaque = 1;
    for id in [0_u32, 0xfffe, 0xffff] {
        for payload in [vec![], vec![0xff, 0, 0xfe]] {
            let mut event = template.clone();
            let mut format = event
                .attributes
                .get("pformat")
                .cloned()
                .ok_or_else(|| invalid("control template pformat absent"))?;
            format.set_ce_integer(i32::try_from(id).map_err(invalid)?);
            event.attributes.insert("pformat".into(), format);
            event.set_binary_data(payload.clone());
            let (restored, _) = serialized_roundtrip(&event)?;
            if restored.payload_encoding().map(|value| value.id()) != Some(id)
                || restored.payload().as_deref() != Some(payload.as_slice())
            {
                return Err(invalid("CloudEvents opaque identity/presence changed"));
            }
            opaque += 1;
        }
    }
    streamer_flow_evidence::emit_json_or_fail(
        "CLOUDEVENT_SETUP ",
        &serde_json::json!({"schema":1,"rejected_legacy_or_missing_format":rejected,
            "opaque_presence_cases":opaque,"reserved_ids_received_not_originated":true,
            "scope":"Local converter admission controls; no test message was offered to a transport"}),
    );
    Ok(())
}

/// Serialize the actual message as a protobuf CloudEvent and convert it back.
///
/// # Errors
/// Rejects converter, serialization or payload/metadata preservation failures.
pub fn message(message: UMessage, stage: &str) -> Result<UMessage, UStatus> {
    SETUP.get_or_init(setup_controls).clone()?;
    let before = streamer_flow_evidence::Observation::message(stage, None, &message)?;
    let event = CloudEvent::try_from(message.clone()).map_err(invalid)?;
    let (restored, bytes) = serialized_roundtrip(&event)?;
    let after = streamer_flow_evidence::Observation::message(stage, None, &restored)?;
    if before != after
        || message.id() != restored.id()
        || message.type_() != restored.type_()
        || message.source() != restored.source()
        || message.sink() != restored.sink()
        || message.payload() != restored.payload()
        || message.payload_encoding() != restored.payload_encoding()
        || message.request_id() != restored.request_id()
        || message.ttl().filter(|value| *value > 0) != restored.ttl()
        || message.priority().unwrap_or(up_rust::UPriority::CS1)
            != restored.priority().unwrap_or(up_rust::UPriority::CS1)
        || message.commstatus().unwrap_or(UCode::Ok) != restored.commstatus().unwrap_or(UCode::Ok)
        || message.token() != restored.token()
        || message.permission_level() != restored.permission_level()
        || message.traceparent() != restored.traceparent()
    {
        return Err(invalid(
            "CloudEvents conversion changed actual message semantics",
        ));
    }
    let mut hex = String::with_capacity(bytes.len().saturating_mul(2));
    use std::fmt::Write;
    for byte in &bytes {
        write!(&mut hex, "{byte:02x}").map_err(invalid)?;
    }
    let semantics = |value: &UMessage| {
        serde_json::json!({
            "priority":value.priority().map(|priority| priority.to_priority_code()),
            "ttl_ms":value.ttl(),"commstatus":value.commstatus().map(|status| status.value()),
            "permission_level":value.permission_level(),"token":value.token(),
            "traceparent":value.traceparent()
        })
    };
    streamer_flow_evidence::emit_json_or_fail(
        "CLOUDEVENT_PROOF ",
        &serde_json::json!({"schema":1,"stage":stage,"before":before,"after":after,"semantic_before":semantics(&message),"semantic_after":semantics(&restored),
            "event_proto_hex":hex,"event_proto_bytes":bytes.len(),
            "event_proto_sha256":format!("{:x}",Sha256::digest(&bytes)),
            "data_property":"binary_data","allocating_application_control":true}),
    );
    Ok(restored)
}

/// Preserve the existing native identity through the same admitted agreement.
///
/// # Errors
/// Rejects native identity loss, an unknown agreement or any lossy projection.
pub fn frame(
    metadata: &UFrameMetadata,
    payload: Option<Bytes>,
    stage: &str,
) -> Result<(UFrameMetadata, Option<Bytes>), UStatus> {
    let native = metadata.native_type_token().is_some();
    let projected = if native {
        up_rust::frame::metadata::try_project_frame_to_umessage_with_native_agreement(
            metadata.clone(),
            payload,
            NATIVE
                .get()
                .ok_or_else(|| invalid("CloudEvents native control needs admitted agreement"))?,
        )
    } else {
        up_rust::frame::metadata::try_project_frame_to_umessage(metadata.clone(), payload)
    }
    .map_err(invalid)?;
    let restored = message(projected, stage)?;
    let restored_metadata = if native {
        up_rust::frame::metadata::try_project_umessage_to_frame_metadata_with_native_agreement(
            &restored,
            NATIVE
                .get()
                .ok_or_else(|| invalid("CloudEvents native agreement absent"))?,
        )
    } else {
        up_rust::frame::metadata::try_project_umessage_to_frame_metadata(&restored)
    }
    .map_err(invalid)?;
    // Classic CloudEvents normalizes absent priority to CS1, successful
    // status to absence and zero TTL to absence. Check those semantics, then
    // retain the original immutable frame metadata witness: native token and
    // optional field presence are not invented from the CloudEvent bytes.
    if restored_metadata.kind() != metadata.kind()
        || restored_metadata.id() != metadata.id()
        || restored_metadata.source() != metadata.source()
        || restored_metadata.sink() != metadata.sink()
        || restored_metadata.reqid() != metadata.reqid()
        || restored_metadata.payload_encoding() != metadata.payload_encoding()
        || restored_metadata.native_type_token() != metadata.native_type_token()
        || restored_metadata
            .priority()
            .unwrap_or(up_rust::UPriority::CS1)
            != metadata.priority().unwrap_or(up_rust::UPriority::CS1)
        || restored_metadata.comm_status().unwrap_or(UCode::Ok)
            != metadata.comm_status().unwrap_or(UCode::Ok)
        || restored_metadata.ttl().filter(|ttl| !ttl.is_zero())
            != metadata.ttl().filter(|ttl| !ttl.is_zero())
        || restored_metadata.permission_level() != metadata.permission_level()
        || restored_metadata.token() != metadata.token()
        || restored_metadata.traceparent() != metadata.traceparent()
    {
        return Err(invalid(
            "CloudEvents frame semantics/native identity changed",
        ));
    }
    streamer_flow_evidence::emit_json_or_fail(
        "CLOUDEVENT_FRAME_WITNESS ",
        &serde_json::json!({
            "schema":1,"stage":stage,"id":metadata.id().to_hyphenated_string(),
            "actor":std::env::var("UPROTOCOL_FLOW_EVIDENCE_ACTOR").ok(),
            "native_token_before":metadata.native_type_token().map(|token| format!("{token:?}")),
            "native_token_reconstructed":restored_metadata.native_type_token().map(|token| format!("{token:?}")),
            "original_frame_metadata_retained":true,
            "retained_priority":metadata.priority().map(|value| value.to_priority_code()),
            "projected_priority":restored_metadata.priority().map(|value| value.to_priority_code()),
            "retained_status":metadata.comm_status().map(|value| value.value()),
            "projected_status":restored_metadata.comm_status().map(|value| value.value()),
            "retained_zero_ttl":metadata.ttl().is_some_and(|value| value.is_zero()),
            "all_other_classic_fields_equal":true
        }),
    );
    Ok((metadata.clone(), restored.payload()))
}

/// Check every received borrowed slice without forming a typed payload reference.
///
/// # Errors
/// Rejects non-contiguous or inconsistent carriers and lossy conversion.
pub fn received_frame(frame: &(impl up_rust::UFrameView + ?Sized)) -> Result<(), UStatus> {
    let payload = if frame.has_payload() {
        Some(Bytes::copy_from_slice(
            frame
                .try_contiguous_payload()
                .ok_or_else(|| invalid("CloudEvents control requires contiguous payload"))?,
        ))
    } else {
        None
    };
    self::frame(frame.metadata(), payload, "rx")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_protobuf_events_preserve_all_four_message_roles() {
        let source = UUri::try_from("//source/1234/1/8001").unwrap();
        let destination = UUri::try_from("//sink/1234/1/0").unwrap();
        let method = UUri::try_from("//sink/1234/1/1000").unwrap();
        let reply = UUri::try_from("//source/1234/1/0").unwrap();
        let request = UMessageBuilder::request(method, reply, 1000)
            .build_with_payload(vec![0xff, 0, 0xfe], up_rust::PayloadEncoding::RAW)
            .unwrap();
        let response = UMessageBuilder::response_for_request(request.attributes())
            .build_with_payload(vec![1, 0, 2], up_rust::PayloadEncoding::RAW)
            .unwrap();
        let messages = [
            UMessageBuilder::publish(source)
                .build_with_payload(Vec::<u8>::new(), up_rust::PayloadEncoding::RAW)
                .unwrap(),
            UMessageBuilder::notification(
                UUri::try_from("//source/1234/1/8000").unwrap(),
                destination,
            )
            .build_with_payload(vec![3, 0, 4], up_rust::PayloadEncoding::RAW)
            .unwrap(),
            request,
            response,
        ];
        for value in messages {
            let restored = message(value.clone(), "test").unwrap();
            assert_eq!(value.payload(), restored.payload());
            assert_eq!(value.type_(), restored.type_());
            assert_eq!(value.request_id(), restored.request_id());
        }
    }
}

#[cfg(test)]
mod native_projection_tests {
    use super::*;
    use std::sync::Arc;
    use up_rust::frame::native::{
        NativeByteOrder, NativeProfile, NativeProfileMode, NativeProfileTable, NativeRepresentation,
    };

    #[test]
    fn native_token_mismatch_is_rejected_before_cloudevents_projection() {
        let representation = NativeRepresentation::new(
            "cloudevents.control.u32",
            1,
            4,
            4,
            NativeByteOrder::LittleEndian,
            vec![],
        )
        .unwrap();
        let profile = NativeProfile::new(
            "cloudevents.control",
            1,
            NativeProfileMode::Table(
                NativeProfileTable::new([(
                    up_rust::PayloadEncoding::from_id(0xf101).unwrap(),
                    representation.clone(),
                )])
                .unwrap(),
            ),
        )
        .unwrap();
        let agreement = NativeProfileAgreement::new(Arc::new(profile.clone()), &profile).unwrap();
        let identity = agreement.identity_for(&representation).unwrap();
        NATIVE
            .set(agreement)
            .expect("isolated native control agreement");
        let topic = UUri::try_from("//source/1234/1/8001").unwrap();
        let metadata = UFrameMetadata::publish(topic.clone())
            .with_native_payload_identity(identity)
            .build()
            .unwrap();
        let payload = Bytes::copy_from_slice(&42_u32.to_le_bytes());
        let (restored, bytes) = frame(&metadata, Some(payload.clone()), "test").unwrap();
        assert_eq!(restored, metadata);
        assert_eq!(bytes, Some(payload.clone()));
        assert_ne!(
            metadata.native_type_token(),
            Some(up_rust::NativeTypeToken::from_u32(0))
        );
        let wrong = UFrameMetadata::publish(topic)
            .with_payload_encoding(up_rust::PayloadEncoding::from_id(0xf101).unwrap())
            .with_native_type_token(up_rust::NativeTypeToken::from_u32(0))
            .build()
            .unwrap();
        assert!(frame(&wrong, Some(payload), "test").is_err());
    }
}
