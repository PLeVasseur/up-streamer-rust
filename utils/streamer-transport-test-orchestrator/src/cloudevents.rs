// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Require serialized CloudEvents conversion evidence for every enabled row.

use super::*;
use protobuf::Message;
use streamer_flow_evidence::Observation;
use up_rust::CloudEvent;

fn semantic_attributes(value: &up_rust::UMessage) -> serde_json::Value {
    json!({"priority":value.priority().map(|priority| priority.to_priority_code()),
        "ttl_ms":value.ttl(),"commstatus":value.commstatus().map(|status| status.value()),
        "permission_level":value.permission_level(),"token":value.token(),
        "traceparent":value.traceparent()})
}

fn verify_defaults(before: &serde_json::Value, after: &serde_json::Value) -> Result<()> {
    let names = [
        "priority",
        "ttl_ms",
        "commstatus",
        "permission_level",
        "token",
        "traceparent",
    ];
    for attributes in [before, after] {
        anyhow::ensure!(
            attributes
                .as_object()
                .is_some_and(|object| object.len() == names.len()
                    && names.iter().all(|name| object.contains_key(*name))),
            "CloudEvents semantic attribute evidence incomplete"
        );
    }
    anyhow::ensure!(
        before["priority"].as_str().unwrap_or("CS1") == after["priority"].as_str().unwrap_or("CS1")
    );
    anyhow::ensure!(
        before["commstatus"].as_i64().unwrap_or(0) == after["commstatus"].as_i64().unwrap_or(0)
    );
    anyhow::ensure!(
        before["ttl_ms"].as_u64().filter(|value| *value > 0)
            == after["ttl_ms"].as_u64().filter(|value| *value > 0)
    );
    for name in ["permission_level", "token", "traceparent"] {
        anyhow::ensure!(
            before[name] == after[name],
            "CloudEvents changed semantic attribute {name}"
        );
    }
    Ok(())
}

fn verify_event(value: &serde_json::Value) -> Result<Observation> {
    anyhow::ensure!(value["schema"] == 1 && value["allocating_application_control"] == true);
    let before: Observation = serde_json::from_value(value["before"].clone())?;
    let after: Observation = serde_json::from_value(value["after"].clone())?;
    anyhow::ensure!(
        before == after,
        "CloudEvents before/after observations differ"
    );
    anyhow::ensure!(matches!(before.stage.as_str(), "tx" | "rx"));
    let hex = value["event_proto_hex"]
        .as_str()
        .context("missing protobuf event bytes")?;
    anyhow::ensure!(hex.len() % 2 == 0, "odd protobuf event hex length");
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .map_err(anyhow::Error::from)
                .and_then(|text| u8::from_str_radix(text, 16).map_err(anyhow::Error::from))
        })
        .collect::<Result<_>>()?;
    anyhow::ensure!(value["event_proto_bytes"].as_u64() == Some(bytes.len() as u64));
    anyhow::ensure!(value["event_proto_sha256"] == format!("{:x}", Sha256::digest(&bytes)));
    let event = CloudEvent::parse_from_bytes(&bytes)?;
    anyhow::ensure!(
        event.has_binary_data() && !event.has_text_data() && !event.has_proto_data(),
        "matrix CloudEvent must carry actual opaque binary_data"
    );
    let message = up_rust::UMessage::try_from(event).map_err(|error| anyhow!("{error}"))?;
    anyhow::ensure!(
        value["semantic_after"] == semantic_attributes(&message),
        "CloudEvents decoded semantic attributes disagree"
    );
    verify_defaults(&value["semantic_before"], &value["semantic_after"])?;
    let payload = message.payload();
    anyhow::ensure!(
        before.id == message.id().to_hyphenated_string()
            && before.reqid == message.request_id().map(|id| id.to_hyphenated_string())
            && before.kind == format!("{:?}", message.type_())
            && before.source == message.source().to_uri(false)
            && before.sink == message.sink().map(|uri| uri.to_uri(false))
            && before.encoding == message.payload_encoding().map(|encoding| encoding.id())
            && before.present == payload.is_some()
            && before.length == payload.as_deref().unwrap_or_default().len()
            && before.sha256
                == format!(
                    "{:x}",
                    Sha256::digest(payload.as_deref().unwrap_or_default())
                ),
        "serialized CloudEvent disagrees with actual application metadata/payload"
    );
    Ok(before)
}

