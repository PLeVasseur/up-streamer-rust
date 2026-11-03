// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Bounded independent sequence, complete-attribute and actual-handoff reconciliation.
use crate::config::Role;
use crate::journal::Record;
use anyhow::{ensure, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use streamer_flow_evidence::Observation;

#[cfg(test)]
#[path = "oracle_tests.rs"]
mod tests;

/// A retired message id, kept compactly: a canonical lowercase hyphenated UUID as its 16 bytes,
/// any other string verbatim, so two ids share a key exactly when they are the same string. Each
/// row keeps up to twice its pending limit of retired ids, and that memory counts toward a soak
/// cohort's aggregate memory bound.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum RetiredId {
    Uuid([u8; 16]),
    Other(Box<str>),
}

impl RetiredId {
    fn new(id: &str) -> Self {
        let mut bytes = [0u8; 16];
        let mut digits = 0;
        let canonical = id.len() == 36
            && id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    return byte == b'-';
                }
                let nibble = match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    _ => return false,
                };
                bytes[digits / 2] |= nibble << if digits % 2 == 0 { 4 } else { 0 };
                digits += 1;
                true
            });
        if canonical {
            Self::Uuid(bytes)
        } else {
            Self::Other(id.into())
        }
    }
}

#[derive(Clone)]
struct Fact {
    observation: Observation,
    attributes: Option<String>,
    sequence: Option<u64>,
    phase: Option<String>,
    reconstructed_sink: bool,
}

#[derive(Default)]
struct Entry {
    facts: BTreeMap<(String, String), Fact>,
}
impl Entry {
    fn get(&self, actor: &str, stage: &str) -> Option<&Fact> {
        self.facts.get(&(actor.into(), stage.into()))
    }
}

#[derive(Default, Clone, Debug, Serialize)]
pub(crate) struct Counts {
    pub offered: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub processed: u64,
    pub explicit_application_loss: u64,
    pub rpc_completed: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub reconstructed_sink_ids: BTreeMap<String, String>,
    pub reordered_receipts: u64,
    pub maximum_reorder_distance: u64,
    pub phases: BTreeMap<String, Counts>,
    pub pending_peak: usize,
    pub records: u64,
    pub unaccounted: usize,
    pub route_ids: BTreeMap<String, String>,
    pub max_application_leases: u64,
    pub application_lease_peaks: BTreeMap<String, [u64; 2]>,
}

pub(crate) struct Oracle {
    someip_finite: bool,
    reconstructed_sink_ids: BTreeMap<String, String>,
    reconstructed_attributes: BTreeMap<String, String>,
    require_fifo: bool,
    reordered: u64,
    reorder_distance: u64,
    role: Role,
    run: String,
    pending: BTreeMap<String, Entry>,
    retired: BTreeSet<RetiredId>,
    retired_order: VecDeque<RetiredId>,
    receive_order: [VecDeque<(String, Option<String>)>; 2],
    last_received: [u64; 2],
    sequence: u64,
    phases: BTreeMap<String, Counts>,
    routes: BTreeMap<String, String>,
    peak: usize,
    records: u64,
    starts: BTreeSet<String>,
    finishes: BTreeSet<String>,
    final_metrics: BTreeSet<String>,
    final_counts: BTreeMap<String, (u64, u64, u64)>,
    received_counts: BTreeMap<String, (u64, u64, u64)>,
    source_drained: bool,
    max_leases: u64,
    lease_peaks: BTreeMap<String, [u64; 2]>,
    limit: usize,
    queue_capacity: u64,
}

fn equal(left: &Fact, right: &Fact) -> Result<()> {
    let a = &left.observation;
    let b = &right.observation;
    ensure!(
        a.run == b.run
            && (a.id == b.id || right.reconstructed_sink)
            && a.reqid == b.reqid
            && a.kind == b.kind
            && a.source == b.source
            && a.sink == b.sink
            && a.encoding == b.encoding
            && a.present == b.present
            && a.length == b.length
            && a.sha256 == b.sha256,
        "message identity, metadata or payload changed"
    );
    if let (Some(a), Some(b)) = (&left.attributes, &right.attributes) {
        if right.reconstructed_sink {
            return Ok(());
        }
        ensure!(a == b, "complete attributes changed");
    }
    Ok(())
}

