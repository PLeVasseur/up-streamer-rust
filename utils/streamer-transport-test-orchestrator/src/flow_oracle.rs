// SPDX-License-Identifier: Apache-2.0
//! Independent per-message evidence gate, run after all row processes stop.

use super::*;
use streamer_flow_evidence::{equivalent, validate_leg, Observation, MARKER};

fn records(path: &Path, run: &str, actor: &str) -> Result<Vec<Observation>> {
    let contents = fs::read_to_string(path)?;
    if contents.contains("FLOW_EVIDENCE_ERROR") {
        return Err(anyhow!("evidence capture failed in {}", path.display()));
    }
    let mut records = Vec::new();
    for line in contents.lines() {
        let Some(json) = line.strip_prefix(MARKER) else {
            continue;
        };
        let record: Observation = serde_json::from_str(json)?;
        if record.schema != 1 || record.run != run || record.actor != actor {
            return Err(anyhow!(
                "foreign or malformed message evidence in {}",
                path.display()
            ));
        }
        if record.id.is_empty()
            || record.source.is_empty()
            || record.sha256.len() != 64
            || !record.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || record.present != record.encoding.is_some()
            || (!record.present && record.length != 0)
            || (actor == "streamer" && record.route.as_ref().is_none_or(String::is_empty))
        {
            return Err(anyhow!(
                "incomplete or inconsistent message evidence in {}",
                path.display()
            ));
        }
        if !["tx", "rx", "ingress", "egress"].contains(&record.stage.as_str())
            || (actor == "streamer" && !["ingress", "egress"].contains(&record.stage.as_str()))
            || (actor != "streamer" && !["tx", "rx"].contains(&record.stage.as_str()))
        {
            return Err(anyhow!("invalid evidence stage in {}", path.display()));
        }
        records.push(record);
        if records.len() > 4096 {
            return Err(anyhow!("unbounded message evidence in {}", path.display()));
        }
    }
    Ok(records)
}

pub(super) fn verify(row: &MatrixRow, row_dir: &Path, bundle: &RunBundle) -> Result<()> {
    let run = format!("{}::{}", bundle.root.display(), row.id);
    let active = records(&row_dir.join("active.log"), &run, "active")?;
    let passive = records(&row_dir.join("passive.log"), &run, "passive")?;
    let bridge = records(&row_dir.join("streamer.log"), &run, "streamer")?;
    let pick = |values: &[Observation], stage: &str| -> Vec<Observation> {
        values
            .iter()
            .filter(|v| v.stage == stage)
            .cloned()
            .collect()
    };
    let source_tx = pick(&active, "tx");
    let sink_rx = pick(&passive, "rx");
    let source_ids = row.source.physical != PhysicalTransport::Vsomeip;
    let sink_ids = row.sink.physical != PhysicalTransport::Vsomeip;
    let forward = validate_leg(&source_tx, &sink_rx, &bridge, source_ids, sink_ids)
        .map_err(|error| anyhow!("forward message proof for {}: {error}", row.id))?;
    let sink_tx = pick(&passive, "tx");
    let source_rx = pick(&active, "rx");
    let reverse = if row.role == RoleStyle::ClientServerRpc {
        for response in &sink_tx {
            if !sink_rx
                .iter()
                .any(|request| response.reqid.as_ref() == Some(&request.id))
            {
                return Err(anyhow!(
                    "server response is not correlated with a received request"
                ));
            }
        }
        for response in &source_rx {
            if !source_tx
                .iter()
                .any(|request| response.reqid.as_ref() == Some(&request.id))
            {
                return Err(anyhow!(
                    "client response is not correlated with a submitted request"
                ));
            }
        }
        validate_leg(&sink_tx, &source_rx, &bridge, sink_ids, source_ids)
            .map_err(|error| anyhow!("reverse message proof for {}: {error}", row.id))?
    } else {
        0
    };
    for record in &bridge {
        if !source_tx
            .iter()
            .any(|sent| equivalent(sent, record, source_ids))
            && !sink_tx
                .iter()
                .any(|sent| equivalent(sent, record, sink_ids))
        {
            return Err(anyhow!(
                "bridge observed a message not submitted by either role"
            ));
        }
    }
    atomic_write_json(
        &row_dir.join("flow-verification.json"),
        &json!({
            "schema": 1, "row_id": row.id, "run": run, "verified": true,
            "source_tx": source_tx, "sink_rx": sink_rx, "sink_tx": sink_tx, "source_rx": source_rx,
            "bridge": bridge, "forwarded": forward, "reverse_forwarded": reverse,
            "source_preserves_ids": source_ids, "sink_preserves_ids": sink_ids,
            "contract": "every observed delivery traversed an evidenced bridge with matching bytes/metadata; forwarding bounded by actual sends",
        }),
    )
}