pub(super) fn verify(row: &MatrixRow, directory: &Path) -> Result<()> {
    let mut actors = BTreeMap::new();
    for actor in ["active", "passive"] {
        let path = directory.join(format!("{actor}.log"));
        let text = structured_capture::read(&path)?;
        anyhow::ensure!(
            !text.contains("CLOUDEVENT_ERROR") && !text.contains("FLOW_EVIDENCE_ERROR")
        );
        let setup: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|line| line.strip_prefix("CLOUDEVENT_SETUP "))
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        anyhow::ensure!(
            setup.len() == 1
                && setup[0]["rejected_legacy_or_missing_format"] == 8
                && setup[0]["opaque_presence_cases"] == 7
                && setup[0]["reserved_ids_received_not_originated"] == true,
            "CloudEvents local admission/presence controls missing"
        );
        let mut converted = BTreeMap::<String, usize>::new();
        let mut actual = BTreeMap::<String, usize>::new();
        let mut count = 0;
        let mut converted_ids = BTreeMap::<(String, String), usize>::new();
        let mut witnesses = BTreeMap::<(String, String), usize>::new();
        let mut witness_count = 0;
        for line in text.lines() {
            if let Some(raw) = line.strip_prefix("CLOUDEVENT_PROOF ") {
                let value = verify_event(&serde_json::from_str(raw)?)?;
                anyhow::ensure!(value.actor == actor);
                *converted_ids
                    .entry((value.id.clone(), value.stage.clone()))
                    .or_default() += 1;
                *converted.entry(serde_json::to_string(&value)?).or_default() += 1;
                count += 1;
            }
            if let Some(raw) = line.strip_prefix(streamer_flow_evidence::MARKER) {
                let value: Observation = serde_json::from_str(raw)?;
                anyhow::ensure!(
                    value.actor == actor && matches!(value.stage.as_str(), "tx" | "rx")
                );
                *actual.entry(serde_json::to_string(&value)?).or_default() += 1;
            }
        }
        for line in text
            .lines()
            .filter_map(|line| line.strip_prefix("CLOUDEVENT_FRAME_WITNESS "))
        {
            let witness: serde_json::Value = serde_json::from_str(line)?;
            anyhow::ensure!(
                witness["schema"] == 1
                    && witness["actor"] == actor
                    && witness["original_frame_metadata_retained"] == true
                    && witness["all_other_classic_fields_equal"] == true
                    && witness["native_token_before"] == witness["native_token_reconstructed"]
            );
            anyhow::ensure!(
                witness["retained_priority"].as_str().unwrap_or("CS1")
                    == witness["projected_priority"].as_str().unwrap_or("CS1")
            );
            anyhow::ensure!(
                witness["retained_status"].as_i64().unwrap_or(0)
                    == witness["projected_status"].as_i64().unwrap_or(0)
            );
            let id = witness["id"].as_str().context("frame witness ID missing")?;
            let stage = witness["stage"]
                .as_str()
                .context("frame witness stage missing")?;
            *witnesses
                .entry((id.to_owned(), stage.to_owned()))
                .or_default() += 1;
            witness_count += 1;
        }
        let kind = if actor == "active" {
            row.source.kind
        } else {
            row.sink.kind
        };
        if kind != EndpointKind::Classic {
            anyhow::ensure!(
                witnesses == converted_ids,
                "native/owned frame witnesses do not cover actual CloudEvents operations"
            );
        } else {
            for (key, count) in &witnesses {
                anyhow::ensure!(converted_ids.get(key).is_some_and(|actual| count <= actual));
            }
        }
        anyhow::ensure!(
            count > 0 && converted == actual,
            "CloudEvents records do not cover actual {actor} sends/receipts"
        );
        actors.insert(
            actor,
            json!({"converted_operations":count,"frame_witnesses":witness_count,"local_rejections":8,
            "opaque_presence_cases":7,"raw_log_sha256":sha256_file(&path)?}),
        );
    }
    atomic_write_json(
        &directory.join("cloudevents-verification.json"),
        &json!({
            "schema":1,"row_id":row.id,"verified":true,"actors":actors,
            "protobuf_events_decoded_again_by_controller":true,
            "all_actual_endpoint_operations_covered":true,
            "allocating_application_control":true,
            "native_identity_requires_original_admitted_agreement":true,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudevents_controller_rejects_changed_payload_even_with_rebound_event_hash() {
        let message = up_rust::UMessageBuilder::publish(
            up_rust::UUri::try_from("//source/1234/1/8001").unwrap(),
        )
        .build_with_payload(vec![0xff, 0, 0xfe], up_rust::PayloadEncoding::RAW)
        .unwrap();
        let event = CloudEvent::try_from(message.clone()).unwrap();
        let bytes = event.write_to_bytes().unwrap();
        let restored =
            up_rust::UMessage::try_from(CloudEvent::parse_from_bytes(&bytes).unwrap()).unwrap();
        let observation = Observation {
            schema: 1,
            run: "consumer-control".into(),
            actor: "active".into(),
            stage: "tx".into(),
            route: None,
            id: message.id().to_hyphenated_string(),
            reqid: None,
            kind: "Publish".into(),
            source: message.source().to_uri(false),
            sink: None,
            encoding: Some(up_rust::PayloadEncoding::RAW.id()),
            present: true,
            length: 3,
            sha256: format!("{:x}", Sha256::digest([0xff, 0, 0xfe])),
        };
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let mut value = json!({"schema":1,"allocating_application_control":true,"before":observation,"after":observation,
            "semantic_before":semantic_attributes(&message),"semantic_after":semantic_attributes(&restored),
            "event_proto_hex":hex(&bytes),"event_proto_bytes":bytes.len(),"event_proto_sha256":format!("{:x}",Sha256::digest(&bytes))});
        assert!(verify_event(&value).is_ok());
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("semantic_before");
        assert!(verify_event(&missing).is_err());
        let mut wrong = event;
        wrong.set_binary_data(vec![0xff, 0, 0xfd]);
        let bytes = wrong.write_to_bytes().unwrap();
        value["event_proto_hex"] = json!(hex(&bytes));
        value["event_proto_bytes"] = json!(bytes.len());
        value["event_proto_sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        assert!(verify_event(&value).is_err());
    }
}