fn leg(
    entry: &Entry,
    sender: &str,
    receiver: &str,
    offered: &str,
    accepted: &str,
) -> Result<Option<(bool, String)>> {
    let Some(base) = entry.get(sender, offered) else {
        return Ok(None);
    };
    let stages = [
        (sender, accepted),
        ("bridge", "ingress"),
        ("bridge", "egress"),
        (receiver, "receive"),
    ];
    for (actor, stage) in stages {
        let Some(value) = entry.get(actor, stage) else {
            return Ok(None);
        };
        equal(base, value)?;
    }
    let processed = entry.get(receiver, "processed");
    let dropped = entry.get(receiver, "queue_drop");
    if processed.is_none() && dropped.is_none() {
        return Ok(None);
    }
    ensure!(
        processed.is_some() != dropped.is_some(),
        "ambiguous receive disposition"
    );
    equal(base, processed.or(dropped).expect("disposition"))?;
    ensure!(
        entry.facts.len() == 6,
        "unexpected facts on delivered message"
    );
    let ingress = &entry
        .get("bridge", "ingress")
        .expect("checked")
        .observation
        .route;
    let egress = &entry
        .get("bridge", "egress")
        .expect("checked")
        .observation
        .route;
    ensure!(
        ingress == egress && ingress.as_ref().is_some_and(|route| !route.is_empty()),
        "missing or changed bridge handoff"
    );
    Ok(Some((
        processed.is_some(),
        ingress.clone().expect("checked route"),
    )))
}

impl Oracle {
    #[cfg(test)]
    pub fn new(role: Role, run: String, queue_capacity: u64, limit: usize) -> Self {
        Self::with_order(role, run, queue_capacity, limit, true)
    }

    pub fn with_order(
        role: Role,
        run: String,
        queue_capacity: u64,
        limit: usize,
        require_fifo: bool,
    ) -> Self {
        Self {
            someip_finite: false,
            reconstructed_sink_ids: BTreeMap::new(),
            reconstructed_attributes: BTreeMap::new(),
            require_fifo,
            reordered: 0,
            reorder_distance: 0,
            role,
            run,
            pending: BTreeMap::new(),
            retired: BTreeSet::new(),
            retired_order: VecDeque::new(),
            receive_order: Default::default(),
            last_received: [0, 0],
            sequence: 0,
            phases: BTreeMap::new(),
            routes: BTreeMap::new(),
            peak: 0,
            records: 0,
            starts: BTreeSet::new(),
            finishes: BTreeSet::new(),
            final_metrics: BTreeSet::new(),
            final_counts: BTreeMap::new(),
            received_counts: BTreeMap::new(),
            source_drained: false,
            max_leases: 0,
            lease_peaks: BTreeMap::new(),
            limit,
            queue_capacity,
        }
    }

