// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Explicit finite publication coverage for four additional sink backends.
use super::*;

pub(super) fn rows() -> Result<Vec<MatrixRow>> {
    let rows: Vec<_> = matrix_rows()
        .into_iter()
        .filter(|row| {
            row.source.id == "zenoh-copy-minimized"
                && [
                    "iceoryx2-copy-minimized",
                    "lola-copy-minimized",
                    "mqtt5-classic",
                    "vsomeip-classic",
                ]
                .contains(&row.sink.id)
                && row.role == RoleStyle::PublisherSubscriber
                && row.encoding == WireEncoding::Protobuf
        })
        .collect();
    anyhow::ensure!(
        rows.len() == 4
            && rows
                .iter()
                .all(|row| support_status(row).classification == RowClassification::Pass),
        "finite sinks require four supported canonical protobuf publication rows"
    );
    Ok(rows)
}

pub(super) fn reconstructed(row: &MatrixRow) -> bool {
    row.sink.physical == PhysicalTransport::Vsomeip
}

pub(super) fn oracle(options: &ReplayOptions, row: &MatrixRow, lane: Lane) -> Result<Reconciler> {
    if lane == Lane::FiniteSinks && reconstructed(row) {
        Reconciler::someip_finite(options)
    } else {
        Reconciler::new(options)
    }
}

pub(super) fn replay(
    directory: &Path,
    options: &ReplayOptions,
    row: &MatrixRow,
    lane: Lane,
) -> Result<serde_json::Value> {
    if lane == Lane::FiniteSinks && reconstructed(row) {
        streamer_soak::replay_someip_finite(directory, options)
    } else {
        streamer_soak::replay(directory, options)
    }
}

pub(super) fn audit_backend_inputs(
    directory: &Path,
    inputs: &serde_json::Value,
    row: &MatrixRow,
) -> Result<()> {
    let mut names = Vec::new();
    if row.uses_vsomeip() {
        names.extend([
            "vsomeip-streamer.json",
            "vsomeip-source.json",
            "vsomeip-sink.json",
        ]);
    }
    if row.uses_mqtt5() {
        names.push("mosquitto.conf");
    }
    if names.is_empty() {
        anyhow::ensure!(
            inputs.get("backend_configuration_sha256").is_none(),
            "unexpected backend configuration identities"
        );
        return Ok(());
    }
    let hashes = inputs["backend_configuration_sha256"]
        .as_object()
        .context("missing backend identities")?;
    anyhow::ensure!(hashes.len() == names.len(), "incomplete backend identities");
    for name in names {
        anyhow::ensure!(
            hashes.get(name).and_then(serde_json::Value::as_str)
                == Some(sha256_file(&directory.join(name))?.as_str()),
            "backend configuration changed"
        );
    }
    if row.uses_mqtt5() {
        let ready: serde_json::Value = read(&directory.join("mqtt-readiness.json"))?;
        anyhow::ensure!(
            ready["contract"] == "mqtt5_connect_connack"
                && ready["checks"].as_u64().is_some_and(|n| n > 0),
            "missing native MQTT readiness"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_sinks_require_four_distinct_supported_bridge_paths() {
        let rows = rows().unwrap();
        let sinks: BTreeSet<_> = rows.iter().map(|r| r.sink.id).collect();
        assert_eq!(
            sinks,
            BTreeSet::from([
                "iceoryx2-copy-minimized",
                "lola-copy-minimized",
                "mqtt5-classic",
                "vsomeip-classic"
            ])
        );
        assert!(rows.iter().all(|r| r.source.id == "zenoh-copy-minimized"
            && r.source.physical != r.sink.physical
            && r.role == RoleStyle::PublisherSubscriber
            && r.encoding == WireEncoding::Protobuf
            && support_status(r).classification == RowClassification::Pass));
    }
}
