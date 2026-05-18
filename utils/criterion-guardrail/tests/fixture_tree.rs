/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use criterion_guardrail::{evaluate_guardrail, GuardrailInput, REQUIRED_BENCHMARK_IDS};
use std::{fs, path::Path};
use tempfile::TempDir;

const HEADER: &str = "group,function,value,sample_measured_value,unit,iteration_count\n";

fn write_raw_csv(path: &Path, measured_values: &[u64]) {
    let mut payload = String::from(HEADER);
    for measured_value in measured_values {
        payload.push_str(&format!("routing,bench,id,{measured_value},ns,10\n"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent directories");
    }
    fs::write(path, payload).expect("write fixture csv");
}

#[test]
fn fixture_tree_supports_direct_and_fallback_layouts() {
    let tempdir = TempDir::new().expect("tempdir");
    let criterion_root = tempdir.path().join("criterion");

    for (index, benchmark_id) in REQUIRED_BENCHMARK_IDS.iter().enumerate() {
        write_raw_csv(
            &criterion_root
                .join(benchmark_id)
                .join("ergonomics_baseline")
                .join("raw.csv"),
            &[1000, 1010, 990],
        );

        let candidate_dir = criterion_root
            .join(benchmark_id)
            .join("ergonomics_candidate");
        let candidate_path = if index % 2 == 0 {
            candidate_dir.join("raw.csv")
        } else {
            candidate_dir.join("new").join("raw.csv")
        };
        write_raw_csv(&candidate_path, &[1010, 1020, 1000]);
    }

    let report = evaluate_guardrail(&GuardrailInput {
        criterion_root,
        baseline: "ergonomics_baseline".to_string(),
        candidate: "ergonomics_candidate".to_string(),
        throughput_threshold_pct: 3.0,
        latency_threshold_pct: 5.0,
        alloc_proxy_threshold_pct: 5.0,
    })
    .expect("fixture tree should parse");

    assert!(report.pass);
    assert_eq!(report.results.len(), REQUIRED_BENCHMARK_IDS.len());
}
