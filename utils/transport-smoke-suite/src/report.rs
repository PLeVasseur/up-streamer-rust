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

use crate::claims::ClaimOutcome;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Version label for serialized smoke scenario reports.
pub const SCENARIO_REPORT_SCHEMA_VERSION: &str = "1.2";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Separates passing execution, validated failure and unavailable prerequisites.
pub enum ScenarioClassification {
    /// The scenario completed and satisfied its claims.
    Pass,
    /// Execution or claims failed; also the deserialization default.
    #[default]
    ValidatedFail,
    /// A prerequisite prevented meaningful execution.
    Blocked,
}

impl ScenarioClassification {
    /// Maps a completed boolean outcome to pass or validated failure.
    pub fn from_pass(pass: bool) -> Self {
        if pass {
            Self::Pass
        } else {
            Self::ValidatedFail
        }
    }

    /// Returns the stable snake-case report label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::ValidatedFail => "validated_fail",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Wall timestamps and elapsed duration for one execution phase.
pub struct PhaseTiming {
    /// Phase label.
    pub phase: String,
    /// Start timestamp in the report's textual time format.
    pub start_ts: String,
    /// Completion timestamp in the report's textual time format.
    pub end_ts: String,
    /// Measured elapsed milliseconds.
    pub duration_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Launch and exit metadata for a scenario-owned child.
pub struct ProcessMetadata {
    /// Process/role label.
    pub name: String,
    /// Spawned process identifier.
    pub pid: i32,
    /// Dedicated process-group identifier.
    pub process_group_id: i32,
    /// Actual command invocation.
    pub command: String,
    /// Child working directory.
    pub workdir: String,
    /// Captured output log path.
    pub log_file: String,
    /// Numeric exit code, if one was observed.
    pub exit_status: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Complete smoke scenario result linking timing, process and claim evidence.
pub struct ScenarioReport {
    /// Serialized report schema label.
    pub schema_version: String,
    /// Executed scenario identifier.
    pub scenario_id: String,
    /// Scenario transport-family label.
    pub transport_family: String,
    /// Whether execution satisfied its required checks.
    pub pass: bool,
    /// Pass, validated failure or blocked classification.
    #[serde(default)]
    pub classification: ScenarioClassification,
    /// Runner exit code associated with the classification.
    pub exit_code: i32,
    /// Timings for the scenario's execution phases.
    pub phase_timings: Vec<PhaseTiming>,
    /// Launch and completion records for owned processes.
    pub processes: Vec<ProcessMetadata>,
    /// Evaluated positive claims.
    pub claim_outcomes: Vec<ClaimOutcome>,
    /// Evaluated forbidden-pattern claims.
    pub forbidden_claim_outcomes: Vec<ClaimOutcome>,
    /// Failure or blocking explanation, if present.
    pub failure_reason: Option<String>,
    /// Command supplied for reproducing this scenario configuration.
    pub repro_command: String,
    /// Directory containing scenario evidence.
    pub artifact_dir: String,
    /// Resolved file-backed claim source, when loading reached that point.
    #[serde(default)]
    pub claims_source_path: Option<String>,
    /// Additional artifact paths referenced by this result.
    #[serde(default)]
    pub supporting_artifacts: Vec<String>,
    /// Scenario start timestamp.
    pub start_ts: String,
    /// Scenario completion timestamp.
    pub end_ts: String,
    /// Total measured elapsed milliseconds.
    pub duration_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Compact outcome for one selected smoke scenario.
pub struct MatrixScenarioSummary {
    /// Scenario identifier.
    pub scenario_id: String,
    /// Whether the scenario passed.
    pub pass: bool,
    /// Pass, validated failure or blocked classification.
    #[serde(default)]
    pub classification: ScenarioClassification,
    /// Scenario runner exit code.
    pub exit_code: i32,
    /// Evidence directory, if one was produced.
    pub artifact_dir: Option<String>,
    /// Failure or blocking explanation, if present.
    pub failure_reason: Option<String>,
    /// Measured scenario elapsed milliseconds.
    pub duration_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Scenario identity paired with its failure explanation.
pub struct FailedScenarioSummary {
    /// Failed scenario identifier.
    pub scenario_id: String,
    /// Explanation retained from execution or validation.
    pub failure_reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Aggregate result for the selected finite smoke scenarios.
pub struct MatrixSummary {
    /// Serialized summary schema label.
    pub schema_version: String,
    /// Requested scenario identifiers in selection order.
    pub selected_scenarios: Vec<String>,
    /// Number of passing scenarios.
    pub pass_count: usize,
    /// Number of scenarios classified as validated failures.
    pub validated_fail_count: usize,
    /// Number of scenarios blocked by prerequisites.
    pub blocked_count: usize,
    /// Total number of nonpassing scenarios.
    pub fail_count: usize,
    /// Measured aggregate elapsed milliseconds.
    pub total_duration_ms: u128,
    /// Per-scenario results.
    pub scenarios: Vec<MatrixScenarioSummary>,
    /// Failed scenario identities and explanations.
    pub failed_scenarios: Vec<FailedScenarioSummary>,
    /// Explanation of the aggregate exit status.
    pub aggregate_exit_rationale: String,
    /// Aggregate run start timestamp.
    pub start_ts: String,
    /// Aggregate run completion timestamp.
    pub end_ts: String,
}

/// Writes JSON and text scenario reports, returning their paths in that order.
pub fn write_scenario_report(
    report: &ScenarioReport,
    artifacts_dir: &Path,
) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(artifacts_dir)
        .with_context(|| format!("unable to create artifact dir {}", artifacts_dir.display()))?;