    pub fn observe(&mut self, record: Record) -> Result<()> {
        self.records = self
            .records
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("record count exhausted"))?;
        ensure!(
            record.ordinal == self.records && record.context.run == self.run,
            "foreign or reordered record"
        );
        let value = &record.value;
        ensure!(
            ["source", "sink", "bridge"].contains(&record.actor.as_str()),
            "unknown actor"
        );
        ensure!(
            !self.finishes.contains(&record.actor),
            "observation after worker finish"
        );
        if record.kind != "OP_START" {
            ensure!(
                self.starts.contains(&record.actor),
                "observation before worker start"
            );
        }
        match record.kind.as_str() {
            "OP_ERROR" => anyhow::bail!("worker reported an error: {value}"),
            "OP_START" => {
                ensure!(self.starts.insert(record.actor), "duplicate worker start");
                return Ok(());
            }
            "OP_FINISH" => {
                ensure!(
                    value["success"] == true && self.finishes.insert(record.actor),
                    "worker failure or duplicate finish"
                );
                return Ok(());
            }
            "OP_METRICS" => {
                ensure!(
                    record.actor != "bridge" && !self.final_metrics.contains(&record.actor),
                    "unexpected metrics"
                );
                let number = |key: &str| {
                    value[key]
                        .as_u64()
                        .ok_or_else(|| anyhow::anyhow!("invalid metric {key}"))
                };
                ensure!(
                    number("queue_capacity")? == self.queue_capacity
                        && number("queue_occupancy")? <= self.queue_capacity
                        && number("errors")? == 0,
                    "queue/error bound breached"
                );
                self.max_leases = self.max_leases.max(
                    number("max_application_rx_loans")?
                        .checked_add(number("max_application_tx_loans")?)
                        .ok_or_else(|| anyhow::anyhow!("lease counter overflow"))?,
                );
                let peaks = self.lease_peaks.entry(record.actor.clone()).or_default();
                peaks[0] = peaks[0].max(number("max_application_rx_loans")?);
                peaks[1] = peaks[1].max(number("max_application_tx_loans")?);
                if value["final"] == true {
                    ensure!(
                        number("queue_occupancy")? == 0
                            && number("live_application_rx_loans")? == 0
                            && number("live_application_tx_loans")? == 0,
                        "retained work after drain"
                    );
                    ensure!(
                        self.final_metrics.insert(record.actor.clone()),
                        "duplicate final metrics"
                    );
                    self.final_counts.insert(
                        record.actor.clone(),
                        (
                            number("received")?,
                            number("processed")?,
                            number("queue_drops")?,
                        ),
                    );
                }
                return Ok(());
            }
            "OP_TRANSPORT_HEALTH" => {
                let counters = value
                    .as_object()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("missing transport health counters"))?;
                for (key, value) in counters {
                    ensure!(value.as_u64() == Some(0), "transport health error: {key}");
                }
                return Ok(());
            }
            "OP_EVENT" | "FLOW_MESSAGE_EVIDENCE" => {}
            "OP_SOURCE_DRAINED" => {
                ensure!(
                    record.actor == "source"
                        && !self.source_drained
                        && value["offered"].as_u64() == Some(self.sequence),
                    "source drain count mismatch"
                );
                self.source_drained = true;
                return Ok(());
            }
            "READY" | "OP_REJECTION" | "NATIVE_DEPLOYMENT_LOADED" | "NATIVE_IDENTITY_VERIFIED" => {
                return Ok(())
            }
            _ => anyhow::bail!("unknown journal record kind"),
        }
        let operation = record.kind == "OP_EVENT";
        let observation: Observation = serde_json::from_value(if operation {
            value["observation"].clone()
        } else {
            value.clone()
        })?;
        let stage = if operation {
            value["event"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing event"))?
        } else {
            &observation.stage
        }
        .to_owned();
        ensure!(
            observation.run == self.run
                && observation.actor == record.actor
                && observation.schema == 1
                && observation.stage == stage,
            "foreign observation"
        );
        ensure!(
            observation.present
                && observation.encoding.is_some()
                && observation.length > 0
                && crate::artifact::is_digest(&observation.sha256),
            "invalid payload observation"
        );
        let mut id = observation.id.clone();
        let reconstructed_sink = self.someip_finite && record.actor == "sink";
        if reconstructed_sink {
            ensure!(
                operation
                    && ["receive", "processed"].contains(&stage.as_str())
                    && observation.kind == "Publish"
                    && observation.encoding == Some(2)
                    && observation.reqid.is_none()
                    && observation.sink.is_none()
                    && observation.present,
                "SOME/IP finite policy is only for protobuf publication receipts"
            );
            let attributes = value["attributes_sha256"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing sink attributes"))?;
            if let Some(previous) = self.reconstructed_attributes.get(&id) {
                ensure!(
                    previous == attributes,
                    "sink attributes changed between receive and disposition"
                );
            } else {
                self.reconstructed_attributes
                    .insert(id.clone(), attributes.into());
            }
            if let Some(canonical) = self.reconstructed_sink_ids.get(&id) {
                id = canonical.clone();
            } else {
                let matches: Vec<_> = self
                    .pending
                    .iter()
                    .filter(|(_, entry)| {
                        entry.get("source", "offer").is_some_and(|fact| {
                            let a = &fact.observation;
                            a.sha256 == observation.sha256
                                && a.length == observation.length
                                && a.source == observation.source
                                && a.encoding == observation.encoding
                                && a.kind == observation.kind
                                && a.present == observation.present
                        })
                    })
                    .map(|(key, _)| key.clone())
                    .collect();
                ensure!(
                    matches.len() == 1 && self.reconstructed_sink_ids.len() < 5,
                    "ambiguous or foreign reconstructed finite publication"
                );
                let canonical = matches[0].clone();
                ensure!(
                    !self
                        .reconstructed_sink_ids
                        .values()
                        .any(|v| v == &canonical),
                    "duplicate reconstructed publication"
                );
                self.reconstructed_sink_ids.insert(id, canonical.clone());
                id = canonical;
            }
        }
        ensure!(
            !id.is_empty() && id.len() <= 128,
            "invalid message identity"
        );
        ensure!(
            if operation {
                match (record.actor.as_str(), stage.as_str()) {
                    ("source", "offer" | "accepted" | "rejected") => observation.reqid.is_none(),
                    ("sink", "receive" | "processed" | "queue_drop") => observation.reqid.is_none(),
                    ("sink", "response_offer" | "response_accepted" | "response_rejected")
                    | ("source", "receive" | "processed" | "queue_drop") => {
                        self.role == Role::ClientServerRpc && observation.reqid.is_some()
                    }
                    _ => false,
                }
            } else {
                record.actor == "bridge" && ["ingress", "egress"].contains(&stage.as_str())
            },
            "invalid observation stage/actor/correlation"
        );
        if operation {
            ensure!(
                value["attributes_sha256"]
                    .as_str()
                    .is_some_and(crate::artifact::is_digest),
                "complete attribute hash missing"
            );
        }
        ensure!(
            !self.retired.contains(&RetiredId::new(&id)),
            "duplicate completed message"
        );
        let fact = Fact {
            reconstructed_sink,
            observation,
            attributes: value
                .get("attributes_sha256")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            sequence: value.get("sequence").and_then(|v| v.as_u64()),
            phase: value
                .get("phase")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        };
        if record.actor == "source" && stage == "offer" {
            ensure!(!self.source_drained, "offer after source drained");
            ensure!(
                matches!(fact.phase.as_deref(), Some("warmup" | "normal" | "slow")),
                "offer outside a declared workload phase"
            );
            self.sequence = self
                .sequence
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("sequence exhausted"))?;
            ensure!(
                fact.sequence == Some(self.sequence),
                "offered sequence gap or reorder"
            );
            ensure!(
                fact.observation.kind
                    == if self.role == Role::PublisherSubscriber {
                        "Publish"
                    } else {
                        "Request"
                    },
                "role/message-kind mismatch"
            );
        }
        if stage == "receive" {
            let index = if record.actor == "sink" {
                0
            } else {
                ensure!(record.actor == "source", "unknown receiver");
                1
            };
            self.receive_order[index].push_back((id.clone(), fact.observation.reqid.clone()));
        }
        let counts = self
            .received_counts
            .entry(record.actor.clone())
            .or_default();
        match stage.as_str() {
            "receive" => counts.0 += 1,
            "processed" => counts.1 += 1,
            "queue_drop" => counts.2 += 1,
            _ => {}
        }
        ensure!(
            self.pending
                .entry(id.clone())
                .or_default()
                .facts
                .insert((record.actor, stage), fact)
                .is_none(),
            "duplicate observation"
        );
        self.peak = self.peak.max(self.pending.len());
        ensure!(self.pending.len() <= self.limit, "pending window exceeded");
        self.check_receive_order()?;
        self.reconcile()
    }

    fn check_receive_order(&mut self) -> Result<()> {
        for direction in 0..2 {
            while let Some((id, request)) = self.receive_order[direction].front() {
                let source_id = if direction == 0 {
                    id
                } else {
                    request
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("response lacks correlation"))?
                };
                let Some(sequence) = self
                    .pending
                    .get(source_id)
                    .and_then(|entry| entry.get("source", "offer"))
                    .and_then(|fact| fact.sequence)
                else {
                    break;
                };
                if sequence <= self.last_received[direction] {
                    let distance = self.last_received[direction] - sequence;
                    self.reordered += 1;
                    self.reorder_distance = self.reorder_distance.max(distance);
                    ensure!(
                        !self.require_fifo,
                        "receive stream reordered under FIFO contract"
                    );
                    ensure!(
                        distance <= self.limit as u64,
                        "receive reordering exceeds bounded window"
                    );
                }
                self.last_received[direction] = self.last_received[direction].max(sequence);
                self.receive_order[direction].pop_front();
            }
        }
        Ok(())
    }

    fn route(&mut self, direction: &str, route: String) -> Result<()> {
        if let Some(previous) = self.routes.get(direction) {
            ensure!(previous == &route, "route registration changed");
        }
        self.routes.insert(direction.into(), route);
        if let (Some(a), Some(b)) = (self.routes.get("forward"), self.routes.get("reverse")) {
            ensure!(a != b, "directions share a route registration");
        }
        Ok(())
    }

    fn retire(&mut self, id: &str) {
        self.pending.remove(id);
        let key = RetiredId::new(id);
        self.retired.insert(key.clone());
        self.retired_order.push_back(key);
        if self.retired_order.len() > self.limit * 2 {
            if let Some(old) = self.retired_order.pop_front() {
                self.retired.remove(&old);
            }
        }
    }

    fn reconcile(&mut self) -> Result<()> {
        let requests: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, entry)| entry.get("source", "offer").is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in requests {
            let entry = &self.pending[&id];
            let offered = entry
                .get("source", "offer")
                .expect("selected offer")
                .clone();
            let phase = offered
                .phase
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("offer phase missing"))?;
            ensure!(
                ["warmup", "normal", "slow"].contains(&phase),
                "offer outside load phases"
            );
            if let Some(rejected) = entry.get("source", "rejected") {
                equal(&offered, rejected)?;
                ensure!(
                    entry.facts.len() == 2,
                    "rejected message delivered downstream"
                );
                let counts = self.phases.entry(phase.into()).or_default();
                counts.offered += 1;
                counts.rejected += 1;
                self.retire(&id);
                continue;
            }
            let Some((processed, forward_route)) =
                leg(entry, "source", "sink", "offer", "accepted")?
            else {
                continue;
            };
            let replies: Vec<_> = self
                .pending
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .get("sink", "response_offer")
                        .is_some_and(|value| value.observation.reqid.as_deref() == Some(&id))
                })
                .map(|(id, _)| id.clone())
                .collect();
            let mut reply_id = None;
            let mut reverse_route = None;
            if self.role == Role::ClientServerRpc && processed {
                ensure!(replies.len() <= 1, "duplicate RPC response");
                let Some(reply) = replies.first() else {
                    continue;
                };
                let entry = &self.pending[reply];
                let response = entry.get("sink", "response_offer").expect("selected reply");
                let a = &offered.observation;
                let b = &response.observation;
                ensure!(
                    b.kind == "Response"
                        && b.source == a.sink.clone().unwrap_or_default()
                        && b.sink.as_deref() == Some(&a.source),
                    "RPC endpoints changed"
                );
                ensure!(
                    b.encoding == a.encoding
                        && b.present == a.present
                        && b.length == a.length
                        && b.sha256 == a.sha256,
                    "RPC echo payload changed"
                );
                ensure!(
                    entry.get("sink", "response_rejected").is_none(),
                    "response send rejected"
                );
                let Some((completed, route)) = leg(
                    entry,
                    "sink",
                    "source",
                    "response_offer",
                    "response_accepted",
                )?
                else {
                    continue;
                };
                ensure!(completed, "response dropped at client");
                reply_id = Some(reply.clone());
                reverse_route = Some(route);
            } else {
                ensure!(replies.is_empty(), "reply without processed request");
            }
            self.route("forward", forward_route)?;
            if let Some(route) = reverse_route {
                self.route("reverse", route)?;
            }
            let counts = self.phases.entry(phase.into()).or_default();
            counts.offered += 1;
            counts.accepted += 1;
            if processed {
                counts.processed += 1;
            } else {
                counts.explicit_application_loss += 1;
            }
            if let Some(reply) = reply_id {
                counts.rpc_completed += 1;
                self.retire(&reply);
            }
            self.retire(&id);
        }
        Ok(())
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn enable_someip_finite(&mut self) {
        self.someip_finite = true;
    }

    pub fn finish(self) -> Result<Report> {
        if self.someip_finite {
            ensure!(
                self.sequence == 5
                    && self.reconstructed_sink_ids.len() == 5
                    && self.phases.values().map(|c| c.processed).sum::<u64>() == 5,
                "SOME/IP finite denominator must remain exactly five"
            );
        }
        ensure!(
            self.pending.is_empty() && self.receive_order.iter().all(VecDeque::is_empty),
            "unaccounted or foreign messages remain"
        );
        ensure!(
            self.sequence > 0 && self.phases.values().map(|c| c.processed).sum::<u64>() > 0,
            "no delivered workload"
        );
        ensure!(
            self.starts == BTreeSet::from(["source".into(), "sink".into(), "bridge".into()])
                && self.finishes == self.starts,
            "worker lifecycle incomplete"
        );
        ensure!(
            self.final_metrics == BTreeSet::from(["source".into(), "sink".into()]),
            "final resource observations missing"
        );
        ensure!(self.source_drained, "source drain acknowledgement missing");
        for actor in ["source", "sink"] {
            let actual = self.received_counts.get(actor).copied().unwrap_or_default();
            ensure!(
                self.final_counts.get(actor) == Some(&actual)
                    && actual.0
                        == actual
                            .1
                            .checked_add(actual.2)
                            .ok_or_else(|| anyhow::anyhow!("receive count overflow"))?,
                "final metrics disagree with message journal"
            );
        }
        Ok(Report {
            reconstructed_sink_ids: self.reconstructed_sink_ids,
            reordered_receipts: self.reordered,
            maximum_reorder_distance: self.reorder_distance,
            phases: self.phases,
            pending_peak: self.peak,
            records: self.records,
            unaccounted: 0,
            route_ids: self.routes,
            max_application_leases: self.max_leases,
            application_lease_peaks: self.lease_peaks,
        })
    }
}
