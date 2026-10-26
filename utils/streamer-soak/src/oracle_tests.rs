// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Adversarial synthetic message journals; these are not live transport evidence.
use super::*;
use crate::journal::Context;
use serde_json::{json, Value};
use test_case::test_case;

type Event = (String, String, Value);

fn leg_events(id: &str, sequence: u64, request: Option<&str>, rpc: bool, drop: bool) -> Vec<Event> {
    let response = request.is_some();
    let (sender, receiver, offer, accepted) = if response {
        ("sink", "source", "response_offer", "response_accepted")
    } else {
        ("source", "sink", "offer", "accepted")
    };
    [(sender, offer), (sender, accepted), ("bridge", "ingress"),
        ("bridge", "egress"), (receiver, "receive"),
        (receiver, if drop { "queue_drop" } else { "processed" })]
        .into_iter().map(|(actor, stage)| {
            let observation = json!({"schema": 1, "run": "fixture", "actor": actor, "stage": stage,
                "route": if actor == "bridge" { Some(if response { "reverse" } else { "forward" }) } else { None },
                "id": id, "reqid": request, "kind": if response { "Response" } else if rpc { "Request" } else { "Publish" },
                "source": if response { "//b/2/1/11" } else { "//a/1/1/0" },
                "sink": if response { Some("//a/1/1/0") } else if rpc { Some("//b/2/1/11") } else { None },
                "encoding": 1, "present": true, "length": 3, "sha256": "a".repeat(64)});
            if actor == "bridge" {
                (actor.into(), "FLOW_MESSAGE_EVIDENCE".into(), observation)
            } else {
                (actor.into(), "OP_EVENT".into(), json!({"observation": observation, "event": stage,
                    "attributes_sha256": if response { "d".repeat(64) } else { "b".repeat(64) },
                    "sequence": if actor == "source" && !response { Some(sequence) } else { None }, "phase": "normal"}))
            }
        }).collect()
}

fn publish(id: &str, sequence: u64, drop: bool) -> Vec<Event> {
    leg_events(id, sequence, None, false, drop)
}

fn rpc() -> Vec<Event> {
    let mut values = leg_events("one", 1, None, true, false);
    values.extend(leg_events("reply", 0, Some("one"), true, false));
    values
}

fn lifecycle(mut events: Vec<Event>) -> Vec<Event> {
    let mut values: Vec<Event> = ["source", "sink", "bridge"]
        .into_iter()
        .map(|actor| (actor.into(), "OP_START".into(), json!({})))
        .collect();
    let offered = events
        .iter()
        .filter(|(_, _, v)| v["event"] == "offer")
        .count();
    let metrics: Vec<Event> = ["source", "sink"]
        .into_iter()
        .map(|actor| {
            let count = |stage| {
                events
                    .iter()
                    .filter(|(a, _, v)| a == actor && v["event"] == stage)
                    .count()
            };
            (
                actor.into(),
                "OP_METRICS".into(),
                json!({"final": true, "queue_capacity": 32,
            "queue_occupancy": 0, "errors": 0, "live_application_rx_loans": 0,
            "live_application_tx_loans": 0, "max_application_rx_loans": 1,
            "max_application_tx_loans": 1, "received": count("receive"),
            "processed": count("processed"), "queue_drops": count("queue_drop")}),
            )
        })
        .collect();
    values.append(&mut events);
    values.push((
        "source".into(),
        "OP_SOURCE_DRAINED".into(),
        json!({"offered": offered}),
    ));
    values.extend(metrics);
    values.extend(
        ["source", "sink", "bridge"]
            .into_iter()
            .map(|actor| (actor.into(), "OP_FINISH".into(), json!({"success": true}))),
    );
    values
}

fn evaluate_events(values: Vec<Event>, role: Role, limit: usize) -> Result<Report> {
    let mut oracle = Oracle::new(role, "fixture".into(), 32, limit);
    for (index, (actor, kind, value)) in values.into_iter().enumerate() {
        oracle.observe(Record {
            ordinal: index as u64 + 1,
            actor,
            kind,
            value,
            context: Context {
                run: "fixture".into(),
                source_identity: "a".repeat(64),
                plan_sha256: "b".repeat(64),
            },
        })?;
    }
    oracle.finish()
}

