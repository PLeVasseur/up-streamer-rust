// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Offline CLI replay must reject incomplete evidence without invoking host tools.
use std::process::Command;
use streamer_soak::{JournalWriter, ReplayOptions, Role};

#[test]
fn replay_rejects_incomplete_evidence_without_namespace_or_transport_tools() {
    let directory = tempfile::tempdir().unwrap();
    let options = ReplayOptions {
        run: "offline-fixture".into(),
        source_identity: "a".repeat(64),
        plan_sha256: "b".repeat(64),
        role: Role::PublisherSubscriber,
        queue_capacity: 32,
        pending_limit: 4096,
        require_fifo: true,
    };
    JournalWriter::new(directory.path(), &options, 65536)
        .unwrap()
        .finish(false)
        .unwrap();
    let expected = directory.path().join("expected.json");
    std::fs::write(&expected, serde_json::to_vec(&options).unwrap()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_streamer-transport-test-orchestrator"))
        .arg("--replay-workload")
        .arg(directory.path())
        .arg("--replay-options")
        .arg(expected)
        .current_dir(directory.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("incomplete or unknown journal"));
    assert!(result.stdout.is_empty());
}