    let scenario_report_json = artifacts_dir.join("scenario-report.json");
    let scenario_report_txt = artifacts_dir.join("scenario-report.txt");

    let json_payload = serde_json::to_string_pretty(report).context("serialize scenario report")?;
    fs::write(&scenario_report_json, json_payload).with_context(|| {
        format!(
            "unable to write scenario report JSON {}",
            scenario_report_json.display()
        )
    })?;

    fs::write(&scenario_report_txt, render_scenario_summary_text(report)).with_context(|| {
        format!(
            "unable to write scenario report text {}",
            scenario_report_txt.display()
        )
    })?;

    Ok((scenario_report_json, scenario_report_txt))
}

/// Writes JSON and text matrix summaries, returning their paths in that order.
pub fn write_matrix_summary(
    summary: &MatrixSummary,
    artifacts_dir: &Path,
) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(artifacts_dir)
        .with_context(|| format!("unable to create artifact dir {}", artifacts_dir.display()))?;

    let matrix_summary_json = artifacts_dir.join("matrix-summary.json");
    let matrix_summary_txt = artifacts_dir.join("matrix-summary.txt");

    let json_payload = serde_json::to_string_pretty(summary).context("serialize matrix summary")?;
    fs::write(&matrix_summary_json, json_payload).with_context(|| {
        format!(
            "unable to write matrix summary JSON {}",
            matrix_summary_json.display()
        )
    })?;

    fs::write(&matrix_summary_txt, render_matrix_summary_text(summary)).with_context(|| {
        format!(
            "unable to write matrix summary text {}",
            matrix_summary_txt.display()
        )
    })?;

    Ok((matrix_summary_json, matrix_summary_txt))
}

/// Reads and deserializes a scenario report, preserving contextual read/parse errors.
pub fn load_scenario_report(report_path: &Path) -> Result<ScenarioReport> {
    let payload = fs::read_to_string(report_path)
        .with_context(|| format!("unable to read scenario report {}", report_path.display()))?;
    serde_json::from_str(&payload)
        .with_context(|| format!("unable to parse scenario report {}", report_path.display()))
}

/// Formats a UTC timestamp as RFC3339 text.
pub fn timestamp_to_string(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339()
}

fn render_scenario_summary_text(report: &ScenarioReport) -> String {
    let mut lines = vec![
        format!("scenario_id: {}", report.scenario_id),
        format!("transport_family: {}", report.transport_family),
        format!("status: {}", if report.pass { "PASS" } else { "FAIL" }),
        format!("classification: {}", report.classification.as_str()),
        format!("exit_code: {}", report.exit_code),
        format!("artifact_dir: {}", report.artifact_dir),
        format!(
            "claims_source_path: {}",
            report
                .claims_source_path
                .as_deref()
                .unwrap_or("<not available>")
        ),
        format!("duration_ms: {}", report.duration_ms),
        format!("repro_command: {}", report.repro_command),
    ];

    if let Some(reason) = &report.failure_reason {
        lines.push(format!("failure_reason: {reason}"));
    }

    if !report.supporting_artifacts.is_empty() {
        lines.push("supporting_artifacts:".to_string());
        for artifact in &report.supporting_artifacts {
            lines.push(format!("  - {artifact}"));
        }
    }

    lines.push("phase_timings:".to_string());
    for phase in &report.phase_timings {
        lines.push(format!(
            "  - {} (duration_ms={})",
            phase.phase, phase.duration_ms
        ));
    }

    lines.push("claim_outcomes:".to_string());
    for outcome in report
        .claim_outcomes
        .iter()
        .chain(report.forbidden_claim_outcomes.iter())
    {
        lines.push(format!(
            "  - {}: {} (observed={}, min={}, pattern='{}')",
            outcome.claim_id,
            if outcome.pass { "PASS" } else { "FAIL" },
            outcome.observed_count,
            outcome.min_count,
            outcome.pattern
        ));
    }

    lines.join("\n")
}

fn render_matrix_summary_text(summary: &MatrixSummary) -> String {
    let mut lines = vec![
        format!(
            "selected_scenarios: {}",
            summary.selected_scenarios.join(", ")
        ),
        format!("pass_count: {}", summary.pass_count),
        format!("validated_fail_count: {}", summary.validated_fail_count),
        format!("blocked_count: {}", summary.blocked_count),
        format!("fail_count: {}", summary.fail_count),
        format!("total_duration_ms: {}", summary.total_duration_ms),
        format!(
            "aggregate_exit_rationale: {}",
            summary.aggregate_exit_rationale
        ),
    ];

    lines.push("scenarios:".to_string());
    for scenario in &summary.scenarios {
        lines.push(format!(
            "  - {}: {} (classification={}, exit_code={}, duration_ms={}, artifact_dir={})",
            scenario.scenario_id,
            if scenario.pass { "PASS" } else { "FAIL" },
            scenario.classification.as_str(),
            scenario.exit_code,
            scenario.duration_ms,
            scenario
                .artifact_dir
                .as_deref()
                .unwrap_or("<not available>")
        ));
    }

    lines
        .into_iter()
        .chain(summary.failed_scenarios.iter().map(|failed| {
            format!(
                "failed: {} -> {}",
                failed.scenario_id, failed.failure_reason
            )
        }))
        .collect::<Vec<String>>()
        .join("\n")
}