fn evaluate(values: Vec<Event>, role: Role) -> Result<Report> {
    evaluate_events(lifecycle(values), role, 4096)
}

#[test]
fn accounts_for_actual_handoff_and_explicit_queue_loss() {
    let mut values = publish("one", 1, false);
    values.extend(publish("two", 2, true));
    let report = evaluate(values, Role::PublisherSubscriber).unwrap();
    let count = &report.phases["normal"];
    assert_eq!(
        (
            count.offered,
            count.accepted,
            count.processed,
            count.explicit_application_loss
        ),
        (2, 2, 1, 1)
    );
    assert_eq!(report.unaccounted, 0);
}

#[test]
fn accepts_cross_process_journal_interleaving() {
    let mut values = publish("one", 1, false);
    values.rotate_left(2); // Sink and bridge logs may arrive before source logs.
    assert!(evaluate(values, Role::PublisherSubscriber).is_ok());
}

#[test]
fn rejects_direct_delivery_without_bridge() {
    let values = publish("one", 1, false)
        .into_iter()
        .filter(|(actor, _, _)| actor != "bridge")
        .collect();
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

#[test_case(3; "bridge_handoff")]
#[test_case(4; "sink_receipt")]
fn rejects_duplicate_observations(index: usize) {
    let mut values = publish("one", 1, false);
    values.push(values[index].clone());
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

#[test_case("sha256"; "payload_hash")]
#[test_case("source"; "source_uri")]
#[test_case("run"; "foreign_run")]
fn rejects_message_mutation(field: &str) {
    let mut values = publish("one", 1, false);
    values[4].2["observation"][field] = json!("c".repeat(64));
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

#[test]
fn requires_full_attribute_hash_and_equality() {
    let mut values = publish("one", 1, false);
    values[4].2["attributes_sha256"] = json!("c".repeat(64));
    assert!(evaluate(values.clone(), Role::PublisherSubscriber).is_err());
    values[4]
        .2
        .as_object_mut()
        .unwrap()
        .remove("attributes_sha256");
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

#[test]
fn rejects_unaccounted_loss() {
    let mut values = publish("one", 1, false);
    values.pop();
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

#[test]
fn rejects_source_sequence_gap() {
    assert!(evaluate(publish("one", 2, false), Role::PublisherSubscriber).is_err());
}

#[test]
fn rejects_receipt_reordering() {
    let mut values = publish("one", 1, false);
    values.extend(publish("two", 2, false));
    values.swap(4, 10);
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

#[test]
fn unordered_delivery_is_fully_accounted_without_disabling_identity_checks() {
    let mut events = publish("one", 1, false);
    events.extend(publish("two", 2, false));
    events.swap(4, 10);
    let mut oracle =
        Oracle::with_order(Role::PublisherSubscriber, "fixture".into(), 32, 4096, false);
    for (index, (actor, kind, value)) in lifecycle(events).into_iter().enumerate() {
        oracle
            .observe(Record {
                ordinal: index as u64 + 1,
                actor,
                kind,
                value,
                context: Context {
                    run: "fixture".into(),
                    source_identity: "a".repeat(64),
                    plan_sha256: "b".repeat(64),
                },
            })
            .unwrap();
    }
    let report = oracle.finish().unwrap();
    assert_eq!(report.phases["normal"].processed, 2);
    assert_eq!(
        (report.reordered_receipts, report.maximum_reorder_distance),
        (1, 1)
    );
}

#[test]
fn bounds_unreconciled_messages() {
    let values = (1..=3)
        .flat_map(|n| publish(&n.to_string(), n, false).into_iter().take(2))
        .collect();
    let error = evaluate_events(lifecycle(values), Role::PublisherSubscriber, 2).unwrap_err();
    assert!(error.to_string().contains("pending window"));
}

#[test]
fn validates_complete_rpc_and_endpoint_reversal() {
    assert_eq!(
        evaluate(rpc(), Role::ClientServerRpc).unwrap().phases["normal"].rpc_completed,
        1
    );
}

#[test_case("reqid", "foreign"; "wrong_correlation")]
#[test_case("source", "//foreign/1/1/0"; "wrong_responder")]
#[test_case("sha256", "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"; "changed_echo")]
fn rejects_rpc_mutation(field: &str, replacement: &str) {
    let mut values = rpc();
    for (_, kind, value) in &mut values[6..] {
        let observation = if kind == "OP_EVENT" {
            &mut value["observation"]
        } else {
            value
        };
        observation[field] = json!(replacement);
    }
    assert!(evaluate(values, Role::ClientServerRpc).is_err());
}

#[test]
fn rejects_missing_rpc_response() {
    assert!(evaluate(rpc()[..6].to_vec(), Role::ClientServerRpc).is_err());
}

#[test]
fn binds_distinct_stable_route_registrations() {
    let mut values = rpc();
    for (actor, _, value) in &mut values {
        if actor == "bridge" {
            value["route"] = json!(if value["reqid"].is_null() {
                "00000000-0000-7000-8000-000000000001"
            } else {
                "00000000-0000-7000-8000-000000000002"
            });
        }
    }
    assert!(evaluate(values.clone(), Role::ClientServerRpc).is_ok());
    for (actor, _, value) in &mut values {
        if actor == "bridge" {
            value["route"] = json!("same");
        }
    }
    assert!(evaluate(values, Role::ClientServerRpc).is_err());
}

#[test_case("queue_occupancy", 1; "queued_work")]
#[test_case("live_application_rx_loans", 1; "retained_receive_loan")]
#[test_case("live_application_tx_loans", 1; "retained_send_loan")]
#[test_case("received", 3; "counter_disagrees_with_journal")]
fn rejects_incomplete_drain(field: &str, value: u64) {
    let mut values = lifecycle(publish("one", 1, false));
    let metrics = values
        .iter_mut()
        .find(|(actor, kind, _)| actor == "sink" && kind == "OP_METRICS")
        .unwrap();
    metrics.2[field] = json!(value);
    assert!(evaluate_events(values, Role::PublisherSubscriber, 4096).is_err());
}

#[test]
fn rejects_interrupted_lifecycle() {
    let mut values = lifecycle(publish("one", 1, false));
    values.pop();
    assert!(evaluate_events(values, Role::PublisherSubscriber, 4096).is_err());
}

#[test_case(json!({}); "empty_report")]
#[test_case(json!({"callback_failures": 1}); "backend_failure")]
#[test_case(json!({"callback_failures": "unknown"}); "missing_numeric_measurement")]
fn rejects_missing_or_nonzero_transport_health(health: Value) {
    let mut values = lifecycle(publish("one", 1, false));
    values.insert(
        values.len() - 1,
        ("bridge".into(), "OP_TRANSPORT_HEALTH".into(), health),
    );
    assert!(evaluate_events(values, Role::PublisherSubscriber, 4096).is_err());
}

#[test]
fn rejects_undeclared_phases_without_growing_accounting_keys() {
    let mut values = publish("one", 1, false);
    values[0].2["phase"] = json!("arbitrary-phase");
    assert!(evaluate(values, Role::PublisherSubscriber).is_err());
}

fn options() -> crate::ReplayOptions {
    crate::ReplayOptions {
        run: "fixture".into(),
        source_identity: "a".repeat(64),
        plan_sha256: "b".repeat(64),
        role: Role::PublisherSubscriber,
        queue_capacity: 32,
        pending_limit: 4096,
        require_fifo: true,
    }
}

#[test]
fn public_writer_and_independent_replay_agree_on_complete_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let options = options();
    let mut writer = crate::JournalWriter::new(temp.path(), &options, 65536).unwrap();
    let mut reconciler = crate::Reconciler::new(&options).unwrap();
    for (actor, kind, value) in lifecycle(publish("one", 1, false)) {
        reconciler
            .observe(writer.append(&actor, &kind, value).unwrap())
            .unwrap();
    }
    assert_eq!(reconciler.pending(), 0);
    let live = reconciler.finish().unwrap();
    writer.finish(true).unwrap();
    assert_eq!(crate::replay(temp.path(), &options).unwrap(), live);
    let wrong = crate::ReplayOptions {
        source_identity: "c".repeat(64),
        ..options
    };
    assert!(crate::replay(temp.path(), &wrong).is_err());
}

#[test]
fn online_reconciliation_rejects_records_from_another_source_generation() {
    let temp = tempfile::tempdir().unwrap();
    let options = options();
    let mut writer = crate::JournalWriter::new(temp.path(), &options, 65536).unwrap();
    let wrong = crate::ReplayOptions {
        source_identity: "c".repeat(64),
        ..options
    };
    let mut reconciler = crate::Reconciler::new(&wrong).unwrap();
    assert!(reconciler
        .observe(writer.append("source", "OP_START", json!({})).unwrap())
        .is_err());
    writer.finish(false).unwrap();
}

#[test]
fn rejects_unbounded_or_zero_workload_settings_before_creating_a_writer() {
    let temp = tempfile::tempdir().unwrap();
    for pending_limit in [0, 65537] {
        let options = crate::ReplayOptions {
            pending_limit,
            ..options()
        };
        assert!(crate::JournalWriter::new(temp.path(), &options, 65536).is_err());
        assert!(crate::Reconciler::new(&options).is_err());
    }
}

fn someip_events() -> Vec<Event> {
    let mut events = Vec::new();
    for sequence in 1..=5 {
        let mut message = publish(&format!("source-{sequence}"), sequence, false);
        for (actor, kind, value) in &mut message {
            let observation = if kind == "OP_EVENT" {
                &mut value["observation"]
            } else {
                &mut *value
            };
            observation["encoding"] = json!(2);
            observation["sha256"] = json!(format!("{sequence:064x}"));
            if actor == "sink" {
                observation["id"] = json!(format!("reconstructed-{sequence}"));
            }
            if actor == "sink" {
                value["attributes_sha256"] = json!("c".repeat(64));
            }
        }
        events.extend(message);
    }
    lifecycle(events)
}

fn someip_evaluate(events: Vec<Event>, enable: bool) -> Result<Report> {
    let mut oracle = Oracle::new(Role::PublisherSubscriber, "fixture".into(), 32, 4096);
    if enable {
        oracle.enable_someip_finite();
    }
    for (index, (actor, kind, value)) in events.into_iter().enumerate() {
        oracle.observe(Record {
            ordinal: index as u64 + 1,
            actor,
            kind,
            value,
            context: Context {
                run: "fixture".into(),
                source_identity: "a".repeat(64),
                plan_sha256: "b".repeat(64),
            },
        })?;
    }
    oracle.finish()
}

#[test]
fn someip_finite_reports_original_and_reconstructed_ids() {
    let report = someip_evaluate(someip_events(), true).unwrap();
    assert_eq!(report.reconstructed_sink_ids.len(), 5);
    assert_eq!(report.reconstructed_sink_ids["reconstructed-3"], "source-3");
    assert_eq!(report.phases["normal"].processed, 5);
    assert!(someip_evaluate(someip_events(), false).is_err());
}

#[test_case("sha256"; "payload")]
#[test_case("source"; "source_uri")]
#[test_case("encoding"; "encoding")]
#[test_case("length"; "length")]
#[test_case("id"; "duplicate_reconstructed_identity")]
fn someip_finite_rejects_mutated_sink_observables(field: &str) {
    let mut events = someip_events();
    let (_, _, event) = events
        .iter_mut()
        .find(|(actor, _, event)| actor == "sink" && event["event"] == "receive")
        .unwrap();
    event["observation"][field] = if field == "length" || field == "encoding" {
        json!(999)
    } else {
        json!("unmatched")
    };
    assert!(someip_evaluate(events, true).is_err());
}

#[test]
fn someip_finite_rejects_disposition_attribute_mutation() {
    let mut events = someip_events();
    let (_, _, event) = events
        .iter_mut()
        .find(|(actor, _, event)| actor == "sink" && event["event"] == "processed")
        .unwrap();
    event["attributes_sha256"] = json!("d".repeat(64));
    assert!(someip_evaluate(events, true).is_err());
}

#[test]
fn someip_finite_rejects_missing_or_extra_delivery() {
    let mut events = someip_events();
    let index = events
        .iter()
        .position(|(actor, _, event)| actor == "sink" && event["event"] == "processed")
        .unwrap();
    let duplicate = events[index].clone();
    events.insert(index, duplicate);
    assert!(someip_evaluate(events, true).is_err());
    let events = someip_events()
        .into_iter()
        .filter(|(actor, _, event)| !(actor == "sink" && event["event"] == "processed"))
        .collect();
    assert!(someip_evaluate(events, true).is_err());
}
