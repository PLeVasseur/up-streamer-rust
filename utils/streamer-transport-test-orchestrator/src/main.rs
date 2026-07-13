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

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::json;
use signal_hook::consts::SIGINT;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use std::{io::ErrorKind, thread};

const AUTHORITY_A: &str = "authority-a";
const AUTHORITY_B: &str = "authority-b";
const UE_ID: u32 = 0x5BA0;
const UE_VERSION_MAJOR: u8 = 1;
const TOPIC_RESOURCE_ID: u16 = 0x8001;
const METHOD_RESOURCE_ID: u16 = 0x1000;
const ZENOH_ENDPOINT: &str = "tcp/127.0.0.1:7447";
const ZENOH_SOURCE_PORT: u16 = 7447;
const ZENOH_SINK_PORT: u16 = 7448;
const READY_STREAMER: &str = "READY streamer_initialized";
const READY_LISTENER: &str = "READY listener_registered";
const LOLA_MAX_SAMPLES: usize = 16;
const LOLA_SAMPLE_SLOTS: usize = 128;
const LOLA_QUEUE_SIZE: usize = 128;
const LOLA_MAX_SUBSCRIBERS: usize = 8;
const LOLA_LISTENER_STABILIZATION_MS: u64 = 500;
const LOLA_ROW_COOLDOWN_MS: u64 = 1_000;
const LOLA_ROW_RETRIES: usize = 1;
const ZENOH_ROW_COOLDOWN_MS: u64 = 500;
const ZENOH_ROW_RETRIES: usize = 0;
const ICEORYX2_ROOT_PATH: &str = "/tmp/up-streamer-iceoryx2";
const NAMESPACE_TMP_SIZE: &str = "1g";
const NAMESPACE_SHM_SIZE: &str = "2g";
const MQTT_BROKER_PORT: u16 = 1883;
const ZENOH_LISTENER_STABILIZATION_MS: u64 = 1_000;
const VSOMEIP_LISTENER_STABILIZATION_MS: u64 = 1_000;
const VSOMEIP_DUMMY_SERVICE_ID: u16 = 0x7ffe;
const VSOMEIP_DUMMY_INSTANCE_ID: u16 = 0x0001;
const NOTIFICATION_RESOURCE_ID: u16 = 0x8000;
const DDS_PORT_BASE: i32 = 7_400;
const DDS_DOMAIN_GAIN: i32 = 250;
const DDS_MIN_UNPRIVILEGED_PORT: i32 = 1_024;
const DDS_PORT_MODULUS: i32 = 65_536;

#[derive(Debug, Parser)]
#[command(name = "streamer-transport-test-orchestrator")]
#[command(
    about = "Endpoint-profile matrix orchestrator for configurable-streamer plus role binaries"
)]
struct Cli {
    #[arg(long)]
    list: bool,

    #[arg(long, value_name = "FILE")]
    generate_criteria: Option<PathBuf>,

    #[arg(long = "only")]
    only: Vec<String>,

    #[arg(long)]
    skip_build: bool,

    #[arg(long)]
    use_local_sibling_patches: bool,

    #[arg(long)]
    artifacts_root: Option<PathBuf>,

    #[arg(long, default_value_t = 1)]
    send_count: usize,

    #[arg(long, default_value_t = 200)]
    send_interval_ms: u64,

    #[arg(long, default_value_t = 5_000)]
    timeout_ms: u64,

    #[arg(long, default_value_t = 30)]
    scenario_timeout_secs: u64,

    #[arg(long)]
    max_runnable_rows: Option<usize>,

    #[arg(long, default_value_t = 4)]
    jobs: usize,

    #[arg(long, default_value_t = 1)]
    lola_jobs: usize,

    #[arg(long, default_value_t = 1)]
    iterations: usize,

    #[arg(long)]
    disable_row_retries: bool,

    #[arg(long)]
    copy_minimized_sinks_only: bool,

    #[arg(long)]
    criteria: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PhysicalTransport {
    Zenoh,
    Iceoryx2,
    Lola,
    Mqtt5,
    Vsomeip,
    Dds,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EndpointKind {
    Classic,
    OwnedFrame,
    CopyMinimized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RoleStyle {
    PublisherSubscriber,
    NotifierNotifyee,
    ClientServerRpc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireEncoding {
    Native,
    Protobuf,
    Xcdrv2,
    Arrow,
    Omgidl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RowClassification {
    Pass,
    Unsupported,
    Blocked,
    Failed,
}

#[derive(Clone, Copy, Debug)]
struct EndpointProfile {
    id: &'static str,
    physical: PhysicalTransport,
    kind: EndpointKind,
}

#[derive(Clone, Debug)]
struct MatrixRow {
    id: String,
    ordinal: usize,
    source: EndpointProfile,
    sink: EndpointProfile,
    role: RoleStyle,
    encoding: WireEncoding,
}

#[derive(Clone, Debug)]
struct LolaEndpointInfo {
    instance_specifier: String,
    service_type: String,
    event_name: String,
    response_instance_specifier: Option<String>,
    response_service_type: Option<String>,
    response_event_name: Option<String>,
}

struct LolaManifestPaths {
    streamer: PathBuf,
    active: PathBuf,
    passive: PathBuf,
}

impl LolaManifestPaths {
    fn new(row_dir: &Path) -> Self {
        Self {
            streamer: row_dir.join("mw_com_config_lola_streamer.json"),
            active: row_dir.join("mw_com_config_lola_active.json"),
            passive: row_dir.join("mw_com_config_lola_passive.json"),
        }
    }

    fn role(&self, active: bool) -> &Path {
        if active {
            &self.active
        } else {
            &self.passive
        }
    }
}

#[derive(Debug)]
struct RunningProcess {
    name: String,
    log_path: PathBuf,
    child: Child,
}

struct ZenohConfigPaths {
    source_router: PathBuf,
    source_client: PathBuf,
    sink_router: PathBuf,
    sink_client: PathBuf,
}

struct VsomeipConfigPaths {
    streamer: PathBuf,
    source: PathBuf,
    sink: PathBuf,
}

impl VsomeipConfigPaths {
    fn new(row_dir: &Path) -> Self {
        Self {
            streamer: row_dir.join("vsomeip-streamer.json"),
            source: row_dir.join("vsomeip-source.json"),
            sink: row_dir.join("vsomeip-sink.json"),
        }
    }

    fn role_config(&self, active: bool) -> &Path {
        if active {
            &self.source
        } else {
            &self.sink
        }
    }
}

impl ZenohConfigPaths {
    fn new(row_dir: &Path) -> Self {
        Self {
            source_router: row_dir.join("zenoh-source-router.json5"),
            source_client: row_dir.join("zenoh-source-client.json5"),
            sink_router: row_dir.join("zenoh-sink-router.json5"),
            sink_client: row_dir.join("zenoh-sink-client.json5"),
        }
    }

    fn router_for_side(&self, side: &str) -> &Path {
        match side {
            "source" => &self.source_router,
            "sink" => &self.sink_router,
            _ => unreachable!("unknown side"),
        }
    }

    fn client_for_active(&self, active: bool) -> &Path {
        if active {
            &self.source_client
        } else {
            &self.sink_client
        }
    }

    fn client_for_side(&self, side: &str) -> &Path {
        match side {
            "source" => &self.source_client,
            "sink" => &self.sink_client,
            _ => unreachable!("unknown side"),
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        terminate(self);
    }
}

#[derive(Serialize)]
struct MatrixSummary {
    schema_version: &'static str,
    generated_at: String,
    row_count: usize,
    pass_count: usize,
    unsupported_count: usize,
    blocked_count: usize,
    failed_count: usize,
    iterations: usize,
    jobs: usize,
    lola_jobs: usize,
    retry_policy: RetryPolicySummary,
    retried_row_count: usize,
    max_retries_consumed: usize,
    artifacts_root: String,
    rows: Vec<RowResult>,
}

#[derive(Serialize)]
struct RetryPolicySummary {
    disabled: bool,
    lola_max_retries: usize,
    zenoh_max_retries: usize,
    default_max_retries: usize,
}

#[derive(Serialize)]
struct RowResult {
    row_id: String,
    source_profile: String,
    sink_profile: String,
    source_transport: PhysicalTransport,
    sink_transport: PhysicalTransport,
    source_endpoint_kind: EndpointKind,
    sink_endpoint_kind: EndpointKind,
    role: RoleStyle,
    encoding: WireEncoding,
    iteration: usize,
    attempts_used: usize,
    retries_consumed: usize,
    classification: RowClassification,
    failure_phase: Option<&'static str>,
    reason: String,
    artifact_dir: Option<String>,
    config_path: Option<String>,
    lola_manifest_path: Option<String>,
    lola_manifest_paths: BTreeMap<String, String>,
    logs: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize)]
struct MatrixCriteria {
    expected: ExpectedCounts,
    unsupported_reason_allowlist: Vec<String>,
    retry: RetryCriteria,
}

#[derive(Deserialize, Serialize)]
struct ExpectedCounts {
    pass: usize,
    unsupported: usize,
    blocked: usize,
    failed: usize,
}

#[derive(Deserialize, Serialize)]
struct RetryCriteria {
    max_retries_lola_rows: usize,
    max_retries_zenoh_rows: usize,
    all_other_rows: usize,
    max_retried_rows: usize,
    max_retries_consumed: usize,
}

#[derive(Serialize)]
struct CriteriaResult {
    verdict: &'static str,
    criteria_path: String,
    errors: Vec<String>,
}

#[derive(Clone, Default)]
struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    #[cfg(test)]
    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(anyhow!("cancelled by Ctrl-C"))
        } else {
            Ok(())
        }
    }
}

struct ArtifactRootLock {
    path: PathBuf,
    _file: File,
}

impl ArtifactRootLock {
    fn acquire(root: &Path) -> Result<Self> {
        match fs::create_dir(root) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let mut entries = fs::read_dir(root)
                    .with_context(|| format!("unable to read {}", root.display()))?;
                if entries.next().transpose()?.is_some() {
                    return Err(anyhow!(
                        "artifact root {} already exists and is not empty",
                        root.display()
                    ));
                }
            }
            Err(error) => {
                return Err(error).with_context(|| format!("unable to create {}", root.display()));
            }
        }
        let path = root.join(".streamer-transport-test.lock");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("artifact root {} is already in use", root.display()))?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for ArtifactRootLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct ScheduledTask<T> {
    slot: usize,
    lola_sensitive: bool,
    payload: T,
}

struct SchedulerState<T> {
    pending: VecDeque<ScheduledTask<T>>,
    active_lola: usize,
    stopped: bool,
}

struct RowExecution {
    row: MatrixRow,
    iteration: usize,
}

struct MatrixPlan {
    slot_count: usize,
    runnable: Vec<ScheduledTask<RowExecution>>,
    completed: Vec<(usize, RowResult)>,
}

fn main() -> Result<()> {
    let succeeded = run(Cli::parse())?;
    if !succeeded {
        std::process::exit(1);
    }
    Ok(())
}

fn run(cli: Cli) -> Result<bool> {
    validate_cli(&cli)?;
    let cancellation = Cancellation::default();
    signal_hook::flag::register(SIGINT, Arc::clone(&cancellation.0))
        .context("unable to install Ctrl-C handler")?;

    let repo_root = repo_root()?;
    let rows = matrix_rows();

    if let Some(path) = &cli.generate_criteria {
        let criteria = derived_criteria(&rows);
        fs::write(path, serde_json::to_string_pretty(&criteria)?)
            .with_context(|| format!("unable to write {}", path.display()))?;
        println!("STREAMER_TRANSPORT_TEST_CRITERIA_JSON={}", path.display());
        return Ok(true);
    }

    if cli.list {
        for row in &rows {
            let support = support_status(row);
            println!(
                "{}\t{:?}\t{} -> {}\t{:?}",
                row.id, support.classification, row.source.id, row.sink.id, support.reason
            );
        }
        return Ok(true);
    }

    let mut selected_rows = select_rows(rows, &cli.only)?;
    selected_rows = filter_copy_minimized_sinks(selected_rows, cli.copy_minimized_sinks_only);
    if selected_rows.is_empty() {
        return Err(anyhow!("no matrix rows matched the requested selection"));
    }
    let plan = plan_rows(&selected_rows, cli.iterations, cli.max_runnable_rows);
    let artifacts_root = cli
        .artifacts_root
        .clone()
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                repo_root.join(path)
            }
        })
        .unwrap_or_else(|| generated_artifacts_root(&repo_root, Utc::now(), std::process::id()));
    if let Some(parent) = artifacts_root.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }
    let _artifact_lock = ArtifactRootLock::acquire(&artifacts_root)?;

    if !cli.skip_build {
        build_required_binaries(&repo_root, &selected_rows, cli.use_local_sibling_patches)?;
    }
    cancellation.check()?;

    let MatrixPlan {
        slot_count,
        runnable,
        mut completed,
    } = plan;
    let executed = run_bounded(
        runnable,
        cli.jobs,
        cli.lola_jobs,
        &cancellation,
        |execution| {
            println!(
                "RUNNING {} iteration={}",
                execution.row.id, execution.iteration
            );
            run_row(
                &repo_root,
                &artifacts_root,
                &execution.row,
                &cli,
                execution.iteration,
                &cancellation,
            )
        },
    )?;
    completed.extend(executed);
    let results = canonical_order(slot_count, completed)?;

    let retried_row_count = results
        .iter()
        .filter(|row| row.retries_consumed > 0)
        .count();
    let max_retries_consumed = results
        .iter()
        .map(|row| row.retries_consumed)
        .max()
        .unwrap_or_default();
    let summary = MatrixSummary {
        schema_version: "1.2",
        generated_at: Utc::now().to_rfc3339(),
        row_count: results.len(),
        pass_count: results
            .iter()
            .filter(|row| row.classification == RowClassification::Pass)
            .count(),
        unsupported_count: results
            .iter()
            .filter(|row| row.classification == RowClassification::Unsupported)
            .count(),
        blocked_count: results
            .iter()
            .filter(|row| row.classification == RowClassification::Blocked)
            .count(),
        failed_count: results
            .iter()
            .filter(|row| row.classification == RowClassification::Failed)
            .count(),
        iterations: cli.iterations,
        jobs: cli.jobs,
        lola_jobs: cli.lola_jobs,
        retry_policy: RetryPolicySummary {
            disabled: cli.disable_row_retries,
            lola_max_retries: if cli.disable_row_retries {
                0
            } else {
                LOLA_ROW_RETRIES
            },
            zenoh_max_retries: if cli.disable_row_retries {
                0
            } else {
                ZENOH_ROW_RETRIES
            },
            default_max_retries: 0,
        },
        retried_row_count,
        max_retries_consumed,
        artifacts_root: artifacts_root.display().to_string(),
        rows: results,
    };
    let summary_path = artifacts_root.join("matrix-summary.json");
    fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)
        .with_context(|| format!("unable to write {}", summary_path.display()))?;

    let mut criteria_failed = false;
    if let Some(criteria_path) = &cli.criteria {
        let criteria: MatrixCriteria = serde_json::from_slice(
            &fs::read(criteria_path)
                .with_context(|| format!("unable to read {}", criteria_path.display()))?,
        )
        .with_context(|| format!("invalid matrix criteria {}", criteria_path.display()))?;
        let errors = validate_criteria(&summary, &criteria);
        criteria_failed = !errors.is_empty();
        let result = CriteriaResult {
            verdict: if criteria_failed { "FAIL" } else { "PASS" },
            criteria_path: criteria_path.display().to_string(),
            errors,
        };
        let result_path = artifacts_root.join("matrix-criteria-result.json");
        fs::write(&result_path, serde_json::to_string_pretty(&result)?)
            .with_context(|| format!("unable to write {}", result_path.display()))?;
        println!(
            "STREAMER_TRANSPORT_TEST_CRITERIA_JSON={}",
            result_path.display()
        );
    }

    println!(
        "STREAMER_TRANSPORT_TEST_SUMMARY_JSON={}",
        summary_path.display()
    );
    println!(
        "STREAMER_TRANSPORT_TEST_COUNTS pass={} unsupported={} blocked={} failed={} retried_rows={} max_retries_consumed={}",
        summary.pass_count,
        summary.unsupported_count,
        summary.blocked_count,
        summary.failed_count,
        summary.retried_row_count,
        summary.max_retries_consumed
    );

    Ok(summary.failed_count == 0 && summary.blocked_count == 0 && !criteria_failed)
}

fn validate_cli(cli: &Cli) -> Result<()> {
    if cli.iterations == 0 {
        return Err(anyhow!("--iterations must be greater than zero"));
    }
    validate_concurrency(cli.jobs, cli.lola_jobs)?;
    let mut seen = BTreeSet::new();
    for id in &cli.only {
        if !seen.insert(id) {
            return Err(anyhow!("duplicate --only matrix row id {id}"));
        }
    }
    Ok(())
}

fn validate_concurrency(jobs: usize, lola_jobs: usize) -> Result<()> {
    if jobs == 0 {
        return Err(anyhow!("--jobs must be greater than zero"));
    }
    if lola_jobs == 0 {
        return Err(anyhow!("--lola-jobs must be greater than zero"));
    }
    if lola_jobs > jobs {
        return Err(anyhow!("--lola-jobs must not exceed --jobs"));
    }
    Ok(())
}

fn generated_artifacts_root(repo_root: &Path, now: DateTime<Utc>, process_id: u32) -> PathBuf {
    repo_root
        .join("target")
        .join("streamer-transport-test")
        .join(format!(
            "{}-pid{process_id}",
            now.format("%Y%m%dT%H%M%S%.6fZ")
        ))
}

fn plan_rows(
    rows: &[MatrixRow],
    iterations: usize,
    max_runnable_rows: Option<usize>,
) -> MatrixPlan {
    let mut runnable = Vec::new();
    let mut completed = Vec::new();
    let mut runnable_seen = 0_usize;
    let mut slot = 0_usize;

    for iteration in 1..=iterations {
        for row in rows {
            let support = support_status(row);
            if support.classification != RowClassification::Pass {
                let mut result = row_result(
                    row,
                    support.classification,
                    support.reason,
                    None,
                    None,
                    None,
                    BTreeMap::new(),
                );
                result.iteration = iteration;
                completed.push((slot, result));
            } else {
                runnable_seen += 1;
                if max_runnable_rows.is_some_and(|max| runnable_seen > max) {
                    let max = max_runnable_rows.expect("maximum was checked above");
                    let mut result = row_result(
                        row,
                        RowClassification::Blocked,
                        format!("not executed because --max-runnable-rows={max} was reached"),
                        None,
                        None,
                        None,
                        BTreeMap::new(),
                    );
                    result.iteration = iteration;
                    completed.push((slot, result));
                } else {
                    runnable.push(ScheduledTask {
                        slot,
                        lola_sensitive: row.uses_lola(),
                        payload: RowExecution {
                            row: row.clone(),
                            iteration,
                        },
                    });
                }
            }
            slot += 1;
        }
    }

    MatrixPlan {
        slot_count: slot,
        runnable,
        completed,
    }
}

fn run_bounded<T, R, F>(
    tasks: Vec<ScheduledTask<T>>,
    jobs: usize,
    lola_jobs: usize,
    cancellation: &Cancellation,
    run_task: F,
) -> Result<Vec<(usize, R)>>
where
    T: Send,
    R: Send,
    F: Fn(T) -> Result<R> + Sync,
{
    validate_concurrency(jobs, lola_jobs)?;
    let shared = Arc::new((
        Mutex::new(SchedulerState {
            pending: tasks.into(),
            active_lola: 0,
            stopped: false,
        }),
        Condvar::new(),
    ));
    let (sender, receiver) = mpsc::channel();

    thread::scope(|scope| -> Result<()> {
        let mut workers = Vec::with_capacity(jobs);
        for _ in 0..jobs {
            let shared = Arc::clone(&shared);
            let sender = sender.clone();
            let run_task = &run_task;
            workers.push(scope.spawn(move || loop {
                let task = {
                    let (state_lock, wake) = &*shared;
                    let mut state = state_lock.lock().expect("scheduler state mutex poisoned");
                    loop {
                        if cancellation.is_cancelled() {
                            state.stopped = true;
                        }
                        if state.stopped || state.pending.is_empty() {
                            return;
                        }
                        let eligible = if jobs == 1 {
                            Some(0)
                        } else {
                            let regular_pending =
                                state.pending.iter().any(|task| !task.lola_sensitive);
                            let lola_capacity = if regular_pending {
                                lola_jobs.min(jobs - 1)
                            } else {
                                lola_jobs
                            };
                            if state.active_lola < lola_capacity {
                                state.pending.iter().position(|task| task.lola_sensitive)
                            } else {
                                None
                            }
                            .or_else(|| state.pending.iter().position(|task| !task.lola_sensitive))
                        };
                        if let Some(index) = eligible {
                            let task = state
                                .pending
                                .remove(index)
                                .expect("eligible scheduler task disappeared");
                            if task.lola_sensitive {
                                state.active_lola += 1;
                            }
                            break task;
                        }
                        let (next_state, _) = wake
                            .wait_timeout(state, Duration::from_millis(50))
                            .expect("scheduler state mutex poisoned");
                        state = next_state;
                    }
                };

                let lola_sensitive = task.lola_sensitive;
                let slot = task.slot;
                let result = catch_unwind(AssertUnwindSafe(|| run_task(task.payload)))
                    .unwrap_or_else(|_| Err(anyhow!("matrix scheduler task panicked")));
                let failed = result.is_err();
                {
                    let (state_lock, wake) = &*shared;
                    let mut state = state_lock.lock().expect("scheduler state mutex poisoned");
                    if lola_sensitive {
                        state.active_lola -= 1;
                    }
                    if failed {
                        state.stopped = true;
                    }
                    wake.notify_all();
                }
                if sender.send((slot, result)).is_err() {
                    return;
                }
            }));
        }
        drop(sender);
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow!("matrix scheduler worker panicked"))?;
        }
        Ok(())
    })?;

    let mut completed = Vec::new();
    let mut first_error = None;
    for (slot, result) in receiver {
        match result {
            Ok(result) => completed.push((slot, result)),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    cancellation.check()?;
    Ok(completed)
}

fn canonical_order<T>(slot_count: usize, completed: Vec<(usize, T)>) -> Result<Vec<T>> {
    let mut slots: Vec<Option<T>> = std::iter::repeat_with(|| None).take(slot_count).collect();
    for (slot, result) in completed {
        let destination = slots
            .get_mut(slot)
            .ok_or_else(|| anyhow!("result has invalid canonical slot {slot}"))?;
        if destination.replace(result).is_some() {
            return Err(anyhow!("canonical slot {slot} completed more than once"));
        }
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(slot, result)| {
            result.ok_or_else(|| anyhow!("canonical slot {slot} did not complete"))
        })
        .collect()
}

fn validate_criteria(summary: &MatrixSummary, criteria: &MatrixCriteria) -> Vec<String> {
    let mut errors = Vec::new();
    let actual = (
        summary.pass_count,
        summary.unsupported_count,
        summary.blocked_count,
        summary.failed_count,
    );
    let expected = (
        criteria.expected.pass,
        criteria.expected.unsupported,
        criteria.expected.blocked,
        criteria.expected.failed,
    );
    if actual != expected {
        errors.push(format!(
            "classification counts {actual:?} do not match expected {expected:?}"
        ));
    }
    if summary.row_count != summary.rows.len() {
        errors.push(format!(
            "declared row_count {} does not match rows length {}",
            summary.row_count,
            summary.rows.len()
        ));
    }
    for row in summary
        .rows
        .iter()
        .filter(|row| row.classification == RowClassification::Unsupported)
    {
        if !criteria
            .unsupported_reason_allowlist
            .iter()
            .any(|allowed| row.reason.contains(allowed))
        {
            errors.push(format!(
                "unsupported row {} has unapproved reason: {}",
                row.row_id, row.reason
            ));
        }
    }
    let retry = &criteria.retry;
    if summary.retry_policy.lola_max_retries != retry.max_retries_lola_rows
        || summary.retry_policy.zenoh_max_retries != retry.max_retries_zenoh_rows
        || summary.retry_policy.default_max_retries != retry.all_other_rows
    {
        errors.push(format!(
            "configured retry policy lola/zenoh/default={}/{}/{} does not match criteria {}/{}/{}",
            summary.retry_policy.lola_max_retries,
            summary.retry_policy.zenoh_max_retries,
            summary.retry_policy.default_max_retries,
            retry.max_retries_lola_rows,
            retry.max_retries_zenoh_rows,
            retry.all_other_rows
        ));
    }
    if summary.retried_row_count > retry.max_retried_rows {
        errors.push(format!(
            "retried row count {} exceeds criteria maximum {}",
            summary.retried_row_count, retry.max_retried_rows
        ));
    }
    if summary.max_retries_consumed > retry.max_retries_consumed {
        errors.push(format!(
            "maximum retries consumed {} exceeds criteria maximum {}",
            summary.max_retries_consumed, retry.max_retries_consumed
        ));
    }
    errors
}

fn derived_criteria(rows: &[MatrixRow]) -> MatrixCriteria {
    let pass = rows
        .iter()
        .filter(|row| support_status(row).classification == RowClassification::Pass)
        .count();
    MatrixCriteria {
        expected: ExpectedCounts {
            pass,
            unsupported: rows.len() - pass,
            blocked: 0,
            failed: 0,
        },
        unsupported_reason_allowlist: vec![
            "source and sink endpoint profiles are identical".to_string(),
            "Arrow and OMGIDL are not implemented by MQTT5 or vSomeIP classic role binaries"
                .to_string(),
        ],
        retry: RetryCriteria {
            max_retries_lola_rows: LOLA_ROW_RETRIES,
            max_retries_zenoh_rows: ZENOH_ROW_RETRIES,
            all_other_rows: 0,
            max_retried_rows: 0,
            max_retries_consumed: 0,
        },
    }
}

fn profiles() -> Vec<EndpointProfile> {
    vec![
        EndpointProfile {
            id: "zenoh-classic",
            physical: PhysicalTransport::Zenoh,
            kind: EndpointKind::Classic,
        },
        EndpointProfile {
            id: "mqtt5-classic",
            physical: PhysicalTransport::Mqtt5,
            kind: EndpointKind::Classic,
        },
        EndpointProfile {
            id: "vsomeip-classic",
            physical: PhysicalTransport::Vsomeip,
            kind: EndpointKind::Classic,
        },
        EndpointProfile {
            id: "zenoh-owned-frame",
            physical: PhysicalTransport::Zenoh,
            kind: EndpointKind::OwnedFrame,
        },
        EndpointProfile {
            id: "zenoh-copy-minimized",
            physical: PhysicalTransport::Zenoh,
            kind: EndpointKind::CopyMinimized,
        },
        EndpointProfile {
            id: "iceoryx2-owned-frame",
            physical: PhysicalTransport::Iceoryx2,
            kind: EndpointKind::OwnedFrame,
        },
        EndpointProfile {
            id: "iceoryx2-copy-minimized",
            physical: PhysicalTransport::Iceoryx2,
            kind: EndpointKind::CopyMinimized,
        },
        EndpointProfile {
            id: "lola-owned-frame",
            physical: PhysicalTransport::Lola,
            kind: EndpointKind::OwnedFrame,
        },
        EndpointProfile {
            id: "lola-copy-minimized",
            physical: PhysicalTransport::Lola,
            kind: EndpointKind::CopyMinimized,
        },
        EndpointProfile {
            id: "dds-classic",
            physical: PhysicalTransport::Dds,
            kind: EndpointKind::Classic,
        },
        EndpointProfile {
            id: "dds-owned-frame",
            physical: PhysicalTransport::Dds,
            kind: EndpointKind::OwnedFrame,
        },
        EndpointProfile {
            id: "dds-copy-minimized",
            physical: PhysicalTransport::Dds,
            kind: EndpointKind::CopyMinimized,
        },
    ]
}

fn matrix_rows() -> Vec<MatrixRow> {
    let profiles = profiles();
    let roles = [
        RoleStyle::PublisherSubscriber,
        RoleStyle::NotifierNotifyee,
        RoleStyle::ClientServerRpc,
    ];
    let encodings = [
        WireEncoding::Native,
        WireEncoding::Protobuf,
        WireEncoding::Xcdrv2,
        WireEncoding::Arrow,
        WireEncoding::Omgidl,
    ];
    let mut rows = Vec::new();
    let mut ordinal = 0_usize;
    for source in &profiles {
        for sink in &profiles {
            for role in roles {
                for encoding in encodings {
                    let id = format!(
                        "matrix-{}-to-{}-{}-{}",
                        source.id,
                        sink.id,
                        role.id(),
                        encoding.id()
                    );
                    rows.push(MatrixRow {
                        id,
                        ordinal,
                        source: *source,
                        sink: *sink,
                        role,
                        encoding,
                    });
                    ordinal += 1;
                }
            }
        }
    }
    rows
}

struct SupportStatus {
    classification: RowClassification,
    reason: String,
}

fn support_status(row: &MatrixRow) -> SupportStatus {
    if row.source.id == row.sink.id {
        return unsupported(
            "source and sink endpoint profiles are identical; no bridge boundary is under test",
        );
    }
    if matches!(row.encoding, WireEncoding::Arrow | WireEncoding::Omgidl)
        && [row.source, row.sink].into_iter().any(|profile| {
            profile.kind == EndpointKind::Classic
                && matches!(
                    profile.physical,
                    PhysicalTransport::Mqtt5 | PhysicalTransport::Vsomeip
                )
        })
    {
        return unsupported(
            "Arrow and OMGIDL are not implemented by MQTT5 or vSomeIP classic role binaries",
        );
    }
    if row.source.kind == EndpointKind::Classic || row.sink.kind == EndpointKind::Classic {
        return SupportStatus {
            classification: RowClassification::Pass,
            reason:
                "classic row has configurable-streamer bridge route and stand-alone role binaries"
                    .to_string(),
        };
    }
    if !row.source.kind.is_selected_wire() || !row.sink.kind.is_selected_wire() {
        return unsupported("row does not resolve to selected-wire endpoint profiles");
    }
    SupportStatus {
        classification: RowClassification::Pass,
        reason: "selected-wire row has concrete configurable-streamer route and stand-alone role binaries".to_string(),
    }
}

fn unsupported(reason: &str) -> SupportStatus {
    SupportStatus {
        classification: RowClassification::Unsupported,
        reason: reason.to_string(),
    }
}

fn select_rows(rows: Vec<MatrixRow>, only: &[String]) -> Result<Vec<MatrixRow>> {
    if only.is_empty() {
        return Ok(rows);
    }
    let mut selected = Vec::new();
    for id in only {
        let row = rows
            .iter()
            .find(|row| row.id == *id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown matrix row id {id}"))?;
        selected.push(row);
    }
    Ok(selected)
}

fn filter_copy_minimized_sinks(
    rows: Vec<MatrixRow>,
    copy_minimized_sinks_only: bool,
) -> Vec<MatrixRow> {
    if copy_minimized_sinks_only {
        rows.into_iter()
            .filter(|row| row.sink.kind == EndpointKind::CopyMinimized)
            .collect()
    } else {
        rows
    }
}

fn row_run_id(row: &MatrixRow, iterations: usize, iteration: usize) -> String {
    if iterations == 1 {
        row.id.clone()
    } else {
        format!("{}-iteration{iteration:03}", row.id)
    }
}

fn run_row(
    repo_root: &Path,
    artifacts_root: &Path,
    row: &MatrixRow,
    cli: &Cli,
    iteration: usize,
    cancellation: &Cancellation,
) -> Result<RowResult> {
    let retries = if cli.disable_row_retries {
        0
    } else if row.uses_lola() {
        LOLA_ROW_RETRIES
    } else if row.uses_zenoh() {
        ZENOH_ROW_RETRIES
    } else {
        0
    };
    let max_attempts = retries + 1;
    let mut last_result = None;
    for attempt in 0..max_attempts {
        cancellation.check()?;
        let mut result = run_row_attempt(
            repo_root,
            artifacts_root,
            row,
            cli,
            iteration,
            attempt,
            cancellation,
        )?;
        cancellation.check()?;
        result.iteration = iteration;
        result.attempts_used = attempt + 1;
        result.retries_consumed = attempt;
        if result.classification != RowClassification::Failed {
            if attempt > 0 {
                result.reason = format!("{} after retry {attempt}", result.reason);
            }
            return Ok(result);
        }
        last_result = Some(result);
        if attempt + 1 < max_attempts {
            let cooldown_ms = if row.uses_lola() {
                LOLA_ROW_COOLDOWN_MS
            } else {
                ZENOH_ROW_COOLDOWN_MS
            };
            cancellable_sleep(Duration::from_millis(cooldown_ms), cancellation)?;
        }
    }
    Ok(last_result.expect("at least one row attempt ran"))
}

fn run_row_attempt(
    repo_root: &Path,
    artifacts_root: &Path,
    row: &MatrixRow,
    cli: &Cli,
    iteration: usize,
    attempt: usize,
    cancellation: &Cancellation,
) -> Result<RowResult> {
    let run_id = row_run_id(row, cli.iterations, iteration);
    let row_dir = if attempt == 0 {
        artifacts_root.join(&run_id)
    } else {
        artifacts_root.join(format!("{run_id}-retry{attempt}"))
    };
    fs::create_dir_all(&row_dir)
        .with_context(|| format!("unable to create {}", row_dir.display()))?;
    let mut logs = BTreeMap::new();
    let lola_run_namespace = lola_run_namespace(&row_dir, row);

    let lola_manifest_paths = if row.uses_lola() {
        let paths = LolaManifestPaths::new(&row_dir);
        for (path, role) in [
            (&paths.streamer, 1),
            (&paths.active, 2),
            (&paths.passive, 3),
        ] {
            write_lola_manifest(
                row,
                path,
                &lola_run_namespace,
                lola_application_id(&lola_run_namespace, role),
            )?;
        }
        Some(paths)
    } else {
        None
    };
    let lola_manifest_path = lola_manifest_paths
        .as_ref()
        .map(|paths| paths.streamer.clone());
    let lola_bridge_lib_dir = match detect_lola_bridge_lib_dir(repo_root) {
        Ok(path) => Some(path),
        Err(error) if row.uses_lola() => {
            return Ok(row_result(
                row,
                RowClassification::Blocked,
                format!(
                    "LoLa row {} requires libup_lola_bridge.so; build LoLa examples first or set LD_LIBRARY_PATH: {error:#}",
                    row.id
                ),
                Some(row_dir),
                None,
                lola_manifest_path,
                logs,
            ));
        }
        Err(_) => None,
    };
    let vsomeip_lib_dir = match detect_vsomeip_lib_dir(repo_root) {
        Ok(path) => Some(path),
        Err(error) if row.uses_vsomeip() => {
            return Ok(row_result(
                row,
                RowClassification::Blocked,
                format!(
                    "vSomeIP row {} requires libvsomeip3.so.3 on LD_LIBRARY_PATH: {error:#}",
                    row.id
                ),
                Some(row_dir),
                None,
                lola_manifest_path,
                logs,
            ));
        }
        Err(_) => None,
    };
    let iceoryx2_root = if row.uses_iceoryx2() {
        Some(PathBuf::from(ICEORYX2_ROOT_PATH))
    } else {
        None
    };
    let process_env = row_env(
        row,
        iceoryx2_root.as_deref(),
        lola_bridge_lib_dir.as_deref(),
        vsomeip_lib_dir.as_deref(),
    );
    let mut namespace = start_namespace_holder(&row_dir, cancellation)?;
    logs.insert(
        "namespace_holder".to_string(),
        namespace.log_path.display().to_string(),
    );

    let mut mqtt_broker = if row.uses_mqtt5() {
        let broker = start_mqtt_broker(&row_dir, &process_env, &namespace, cancellation)?;
        logs.insert(
            "mqtt_broker".to_string(),
            broker.log_path.display().to_string(),
        );
        Some(broker)
    } else {
        None
    };

    let config_path = row_dir.join("configurable-streamer.json");
    let zenoh_config_paths = ZenohConfigPaths::new(&row_dir);
    let vsomeip_config_paths = VsomeipConfigPaths::new(&row_dir);
    write_zenoh_router_config(&zenoh_config_paths.source_router, ZENOH_SOURCE_PORT)?;
    write_zenoh_router_config(&zenoh_config_paths.sink_router, ZENOH_SINK_PORT)?;
    write_zenoh_client_config(&zenoh_config_paths.source_client, ZENOH_SOURCE_PORT)?;
    write_zenoh_client_config(&zenoh_config_paths.sink_client, ZENOH_SINK_PORT)?;
    if row.uses_vsomeip() {
        write_vsomeip_configs(row, &vsomeip_config_paths)?;
    }
    write_config(
        repo_root,
        row,
        &config_path,
        &zenoh_config_paths,
        &vsomeip_config_paths,
        lola_manifest_paths
            .as_ref()
            .map(|paths| paths.streamer.as_path()),
        &lola_run_namespace,
    )?;

    let mut streamer = spawn_process(
        "streamer",
        &target_debug_binary(repo_root, "configurable-streamer"),
        &["--config".to_string(), config_path.display().to_string()],
        &repo_root.join("configurable-streamer"),
        &process_env,
        &row_dir,
        Some(&namespace),
    )?;
    logs.insert(
        "streamer".to_string(),
        streamer.log_path.display().to_string(),
    );

    let started = Instant::now();
    let result = (|| -> Result<()> {
        wait_for_marker(
            &mut streamer,
            READY_STREAMER,
            Duration::from_secs(10),
            cancellation,
        )?;

        let passive_spec = role_command(
            row,
            false,
            &zenoh_config_paths,
            &vsomeip_config_paths,
            lola_manifest_paths.as_ref().map(|paths| paths.role(false)),
            &lola_run_namespace,
            cli,
        )?;
        let mut passive = spawn_process(
            "passive",
            &target_debug_binary(repo_root, &passive_spec.binary),
            &passive_spec.args,
            repo_root,
            &process_env,
            &row_dir,
            Some(&namespace),
        )?;
        logs.insert(
            "passive".to_string(),
            passive.log_path.display().to_string(),
        );
        wait_for_marker(
            &mut passive,
            READY_LISTENER,
            Duration::from_secs(10),
            cancellation,
        )?;
        if row.uses_lola() {
            cancellable_sleep(
                Duration::from_millis(LOLA_LISTENER_STABILIZATION_MS),
                cancellation,
            )?;
        }
        if row.sink.physical == PhysicalTransport::Zenoh {
            cancellable_sleep(
                Duration::from_millis(ZENOH_LISTENER_STABILIZATION_MS),
                cancellation,
            )?;
        }
        if row.sink.physical == PhysicalTransport::Vsomeip {
            cancellable_sleep(
                Duration::from_millis(VSOMEIP_LISTENER_STABILIZATION_MS),
                cancellation,
            )?;
        }

        let active_spec = role_command(
            row,
            true,
            &zenoh_config_paths,
            &vsomeip_config_paths,
            lola_manifest_paths.as_ref().map(|paths| paths.role(true)),
            &lola_run_namespace,
            cli,
        )?;
        let mut active = spawn_process(
            "active",
            &target_debug_binary(repo_root, &active_spec.binary),
            &active_spec.args,
            repo_root,
            &process_env,
            &row_dir,
            Some(&namespace),
        )?;
        logs.insert("active".to_string(), active.log_path.display().to_string());

        wait_for_exit(
            &mut active,
            remaining_timeout(started, cli.scenario_timeout_secs, "active")?,
            cancellation,
        )?;
        assert_success(&mut active)?;
        if !row.uses_classic() {
            wait_for_exit(
                &mut passive,
                remaining_timeout(started, cli.scenario_timeout_secs, "passive")?,
                cancellation,
            )?;
            assert_success(&mut passive)?;
        } else {
            wait_for_any_marker(
                &passive.log_path,
                passive_observation_markers(row),
                remaining_timeout(
                    started,
                    cli.scenario_timeout_secs,
                    "classic passive observation",
                )?,
                cancellation,
            )?;
        }

        validate_flow_logs(row, &active.log_path, &passive.log_path)?;
        terminate(&mut passive);
        terminate(&mut active);
        Ok(())
    })();

    terminate(&mut streamer);
    if let Some(broker) = &mut mqtt_broker {
        terminate(broker);
    }
    terminate(&mut namespace);
    if row.uses_lola() {
        cancellable_sleep(Duration::from_millis(LOLA_ROW_COOLDOWN_MS), cancellation)?;
    }

    let mut result = match result {
        Ok(()) => row_result(
            row,
            RowClassification::Pass,
            "payload proof completed through configurable-streamer and stand-alone role binaries"
                .to_string(),
            Some(row_dir),
            Some(config_path),
            lola_manifest_path.clone(),
            logs,
        ),
        Err(error) => row_result(
            row,
            RowClassification::Failed,
            error.to_string(),
            Some(row_dir),
            Some(config_path),
            lola_manifest_path,
            logs,
        ),
    };
    if let Some(paths) = &lola_manifest_paths {
        result.lola_manifest_paths = [
            ("streamer", &paths.streamer),
            ("active", &paths.active),
            ("passive", &paths.passive),
        ]
        .into_iter()
        .map(|(role, path)| (role.to_string(), path.display().to_string()))
        .collect();
    }
    Ok(result)
}

fn build_required_binaries(
    repo_root: &Path,
    rows: &[MatrixRow],
    use_local_sibling_patches: bool,
) -> Result<()> {
    let common_args = if use_local_sibling_patches {
        cargo_patch_args(repo_root)
    } else {
        Vec::new()
    };
    if !common_args.is_empty() {
        println!("Using local sibling Cargo patches:");
        for config in common_args.chunks(2).filter_map(|chunk| chunk.get(1)) {
            println!("  {config}");
        }
    }
    let configurable_features = configurable_streamer_features(rows).join(",");
    let example_features = example_streamer_features(rows).join(",");
    run_cargo(
        repo_root,
        [
            "build",
            "-p",
            "configurable-streamer",
            "--features",
            &configurable_features,
            "--no-default-features",
        ],
        &common_args,
    )?;
    run_cargo(
        repo_root,
        [
            "build",
            "-p",
            "example-streamer-uses",
            "--bins",
            "--features",
            &example_features,
            "--no-default-features",
        ],
        &common_args,
    )?;
    Ok(())
}

fn configurable_streamer_features(rows: &[MatrixRow]) -> Vec<&'static str> {
    let mut features = vec![
        "experimental-copy-minimized-routing",
        "zenoh-zero-copy",
        "iceoryx2-zero-copy",
        "zenoh-owned-frame",
        "iceoryx2-owned-frame",
    ];
    if rows.iter().any(MatrixRow::uses_lola) {
        features.extend(["lola-transport", "lola-owned-frame"]);
    }
    if rows.iter().any(MatrixRow::uses_vsomeip) {
        features.extend(["vsomeip-transport", "bundled-vsomeip"]);
    }
    if rows.iter().any(MatrixRow::uses_dds) {
        features.extend(["dds-transport", "dds-owned-frame", "dds-zero-copy"]);
    }
    features
}

fn example_streamer_features(rows: &[MatrixRow]) -> Vec<&'static str> {
    let mut features = vec![
        "zenoh-transport",
        "zenoh-selected-wire",
        "iceoryx2-selected-wire",
        "mqtt-transport",
    ];
    if rows.iter().any(MatrixRow::uses_lola) {
        features.push("lola-selected-wire");
    }
    if rows.iter().any(MatrixRow::uses_vsomeip) {
        features.extend(["vsomeip-transport", "bundled-vsomeip"]);
    }
    if rows.iter().any(MatrixRow::uses_dds) {
        features.push("dds-transport");
    }
    features
}

fn run_cargo<const N: usize>(
    repo_root: &Path,
    args: [&str; N],
    patch_args: &[String],
) -> Result<()> {
    let mut command = Command::new("cargo");
    command.current_dir(repo_root).args(args).args(patch_args);
    command.env("CARGO_NET_GIT_FETCH_WITH_CLI", "true");
    if std::env::var_os("BAZEL").is_none() {
        let bazel = repo_root.join(".cache/tools/bazelisk-v1.29.0-linux-amd64");
        if bazel.is_file() {
            command.env("BAZEL", bazel);
        }
    }
    let status = command.status().context("failed to run cargo build")?;
    if !status.success() {
        return Err(anyhow!("cargo build failed with status {status}"));
    }
    Ok(())
}

fn cargo_patch_args(repo_root: &Path) -> Vec<String> {
    let siblings = [
        (
            "ssh://git@github.com/PLeVasseur/up-rust.git",
            "up-rust",
            "../up-rust",
        ),
        (
            "https://github.com/PLeVasseur/up-rust",
            "up-rust",
            "../up-rust",
        ),
        (
            "ssh://git@github.com/PLeVasseur/up-wire-xcdrv2-rust.git",
            "up-wire-xcdrv2",
            "../up-wire-xcdrv2-rust",
        ),
        (
            "https://github.com/PLeVasseur/up-wire-xcdrv2-rust",
            "up-wire-xcdrv2",
            "../up-wire-xcdrv2-rust",
        ),
        (
            "https://github.com/PLeVasseur/up-transport-lola-rust",
            "up-transport-lola-rust",
            "../up-transport-lola-rust",
        ),
        (
            "https://github.com/PLeVasseur/up-transport-iceoryx2-rust",
            "up-transport-iceoryx2-rust",
            "../up-transport-iceoryx2-rust",
        ),
        (
            "https://github.com/PLeVasseur/up-client-vsomeip-rust.git",
            "up-transport-vsomeip",
            "../up-transport-vsomeip-rust/up-transport-vsomeip",
        ),
        (
            "https://github.com/PLeVasseur/up-client-vsomeip-rust.git",
            "vsomeip-proc-macro",
            "../up-transport-vsomeip-rust/vsomeip-proc-macro",
        ),
        (
            "https://github.com/PLeVasseur/up-client-vsomeip-rust.git",
            "vsomeip-sys",
            "../up-transport-vsomeip-rust/vsomeip-sys",
        ),
    ];
    let mut args = Vec::new();
    for (source, package, path) in siblings {
        if repo_root.join(path).is_dir() {
            args.push("--config".to_string());
            args.push(format!("patch.\"{source}\".{package}.path = \"{path}\""));
        }
    }
    args
}

fn write_config(
    repo_root: &Path,
    row: &MatrixRow,
    config_path: &Path,
    zenoh_config_paths: &ZenohConfigPaths,
    vsomeip_config_paths: &VsomeipConfigPaths,
    lola_manifest_path: Option<&Path>,
    lola_run_namespace: &str,
) -> Result<()> {
    let zenoh_config = repo_root.join("configurable-streamer/ZENOH_CONFIG.json5");
    let mqtt_config = repo_root.join("configurable-streamer/MQTT_CONFIG.json5");
    let subscription_data = repo_root.join("configurable-streamer/subscription_data.json");
    let source_endpoint = endpoint_name(row.source, "source");
    let sink_endpoint = endpoint_name(row.sink, "sink");
    let source_lola = lola_info(row, AUTHORITY_A, "source", lola_run_namespace);
    let sink_lola = lola_info(row, AUTHORITY_B, "sink", lola_run_namespace);

    let mut zenoh_endpoints = Vec::new();
    let mut mqtt_endpoints = Vec::new();
    let mut iceoryx2_endpoints = Vec::new();
    let mut lola_endpoints = Vec::new();
    let mut vsomeip_endpoints = Vec::new();
    let mut dds_endpoints = Vec::new();

    push_endpoint(
        &mut zenoh_endpoints,
        &mut mqtt_endpoints,
        &mut iceoryx2_endpoints,
        &mut lola_endpoints,
        &mut vsomeip_endpoints,
        &mut dds_endpoints,
        row.source,
        "source",
        AUTHORITY_A,
        &source_endpoint,
        &sink_endpoint,
        row.encoding,
        zenoh_config_paths,
        source_lola.as_ref(),
        lola_manifest_path,
        row.role,
    );
    push_endpoint(
        &mut zenoh_endpoints,
        &mut mqtt_endpoints,
        &mut iceoryx2_endpoints,
        &mut lola_endpoints,
        &mut vsomeip_endpoints,
        &mut dds_endpoints,
        row.sink,
        "sink",
        AUTHORITY_B,
        &sink_endpoint,
        &source_endpoint,
        row.encoding,
        zenoh_config_paths,
        sink_lola.as_ref(),
        lola_manifest_path,
        row.role,
    );

    let mut transports = json!({
        "zenoh": { "config_file": zenoh_config, "endpoints": zenoh_endpoints },
        "mqtt": { "config_file": mqtt_config, "endpoints": mqtt_endpoints },
    });
    if !iceoryx2_endpoints.is_empty() {
        transports["iceoryx2"] = json!({ "endpoints": iceoryx2_endpoints });
    }
    if !lola_endpoints.is_empty() {
        transports["lola"] = json!({ "endpoints": lola_endpoints });
    }
    if !vsomeip_endpoints.is_empty() {
        transports["vsomeip"] = json!({
            "config_file": vsomeip_config_paths.streamer,
            "remote_authority": vsomeip_remote_authority(row),
            "endpoints": vsomeip_endpoints
        });
    }
    if !dds_endpoints.is_empty() {
        transports["dds"] = json!({
            "domain_id": dds_domain(row),
            "origin_id": dds_streamer_origin(row),
            "qos": { "reliability": "reliable" },
            "history_depth": 32,
            "readiness": { "required_matched_readers": 1, "timeout_ms": 5000 },
            "endpoints": dds_endpoints,
        });
    }

    let streamer_uuri = streamer_uuri_config(row);
    let config = json!({
        "up_streamer_config": { "message_queue_size": 32 },
        "streamer_uuri": streamer_uuri,
        "usubscription_config": { "mode": "static_file", "file_path": subscription_data },
        "transports": transports
    });
    fs::write(config_path, serde_json::to_string_pretty(&config)?)
        .with_context(|| format!("unable to write {}", config_path.display()))
}

fn streamer_uuri_config(row: &MatrixRow) -> serde_json::Value {
    if row.source.physical == PhysicalTransport::Vsomeip
        && matches!(
            row.role,
            RoleStyle::ClientServerRpc | RoleStyle::NotifierNotifyee
        )
    {
        return json!({
            "authority": AUTHORITY_B,
            "ue_id": UE_ID,
            "ue_version_major": UE_VERSION_MAJOR,
        });
    }
    json!({ "authority": "authority-streamer", "ue_id": 78, "ue_version_major": 1 })
}

#[allow(clippy::too_many_arguments)]
fn push_endpoint(
    zenoh_endpoints: &mut Vec<serde_json::Value>,
    mqtt_endpoints: &mut Vec<serde_json::Value>,
    iceoryx2_endpoints: &mut Vec<serde_json::Value>,
    lola_endpoints: &mut Vec<serde_json::Value>,
    vsomeip_endpoints: &mut Vec<serde_json::Value>,
    dds_endpoints: &mut Vec<serde_json::Value>,
    profile: EndpointProfile,
    side: &str,
    authority: &str,
    endpoint: &str,
    forward_endpoint: &str,
    encoding: WireEncoding,
    zenoh_config_paths: &ZenohConfigPaths,
    lola: Option<&LolaEndpointInfo>,
    lola_manifest_path: Option<&Path>,
    role: RoleStyle,
) {
    let forwarding_routes = if role == RoleStyle::NotifierNotifyee
        && side == "sink"
        && profile.physical == PhysicalTransport::Vsomeip
    {
        Vec::new()
    } else {
        vec![json!({ "endpoint": forward_endpoint, "wire_format": encoding.wire_format() })]
    };
    let mut value = json!({
        "authority": authority,
        "endpoint": endpoint,
        "routing_mode": profile.kind.routing_mode(),
        "forwarding_routes": forwarding_routes,
    });
    if profile.kind == EndpointKind::CopyMinimized {
        value["copy_minimized_payload_alignment"] = json!(8);
    }
    if profile.physical == PhysicalTransport::Zenoh {
        value["zenoh_config_file"] = json!(zenoh_config_paths
            .router_for_side(side)
            .display()
            .to_string());
        value["zenoh_client_config_file"] = json!(zenoh_config_paths
            .client_for_side(side)
            .display()
            .to_string());
    }
    if let Some(lola) = lola {
        value["lola_instance_specifier"] = json!(lola.instance_specifier);
        value["lola_service_type"] = json!(lola.service_type);
        value["lola_event_name"] = json!(lola.event_name);
        value["lola_sample_size"] = json!(65536);
        value["lola_sample_alignment"] = json!(8);
        value["lola_max_samples"] = json!(LOLA_MAX_SAMPLES);
        value["lola_mw_com_config_file"] = json!(lola_manifest_path
            .expect("LoLa endpoint requires generated manifest")
            .display()
            .to_string());
        if role == RoleStyle::ClientServerRpc {
            value["lola_response_instance_specifier"] =
                json!(lola.response_instance_specifier.as_ref().unwrap());
            value["lola_response_service_type"] =
                json!(lola.response_service_type.as_ref().unwrap());
            value["lola_response_event_name"] = json!(lola.response_event_name.as_ref().unwrap());
            value["lola_response_mw_com_config_file"] = json!(lola_manifest_path
                .expect("LoLa endpoint requires generated manifest")
                .display()
                .to_string());
            value["lola_default_rx_channel"] = json!(match side {
                "source" => "primary",
                "sink" => "response",
                _ => "both",
            });
        }
    }
    if profile.physical == PhysicalTransport::Vsomeip {
        value["assumed_payload_encoding"] = json!(encoding.payload_encoding_literal());
        value["assumed_payload_content_type"] = json!(encoding.payload_encoding_content_type());
    }
    match profile.physical {
        PhysicalTransport::Zenoh => zenoh_endpoints.push(value),
        PhysicalTransport::Mqtt5 => mqtt_endpoints.push(value),
        PhysicalTransport::Iceoryx2 => iceoryx2_endpoints.push(value),
        PhysicalTransport::Lola => lola_endpoints.push(value),
        PhysicalTransport::Vsomeip => vsomeip_endpoints.push(value),
        PhysicalTransport::Dds => dds_endpoints.push(value),
    }
}

fn dds_domain(row: &MatrixRow) -> i32 {
    let mut candidate = 80_i32;
    let mut remaining = row.ordinal;
    loop {
        let multicast_port = (DDS_PORT_BASE + DDS_DOMAIN_GAIN * candidate) % DDS_PORT_MODULUS;
        if multicast_port >= DDS_MIN_UNPRIVILEGED_PORT {
            if remaining == 0 {
                return candidate;
            }
            remaining -= 1;
        }
        candidate += 1;
    }
}

fn dds_streamer_origin(row: &MatrixRow) -> String {
    format!("matrix-{:04}-streamer", row.ordinal)
}

fn dds_role_origin(row: &MatrixRow, active: bool) -> String {
    format!(
        "matrix-{:04}-{}-role",
        row.ordinal,
        if active { "source" } else { "sink" }
    )
}

fn vsomeip_remote_authority(row: &MatrixRow) -> &'static str {
    if row.source.physical == PhysicalTransport::Vsomeip {
        AUTHORITY_A
    } else {
        AUTHORITY_B
    }
}

fn write_vsomeip_configs(row: &MatrixRow, paths: &VsomeipConfigPaths) -> Result<()> {
    let network = format!("up_vs_{:08x}", stable_hash(&row.id));
    let base = 0x1000_u16 + ((row.ordinal as u16) * 3);
    let (service_id, instance_id) = vsomeip_config_service(row);
    let app_ids = vsomeip_app_ids(row, base);
    let applications = [
        ("streamer_app", app_ids.streamer),
        ("source_app", app_ids.source),
        ("sink_app", app_ids.sink),
    ];
    write_vsomeip_config(
        &paths.streamer,
        &network,
        "streamer_app",
        app_ids.streamer,
        service_id,
        instance_id,
        &applications,
    )?;
    write_vsomeip_config(
        &paths.source,
        &network,
        "source_app",
        app_ids.source,
        service_id,
        instance_id,
        &applications,
    )?;
    write_vsomeip_config(
        &paths.sink,
        &network,
        "sink_app",
        app_ids.sink,
        service_id,
        instance_id,
        &applications,
    )?;
    Ok(())
}

struct VsomeipAppIds {
    streamer: u16,
    source: u16,
    sink: u16,
}

fn vsomeip_app_ids(row: &MatrixRow, base: u16) -> VsomeipAppIds {
    let mut ids = VsomeipAppIds {
        streamer: base,
        source: base + 1,
        sink: base + 2,
    };
    if row.role == RoleStyle::NotifierNotifyee && row.source.physical == PhysicalTransport::Vsomeip
    {
        ids.source = UE_ID as u16;
    }
    if row.role == RoleStyle::NotifierNotifyee && row.sink.physical == PhysicalTransport::Vsomeip {
        ids.streamer = UE_ID as u16;
    }
    ids
}

fn vsomeip_config_service(row: &MatrixRow) -> (u16, u16) {
    let service_id = (UE_ID & 0xffff) as u16;
    let instance_id = ((UE_ID >> 16) as u16).max(1);
    if (row.role == RoleStyle::ClientServerRpc && row.source.physical == PhysicalTransport::Vsomeip)
        || row.role == RoleStyle::NotifierNotifyee
    {
        (service_id, instance_id)
    } else {
        (VSOMEIP_DUMMY_SERVICE_ID, VSOMEIP_DUMMY_INSTANCE_ID)
    }
}

fn write_vsomeip_config(
    path: &Path,
    network: &str,
    app_name: &str,
    app_id: u16,
    service_id: u16,
    instance_id: u16,
    applications: &[(&str, u16)],
) -> Result<()> {
    let mut application_entries =
        vec![json!({ "name": app_name, "id": format!("0x{app_id:04x}") })];
    for (candidate_name, candidate_id) in applications {
        if *candidate_name != app_name {
            application_entries
                .push(json!({ "name": candidate_name, "id": format!("0x{candidate_id:04x}") }));
        }
    }
    let config = json!({
        "unicast": "127.0.0.1",
        "network": network,
        "applications": application_entries,
        "services": [{ "service": format!("0x{service_id:04x}"), "instance": format!("0x{instance_id:04x}") }]
    });
    fs::write(path, serde_json::to_string_pretty(&config)?)
        .with_context(|| format!("unable to write {}", path.display()))
}

fn write_lola_manifest(
    row: &MatrixRow,
    path: &Path,
    lola_run_namespace: &str,
    application_id: u32,
) -> Result<()> {
    let mut service_types = Vec::new();
    let mut service_instances = Vec::new();
    let mut service_id = lola_service_id_base(row);
    for (authority, side) in [(AUTHORITY_A, "source"), (AUTHORITY_B, "sink")] {
        let profile = if side == "source" {
            row.source
        } else {
            row.sink
        };
        if profile.physical != PhysicalTransport::Lola {
            continue;
        }
        let info = lola_info(row, authority, side, lola_run_namespace)
            .expect("LoLa profile has LoLa info");
        push_lola_manifest_entry(
            &mut service_types,
            &mut service_instances,
            &mut service_id,
            &info.instance_specifier,
            &info.service_type,
            &info.event_name,
        );
        if row.role == RoleStyle::ClientServerRpc {
            push_lola_manifest_entry(
                &mut service_types,
                &mut service_instances,
                &mut service_id,
                info.response_instance_specifier.as_ref().unwrap(),
                info.response_service_type.as_ref().unwrap(),
                info.response_event_name.as_ref().unwrap(),
            );
        }
    }
    let manifest = json!({
        "serviceTypes": service_types,
        "serviceInstances": service_instances,
        "global": {
            "asil-level": "QM",
            "applicationID": application_id,
            "queue-size": { "QM-receiver": LOLA_QUEUE_SIZE, "QM-sender": LOLA_QUEUE_SIZE },
            "shm-size-calc-mode": "SIMULATION"
        }
    });
    fs::write(path, serde_json::to_string_pretty(&manifest)?)
        .with_context(|| format!("unable to write {}", path.display()))
}

fn write_zenoh_router_config(path: &Path, port: u16) -> Result<()> {
    let config = format!(
        r#"{{
  mode: "router",
  listen: {{
    endpoints: ["tcp/0.0.0.0:{port}"],
  }},
  scouting: {{
    multicast: {{
      enabled: false,
    }},
    gossip: {{
      enabled: false,
    }},
  }},
}}
"#
    );
    fs::write(path, config).with_context(|| format!("unable to write {}", path.display()))
}

fn write_zenoh_client_config(path: &Path, port: u16) -> Result<()> {
    let config = format!(
        r#"{{
  mode: "client",
  connect: {{
    endpoints: ["tcp/127.0.0.1:{port}"],
  }},
  scouting: {{
    multicast: {{
      enabled: false,
    }},
    gossip: {{
      enabled: false,
    }},
  }},
}}
"#
    );
    fs::write(path, config).with_context(|| format!("unable to write {}", path.display()))
}

fn push_lola_manifest_entry(
    service_types: &mut Vec<serde_json::Value>,
    service_instances: &mut Vec<serde_json::Value>,
    service_id: &mut u32,
    instance_specifier: &str,
    service_type: &str,
    event_name: &str,
) {
    service_types.push(json!({
        "serviceTypeName": service_type,
        "version": { "major": 1, "minor": 0 },
        "bindings": [{ "binding": "SHM", "serviceId": *service_id, "events": [{ "eventName": event_name, "eventId": 1 }] }]
    }));
    service_instances.push(json!({
        "instanceSpecifier": instance_specifier,
        "serviceTypeName": service_type,
        "version": { "major": 1, "minor": 0 },
        "instances": [{
            "instanceId": 1,
            "asil-level": "QM",
            "binding": "SHM",
            "events": [{
                "eventName": event_name,
                "numberOfSampleSlots": LOLA_SAMPLE_SLOTS,
                "maxSubscribers": LOLA_MAX_SUBSCRIBERS,
                "numberOfIpcTracingSlots": 0
            }]
        }]
    }));
    *service_id += 1;
}

fn lola_service_id_base(row: &MatrixRow) -> u32 {
    14_000 + (row.ordinal as u32 * 4)
}

struct RoleCommand {
    binary: String,
    args: Vec<String>,
}

fn role_command(
    row: &MatrixRow,
    active: bool,
    zenoh_config_paths: &ZenohConfigPaths,
    vsomeip_config_paths: &VsomeipConfigPaths,
    lola_manifest_path: Option<&Path>,
    lola_run_namespace: &str,
    cli: &Cli,
) -> Result<RoleCommand> {
    let profile = if active { row.source } else { row.sink };
    let local_authority = if active { AUTHORITY_A } else { AUTHORITY_B };
    let peer_authority = if active { AUTHORITY_B } else { AUTHORITY_A };
    let role_name = role_binary_suffix(row.role, active);
    let binary = binary_name(profile, role_name);
    let zenoh_client_config = zenoh_config_paths.client_for_active(active);
    let mut args = if profile.kind == EndpointKind::Classic {
        if profile.physical == PhysicalTransport::Dds {
            dds_args(row, profile, active, local_authority, peer_authority, cli)
        } else {
            classic_args(
                row,
                profile,
                role_name,
                local_authority,
                peer_authority,
                zenoh_client_config,
                vsomeip_config_paths.role_config(active),
                cli,
            )
        }
    } else if profile.physical == PhysicalTransport::Dds {
        dds_args(row, profile, active, local_authority, peer_authority, cli)
    } else if profile.physical == PhysicalTransport::Zenoh {
        zenoh_args(
            row,
            role_name,
            local_authority,
            peer_authority,
            zenoh_client_config,
            cli,
        )
    } else {
        generic_args(
            row,
            profile,
            local_authority,
            peer_authority,
            zenoh_client_config,
            cli,
        )
    };
    if profile.physical == PhysicalTransport::Lola {
        add_lola_args(
            &mut args,
            row,
            active,
            local_authority,
            if active { "source" } else { "sink" },
            lola_manifest_path.expect("LoLa role requires generated manifest"),
            lola_run_namespace,
        )?;
    }
    Ok(RoleCommand { binary, args })
}

fn dds_args(
    row: &MatrixRow,
    profile: EndpointProfile,
    active: bool,
    local_authority: &str,
    peer_authority: &str,
    cli: &Cli,
) -> Vec<String> {
    vec![
        "--domain-id".into(),
        dds_domain(row).to_string(),
        "--origin-id".into(),
        dds_role_origin(row, active),
        "--reliability".into(),
        "reliable".into(),
        "--history-depth".into(),
        "32".into(),
        "--route-family".into(),
        profile.kind.route_family().into(),
        "--encoding".into(),
        row.encoding.cli_value().into(),
        "--local-authority".into(),
        local_authority.into(),
        "--peer-authority".into(),
        peer_authority.into(),
        "--topic-resource-id".into(),
        role_topic_resource_id(row).to_string(),
        "--method-resource-id".into(),
        METHOD_RESOURCE_ID.to_string(),
        "--timeout-ms".into(),
        cli.timeout_ms.max(8_000).to_string(),
        "--payload".into(),
        row.id.clone(),
        "--payload-alignment".into(),
        "8".into(),
    ]
}

#[allow(clippy::too_many_arguments)]
fn classic_args(
    row: &MatrixRow,
    profile: EndpointProfile,
    role_name: &str,
    local_authority: &str,
    peer_authority: &str,
    zenoh_client_config: &Path,
    vsomeip_config: &Path,
    cli: &Cli,
) -> Vec<String> {
    match profile.physical {
        PhysicalTransport::Zenoh => classic_zenoh_args(
            row,
            role_name,
            local_authority,
            peer_authority,
            zenoh_client_config,
            cli,
        ),
        PhysicalTransport::Mqtt5 => {
            classic_mqtt_args(row, role_name, local_authority, peer_authority, cli)
        }
        PhysicalTransport::Vsomeip => classic_vsomeip_args(
            row,
            role_name,
            local_authority,
            peer_authority,
            vsomeip_config,
            cli,
        ),
        PhysicalTransport::Iceoryx2 | PhysicalTransport::Lola | PhysicalTransport::Dds => {
            unreachable!("selected-wire or DDS profiles are handled before classic dispatch")
        }
    }
}

fn classic_vsomeip_args(
    row: &MatrixRow,
    role_name: &str,
    local_authority: &str,
    peer_authority: &str,
    vsomeip_config: &Path,
    cli: &Cli,
) -> Vec<String> {
    let common_identity = [
        "--uauthority".to_string(),
        local_authority.to_string(),
        "--uentity".to_string(),
        format!("0x{UE_ID:X}"),
        "--uversion".to_string(),
        format!("0x{UE_VERSION_MAJOR:X}"),
    ];
    let common_transport = [
        "--remote-authority".to_string(),
        peer_authority.to_string(),
        "--vsomeip-config".to_string(),
        vsomeip_config.display().to_string(),
        "--encoding".to_string(),
        row.encoding.cli_value().to_string(),
    ];
    let notification_resource_id = role_topic_resource_id(row);

    match role_name {
        "publisher" => common_identity
            .into_iter()
            .chain(["--resource".to_string(), format!("0x{TOPIC_RESOURCE_ID:X}")])
            .chain(common_transport)
            .chain([
                "--send-count".to_string(),
                role_send_count(row, cli).to_string(),
                "--send-interval-ms".to_string(),
                cli.send_interval_ms.to_string(),
                "--payload".to_string(),
                row.id.clone(),
            ])
            .collect(),
        "subscriber" => common_identity
            .into_iter()
            .chain(["--resource".to_string(), "0x0".to_string()])
            .chain(common_transport)
            .chain([
                "--source-authority".to_string(),
                peer_authority.to_string(),
                "--source-uentity".to_string(),
                format!("0x{UE_ID:X}"),
                "--source-uversion".to_string(),
                format!("0x{UE_VERSION_MAJOR:X}"),
                "--source-resource".to_string(),
                format!("0x{TOPIC_RESOURCE_ID:X}"),
            ])
            .collect(),
        "client" => common_identity
            .into_iter()
            .chain(["--resource".to_string(), "0x0".to_string()])
            .chain(common_transport)
            .chain([
                "--target-authority".to_string(),
                peer_authority.to_string(),
                "--target-uentity".to_string(),
                format!("0x{UE_ID:X}"),
                "--target-uversion".to_string(),
                format!("0x{UE_VERSION_MAJOR:X}"),
                "--target-resource".to_string(),
                format!("0x{METHOD_RESOURCE_ID:X}"),
                "--send-count".to_string(),
                role_send_count(row, cli).to_string(),
                "--send-interval-ms".to_string(),
                cli.send_interval_ms.to_string(),
                "--timeout-ms".to_string(),
                cli.timeout_ms.to_string(),
                "--payload".to_string(),
                row.id.clone(),
            ])
            .collect(),
        "server" => common_identity
            .into_iter()
            .chain([
                "--resource".to_string(),
                format!("0x{METHOD_RESOURCE_ID:X}"),
            ])
            .chain(common_transport)
            .collect(),
        "notifier" => common_identity
            .into_iter()
            .chain([
                "--resource".to_string(),
                format!("0x{notification_resource_id:X}"),
            ])
            .chain(common_transport)
            .chain([
                "--sink-authority".to_string(),
                peer_authority.to_string(),
                "--sink-uentity".to_string(),
                format!("0x{UE_ID:X}"),
                "--send-count".to_string(),
                role_send_count(row, cli).to_string(),
                "--send-interval-ms".to_string(),
                cli.send_interval_ms.to_string(),
                "--payload".to_string(),
                row.id.clone(),
            ])
            .collect(),
        "notifyee" => common_identity
            .into_iter()
            .chain(["--resource".to_string(), "0x0".to_string()])
            .chain(common_transport)
            .chain([
                "--source-authority".to_string(),
                peer_authority.to_string(),
                "--source-uentity".to_string(),
                format!("0x{UE_ID:X}"),
                "--source-uversion".to_string(),
                format!("0x{UE_VERSION_MAJOR:X}"),
                "--source-resource".to_string(),
                format!("0x{notification_resource_id:X}"),
            ])
            .collect(),
        _ => Vec::new(),
    }
}

fn classic_zenoh_args(
    row: &MatrixRow,
    role_name: &str,
    local_authority: &str,
    peer_authority: &str,
    zenoh_client_config: &Path,
    cli: &Cli,
) -> Vec<String> {
    match role_name {
        "publisher" => vec![
            "--zenoh-config".into(),
            zenoh_client_config.display().to_string(),
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            format!("0x{TOPIC_RESOURCE_ID:X}"),
            "--send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--payload".into(),
            row.id.clone(),
        ],
        "subscriber" => vec![
            "--zenoh-config".into(),
            zenoh_client_config.display().to_string(),
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            "0x0".into(),
            "--source-authority".into(),
            peer_authority.into(),
            "--source-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--source-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--source-resource".into(),
            format!("0x{TOPIC_RESOURCE_ID:X}"),
            "--encoding".into(),
            row.encoding.cli_value().into(),
        ],
        "client" => vec![
            "--zenoh-config".into(),
            zenoh_client_config.display().to_string(),
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            "0x0".into(),
            "--target-authority".into(),
            peer_authority.into(),
            "--target-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--target-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--target-resource".into(),
            format!("0x{METHOD_RESOURCE_ID:X}"),
            "--send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--timeout-ms".into(),
            cli.timeout_ms.to_string(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--payload".into(),
            row.id.clone(),
        ],
        "server" | "notifier" | "notifyee" => classic_generic_flow_args(
            row,
            local_authority,
            peer_authority,
            zenoh_client_config,
            cli,
        ),
        _ => Vec::new(),
    }
}

fn classic_generic_flow_args(
    row: &MatrixRow,
    local_authority: &str,
    peer_authority: &str,
    zenoh_client_config: &Path,
    cli: &Cli,
) -> Vec<String> {
    vec![
        "--local-authority".into(),
        local_authority.into(),
        "--peer-authority".into(),
        peer_authority.into(),
        "--ue-id".into(),
        UE_ID.to_string(),
        "--ue-version-major".into(),
        UE_VERSION_MAJOR.to_string(),
        "--topic-resource-id".into(),
        role_topic_resource_id(row).to_string(),
        "--method-resource-id".into(),
        METHOD_RESOURCE_ID.to_string(),
        "--send-count".into(),
        role_send_count(row, cli).to_string(),
        "--send-interval-ms".into(),
        cli.send_interval_ms.to_string(),
        "--timeout-ms".into(),
        cli.timeout_ms.to_string(),
        "--payload".into(),
        row.id.clone(),
        "--encoding".into(),
        row.encoding.cli_value().into(),
        "--zenoh-config".into(),
        zenoh_client_config.display().to_string(),
    ]
}

fn classic_mqtt_args(
    row: &MatrixRow,
    role_name: &str,
    local_authority: &str,
    peer_authority: &str,
    cli: &Cli,
) -> Vec<String> {
    let broker_uri = format!("localhost:{MQTT_BROKER_PORT}");
    let notification_resource_id = role_topic_resource_id(row);
    match role_name {
        "publisher" => vec![
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            format!("0x{notification_resource_id:X}"),
            "--broker-uri".into(),
            broker_uri,
            "--send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--payload".into(),
            row.id.clone(),
        ],
        "subscriber" => vec![
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            "0x0".into(),
            "--source-authority".into(),
            peer_authority.into(),
            "--source-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--source-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--source-resource".into(),
            format!("0x{notification_resource_id:X}"),
            "--broker-uri".into(),
            broker_uri,
        ],
        "client" => vec![
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            "0x0".into(),
            "--target-authority".into(),
            peer_authority.into(),
            "--target-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--target-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--target-resource".into(),
            format!("0x{METHOD_RESOURCE_ID:X}"),
            "--broker-uri".into(),
            broker_uri,
            "--send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--timeout-ms".into(),
            cli.timeout_ms.to_string(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--payload".into(),
            row.id.clone(),
        ],
        "server" => vec![
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            format!("0x{METHOD_RESOURCE_ID:X}"),
            "--broker-uri".into(),
            broker_uri,
        ],
        "notifier" => vec![
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            format!("0x{notification_resource_id:X}"),
            "--sink-authority".into(),
            peer_authority.into(),
            "--sink-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--broker-uri".into(),
            broker_uri,
            "--send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--payload".into(),
            row.id.clone(),
        ],
        "notifyee" => vec![
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            "0x0".into(),
            "--source-authority".into(),
            peer_authority.into(),
            "--source-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--source-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--source-resource".into(),
            format!("0x{notification_resource_id:X}"),
            "--broker-uri".into(),
            broker_uri,
        ],
        _ => Vec::new(),
    }
}

fn zenoh_args(
    row: &MatrixRow,
    role_name: &str,
    local_authority: &str,
    peer_authority: &str,
    zenoh_client_config: &Path,
    cli: &Cli,
) -> Vec<String> {
    match role_name {
        "publisher" => vec![
            "--endpoint".into(),
            ZENOH_ENDPOINT.into(),
            "--zenoh-config".into(),
            zenoh_client_config.display().to_string(),
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            format!("0x{TOPIC_RESOURCE_ID:X}"),
            "--route-family".into(),
            row.source.kind.route_family().into(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--selected-send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--payload".into(),
            row.id.clone(),
        ],
        "subscriber" => vec![
            "--endpoint".into(),
            ZENOH_ENDPOINT.into(),
            "--zenoh-config".into(),
            zenoh_client_config.display().to_string(),
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            "0x5BB0".into(),
            "--uversion".into(),
            "0x1".into(),
            "--resource".into(),
            "0x0".into(),
            "--source-authority".into(),
            peer_authority.into(),
            "--source-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--source-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--source-resource".into(),
            format!("0x{TOPIC_RESOURCE_ID:X}"),
            "--route-family".into(),
            row.sink.kind.route_family().into(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--timeout-ms".into(),
            cli.timeout_ms.to_string(),
        ],
        "client" => vec![
            "--endpoint".into(),
            ZENOH_ENDPOINT.into(),
            "--zenoh-config".into(),
            zenoh_client_config.display().to_string(),
            "--uauthority".into(),
            local_authority.into(),
            "--uentity".into(),
            format!("0x{UE_ID:X}"),
            "--uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--resource".into(),
            "0x0".into(),
            "--target-authority".into(),
            peer_authority.into(),
            "--target-uentity".into(),
            format!("0x{UE_ID:X}"),
            "--target-uversion".into(),
            format!("0x{UE_VERSION_MAJOR:X}"),
            "--target-resource".into(),
            format!("0x{METHOD_RESOURCE_ID:X}"),
            "--route-family".into(),
            row.source.kind.route_family().into(),
            "--encoding".into(),
            row.encoding.cli_value().into(),
            "--send-count".into(),
            role_send_count(row, cli).to_string(),
            "--send-interval-ms".into(),
            cli.send_interval_ms.to_string(),
            "--timeout-ms".into(),
            cli.timeout_ms.to_string(),
            "--payload".into(),
            row.id.clone(),
        ],
        _ => generic_args(
            row,
            zenoh_profile_for_role(row, role_name),
            local_authority,
            peer_authority,
            zenoh_client_config,
            cli,
        ),
    }
}

fn zenoh_profile_for_role(row: &MatrixRow, role_name: &str) -> EndpointProfile {
    match role_name {
        "server" | "notifyee" => row.sink,
        _ => row.source,
    }
}

fn generic_args(
    row: &MatrixRow,
    profile: EndpointProfile,
    local_authority: &str,
    peer_authority: &str,
    zenoh_client_config: &Path,
    cli: &Cli,
) -> Vec<String> {
    let mut args = vec![
        "--route-family".into(),
        profile.kind.route_family().into(),
        "--encoding".into(),
        row.encoding.cli_value().into(),
        "--local-authority".into(),
        local_authority.into(),
        "--peer-authority".into(),
        peer_authority.into(),
        "--ue-id".into(),
        UE_ID.to_string(),
        "--ue-version-major".into(),
        UE_VERSION_MAJOR.to_string(),
        "--topic-resource-id".into(),
        role_topic_resource_id(row).to_string(),
        "--method-resource-id".into(),
        METHOD_RESOURCE_ID.to_string(),
        "--send-count".into(),
        role_send_count(row, cli).to_string(),
        "--send-interval-ms".into(),
        cli.send_interval_ms.to_string(),
        "--timeout-ms".into(),
        cli.timeout_ms.to_string(),
        "--payload".into(),
        row.id.clone(),
    ];
    if profile.physical == PhysicalTransport::Zenoh {
        args.push("--zenoh-config".into());
        args.push(zenoh_client_config.display().to_string());
    }
    args
}

fn add_lola_args(
    args: &mut Vec<String>,
    row: &MatrixRow,
    active: bool,
    authority: &str,
    side: &str,
    manifest: &Path,
    lola_run_namespace: &str,
) -> Result<()> {
    let info = lola_info(row, authority, side, lola_run_namespace)
        .ok_or_else(|| anyhow!("missing LoLa info"))?;
    args.extend([
        "--lola-mw-com-config-file".to_string(),
        manifest.display().to_string(),
        "--lola-instance-specifier".to_string(),
        info.instance_specifier,
        "--lola-service-type".to_string(),
        info.service_type,
        "--lola-event-name".to_string(),
        info.event_name,
        "--lola-sample-size".to_string(),
        "65536".to_string(),
        "--lola-sample-alignment".to_string(),
        "8".to_string(),
        "--lola-max-samples".to_string(),
        LOLA_MAX_SAMPLES.to_string(),
    ]);
    if row.role == RoleStyle::ClientServerRpc {
        args.extend([
            "--lola-rpc-response-mw-com-config-file".to_string(),
            manifest.display().to_string(),
            "--lola-rpc-response-instance-specifier".to_string(),
            info.response_instance_specifier.unwrap(),
            "--lola-rpc-response-service-type".to_string(),
            info.response_service_type.unwrap(),
            "--lola-rpc-response-event-name".to_string(),
            info.response_event_name.unwrap(),
            "--lola-default-rx-channel".to_string(),
            if active { "response" } else { "primary" }.to_string(),
        ]);
    }
    Ok(())
}

fn role_send_count(_row: &MatrixRow, cli: &Cli) -> usize {
    cli.send_count.max(5)
}

fn role_topic_resource_id(row: &MatrixRow) -> u16 {
    if row.role == RoleStyle::NotifierNotifyee && row.uses_vsomeip() {
        NOTIFICATION_RESOURCE_ID
    } else {
        TOPIC_RESOURCE_ID
    }
}

fn binary_name(profile: EndpointProfile, role_name: &str) -> String {
    match profile.physical {
        PhysicalTransport::Zenoh => format!("zenoh_{role_name}"),
        PhysicalTransport::Iceoryx2 => format!("iceoryx2_{role_name}"),
        PhysicalTransport::Lola => format!("lola_{role_name}"),
        PhysicalTransport::Mqtt5 => format!("mqtt_{role_name}"),
        PhysicalTransport::Vsomeip => format!("someip_{role_name}"),
        PhysicalTransport::Dds => format!("dds_{role_name}"),
    }
}

fn role_binary_suffix(role: RoleStyle, active: bool) -> &'static str {
    match (role, active) {
        (RoleStyle::PublisherSubscriber, true) => "publisher",
        (RoleStyle::PublisherSubscriber, false) => "subscriber",
        (RoleStyle::NotifierNotifyee, true) => "notifier",
        (RoleStyle::NotifierNotifyee, false) => "notifyee",
        (RoleStyle::ClientServerRpc, true) => "client",
        (RoleStyle::ClientServerRpc, false) => "server",
    }
}

fn validate_flow_logs(row: &MatrixRow, active_log: &Path, passive_log: &Path) -> Result<()> {
    let active = fs::read_to_string(active_log).unwrap_or_default();
    let passive = fs::read_to_string(passive_log).unwrap_or_default();
    if row.uses_classic() {
        return validate_classic_aware_logs(row, &active, active_log, &passive, passive_log);
    }
    match row.role {
        RoleStyle::PublisherSubscriber => {
            require_log(&active, "FLOW sent_payload_bytes", active_log)?;
            require_log(&passive, "FLOW observed_payload_bytes", passive_log)?;
        }
        RoleStyle::NotifierNotifyee => {
            require_log(&active, "FLOW sent_payload_bytes", active_log)?;
            require_log(&passive, "FLOW observed_payload_bytes", passive_log)?;
        }
        RoleStyle::ClientServerRpc => {
            require_log(&active, "FLOW observed_payload_bytes", active_log)?;
            require_log(&passive, "FLOW observed_payload_bytes", passive_log)?;
        }
    }
    Ok(())
}

fn validate_classic_aware_logs(
    row: &MatrixRow,
    active: &str,
    active_log: &Path,
    passive: &str,
    passive_log: &Path,
) -> Result<()> {
    match row.role {
        RoleStyle::PublisherSubscriber => {
            require_any_log(
                active,
                &["Sending Publish message", "FLOW sent_payload_bytes"],
                active_log,
            )?;
            require_any_log(
                passive,
                &[
                    "PublishReceiver: Received a message",
                    "FLOW observed_payload_bytes",
                ],
                passive_log,
            )?;
        }
        RoleStyle::NotifierNotifyee => {
            require_any_log(
                active,
                &["Sending Notification message", "FLOW sent_payload_bytes"],
                active_log,
            )?;
            require_any_log(
                passive,
                &[
                    "PublishReceiver: Received a message",
                    "FLOW observed_payload_bytes",
                ],
                passive_log,
            )?;
        }
        RoleStyle::ClientServerRpc => {
            require_any_log(
                active,
                &[
                    "ServiceResponseListener: Received a message",
                    "FLOW observed_payload_bytes",
                ],
                active_log,
            )?;
            require_any_log(
                passive,
                &["Sending Response message", "FLOW observed_payload_bytes"],
                passive_log,
            )?;
        }
    }
    Ok(())
}

fn require_log(contents: &str, marker: &str, path: &Path) -> Result<()> {
    if contents.contains(marker) {
        Ok(())
    } else {
        Err(anyhow!(
            "log {} did not contain marker {marker}",
            path.display()
        ))
    }
}

fn require_any_log(contents: &str, markers: &[&str], path: &Path) -> Result<()> {
    if markers.iter().any(|marker| contents.contains(marker)) {
        Ok(())
    } else {
        Err(anyhow!(
            "log {} did not contain any marker from {:?}",
            path.display(),
            markers
        ))
    }
}

fn spawn_process(
    name: &str,
    executable: &Path,
    args: &[String],
    workdir: &Path,
    env: &[(String, String)],
    artifact_dir: &Path,
    namespace: Option<&RunningProcess>,
) -> Result<RunningProcess> {
    let log_path = artifact_dir.join(format!("{name}.log"));
    let stdout = log_file(&log_path)?;
    let stderr = stdout.try_clone()?;
    let mut command = if let Some(namespace) = namespace {
        let mut command = Command::new("nsenter");
        command
            .arg("-t")
            .arg(namespace.child.id().to_string())
            .args(["-U", "--preserve-credentials", "-m", "-n", "-i", "--"])
            .arg(executable);
        command
    } else {
        Command::new(executable)
    };
    command
        .current_dir(workdir)
        .args(args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let child = command.spawn().with_context(|| {
        format!(
            "unable to spawn {name}: {} {}",
            executable.display(),
            args.join(" ")
        )
    })?;
    Ok(RunningProcess {
        name: name.to_string(),
        log_path,
        child,
    })
}

fn start_namespace_holder(
    artifact_dir: &Path,
    cancellation: &Cancellation,
) -> Result<RunningProcess> {
    let log_path = artifact_dir.join("namespace-holder.log");
    let ready_path = artifact_dir.join("namespace-ready");
    let stdout = log_file(&log_path)?;
    let stderr = stdout.try_clone()?;
    let script = format!(
        "set -eu; ip link set lo up; mount -t tmpfs -o size={tmp_size} tmpfs /tmp; mount -t tmpfs -o size={shm_size} tmpfs /dev/shm; mkdir -p {iceoryx2_root}; touch \"$1\"; exec sleep infinity",
        tmp_size = NAMESPACE_TMP_SIZE,
        shm_size = NAMESPACE_SHM_SIZE,
        iceoryx2_root = ICEORYX2_ROOT_PATH,
    );
    let child = Command::new("unshare")
        .args(["-U", "--map-root-user", "-m", "-n", "-i", "sh", "-c"])
        .arg(script)
        .arg("sh")
        .arg(&ready_path)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| "unable to spawn namespace holder with unshare")?;
    let mut process = RunningProcess {
        name: "namespace-holder".to_string(),
        log_path,
        child,
    };
    wait_for_path_or_exit(
        &ready_path,
        &mut process.child,
        &process.log_path,
        Duration::from_secs(10),
        cancellation,
    )?;
    Ok(process)
}

fn start_mqtt_broker(
    artifact_dir: &Path,
    env: &[(String, String)],
    namespace: &RunningProcess,
    cancellation: &Cancellation,
) -> Result<RunningProcess> {
    let config_path = artifact_dir.join("mosquitto.conf");
    fs::write(
        &config_path,
        format!(
            "listener {MQTT_BROKER_PORT} 127.0.0.1\nallow_anonymous true\npersistence false\nlog_dest stdout\nuser root\n"
        ),
    )
    .with_context(|| format!("unable to write {}", config_path.display()))?;
    let mut broker = spawn_process(
        "mqtt-broker",
        Path::new("mosquitto"),
        &["-c".to_string(), config_path.display().to_string()],
        artifact_dir,
        env,
        artifact_dir,
        Some(namespace),
    )?;
    cancellable_sleep(Duration::from_millis(250), cancellation)?;
    if let Some(status) = broker.child.try_wait()? {
        return Err(anyhow!(
            "MQTT broker exited before readiness with {status}; see {}",
            broker.log_path.display()
        ));
    }
    Ok(broker)
}

fn wait_for_path_or_exit(
    path: &Path,
    child: &mut Child,
    log_path: &Path,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        cancellation.check()?;
        if path.exists() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "namespace holder exited with status {status} before creating {}; log={}",
                path.display(),
                log_path.display()
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for namespace holder marker {}; log={}",
                path.display(),
                log_path.display()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn log_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("unable to open {}", path.display()))
}

fn wait_for_marker(
    process: &mut RunningProcess,
    marker: &str,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        cancellation.check()?;
        let contents = fs::read_to_string(&process.log_path).unwrap_or_default();
        if contents.contains(marker) {
            return Ok(());
        }
        if let Some(status) = process.child.try_wait()? {
            return Err(anyhow!(
                "process {} exited with {status} before marker {marker}; see {}",
                process.name,
                process.log_path.display()
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for marker {marker} in {}",
                process.log_path.display()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_any_marker(
    path: &Path,
    markers: &[&str],
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        cancellation.check()?;
        let contents = fs::read_to_string(path).unwrap_or_default();
        if markers.iter().any(|marker| contents.contains(marker)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for any marker from {:?} in {}",
                markers,
                path.display()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn passive_observation_markers(row: &MatrixRow) -> &'static [&'static str] {
    match row.role {
        RoleStyle::PublisherSubscriber | RoleStyle::NotifierNotifyee => &[
            "PublishReceiver: Received a message",
            "FLOW observed_payload_bytes",
        ],
        RoleStyle::ClientServerRpc => &["Sending Response message", "FLOW observed_payload_bytes"],
    }
}

fn wait_for_exit(
    process: &mut RunningProcess,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        cancellation.check()?;
        if process.child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "process {} did not exit within {:?}; log={}",
                process.name,
                timeout,
                process.log_path.display()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn cancellable_sleep(duration: Duration, cancellation: &Cancellation) -> Result<()> {
    let deadline = Instant::now() + duration;
    loop {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

fn assert_success(process: &mut RunningProcess) -> Result<()> {
    let Some(status) = process.child.try_wait()? else {
        return Err(anyhow!("process {} is still running", process.name));
    };
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "process {} exited with status {status}; log={}",
            process.name,
            process.log_path.display()
        ))
    }
}

fn terminate(process: &mut RunningProcess) {
    if matches!(process.child.try_wait(), Ok(Some(_))) {
        return;
    }
    let _ = Command::new("kill")
        .arg("-INT")
        .arg(process.child.id().to_string())
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if matches!(process.child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = process.child.kill();
    let _ = process.child.wait();
}

fn remaining_timeout(started: Instant, timeout_secs: u64, phase: &str) -> Result<Duration> {
    let elapsed = started.elapsed();
    let total = Duration::from_secs(timeout_secs);
    if elapsed >= total {
        Err(anyhow!("scenario timeout expired before {phase}"))
    } else {
        Ok(total - elapsed)
    }
}

fn row_result(
    row: &MatrixRow,
    classification: RowClassification,
    reason: String,
    artifact_dir: Option<PathBuf>,
    config_path: Option<PathBuf>,
    lola_manifest_path: Option<PathBuf>,
    logs: BTreeMap<String, String>,
) -> RowResult {
    let failure_phase =
        (classification == RowClassification::Failed).then(|| infer_failure_phase(&reason));
    RowResult {
        row_id: row.id.clone(),
        source_profile: row.source.id.to_string(),
        sink_profile: row.sink.id.to_string(),
        source_transport: row.source.physical,
        sink_transport: row.sink.physical,
        source_endpoint_kind: row.source.kind,
        sink_endpoint_kind: row.sink.kind,
        role: row.role,
        encoding: row.encoding,
        iteration: 1,
        attempts_used: 0,
        retries_consumed: 0,
        classification,
        failure_phase,
        reason,
        artifact_dir: artifact_dir.map(|path| path.display().to_string()),
        config_path: config_path.map(|path| path.display().to_string()),
        lola_manifest_path: lola_manifest_path.map(|path| path.display().to_string()),
        lola_manifest_paths: BTreeMap::new(),
        logs,
    }
}

fn infer_failure_phase(reason: &str) -> &'static str {
    if reason.contains(READY_STREAMER) && reason.contains("marker") {
        "streamer_readiness"
    } else if reason.contains(READY_LISTENER) && reason.contains("marker") {
        "passive_readiness_or_observation"
    } else if reason.contains("active") && reason.contains("process") {
        "active_process_exit"
    } else if reason.contains("passive") && reason.contains("process") {
        "passive_process_exit"
    } else if reason.contains("FLOW") || reason.contains("flow") {
        "flow_validation"
    } else if reason.contains("timeout") {
        "scenario_timeout"
    } else {
        "row_execution"
    }
}

fn row_env(
    row: &MatrixRow,
    iceoryx2_root: Option<&Path>,
    lola_bridge_lib_dir: Option<&Path>,
    vsomeip_lib_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "RUST_LOG".to_string(),
            "info,configurable_streamer=debug,up_streamer=debug,example_streamer_uses=debug,up_transport_zenoh=debug,up_transport_iceoryx2_rust=debug,up_transport_lola_rust=debug,up_transport_dds=debug".to_string(),
        ),
        ("CARGO_NET_GIT_FETCH_WITH_CLI".to_string(), "true".to_string()),
    ];
    if let Some(root) = iceoryx2_root {
        env.push((
            "UP_ICEORYX2_ROOT_PATH".to_string(),
            root.display().to_string(),
        ));
        env.push((
            "UP_ICEORYX2_PREFIX".to_string(),
            format!("u{:08x}_", stable_hash(&row.id)),
        ));
    }
    let mut ld_library_paths = Vec::new();
    if let Some(lib_dir) = lola_bridge_lib_dir {
        ld_library_paths.push(lib_dir.display().to_string());
    }
    if let Some(lib_dir) = vsomeip_lib_dir {
        ld_library_paths.push(lib_dir.display().to_string());
        if let Some(install_path) = lib_dir.parent() {
            env.push((
                "VSOMEIP_INSTALL_PATH".to_string(),
                install_path.display().to_string(),
            ));
        }
    }
    if !ld_library_paths.is_empty() {
        let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let prefix = ld_library_paths.join(":");
        let value = if existing.is_empty() {
            prefix
        } else {
            format!("{prefix}:{existing}")
        };
        env.push(("LD_LIBRARY_PATH".to_string(), value));
    }
    env
}

fn detect_vsomeip_lib_dir(repo_root: &Path) -> Result<PathBuf> {
    if let Ok(ld_library_path) = std::env::var("LD_LIBRARY_PATH") {
        for path in ld_library_path.split(':') {
            let candidate = Path::new(path).join("libvsomeip3.so.3");
            if candidate.is_file() {
                return Ok(PathBuf::from(path));
            }
        }
    }
    let build_root = repo_root.join("target/debug/build");
    for entry in fs::read_dir(&build_root)
        .with_context(|| format!("unable to read {}", build_root.display()))?
    {
        let entry = entry?;
        let path = entry.path().join("out/vsomeip/vsomeip-install/lib");
        if path.join("libvsomeip3.so.3").is_file() {
            return Ok(path);
        }
    }
    Err(anyhow!(
        "libvsomeip3.so.3 not found under LD_LIBRARY_PATH or target/debug/build"
    ))
}

fn detect_lola_bridge_lib_dir(repo_root: &Path) -> Result<PathBuf> {
    if let Ok(ld_library_path) = std::env::var("LD_LIBRARY_PATH") {
        for path in ld_library_path.split(':') {
            let candidate = Path::new(path).join("libup_lola_bridge.so");
            if candidate.is_file() {
                return Ok(PathBuf::from(path));
            }
        }
    }
    let build_root = repo_root.join("target/debug/build");
    for entry in fs::read_dir(&build_root)
        .with_context(|| format!("unable to read {}", build_root.display()))?
    {
        let entry = entry?;
        let path = entry
            .path()
            .join("out/up-lola-bridge-bazel/bazel-out/k8-fastbuild/bin");
        if path.join("libup_lola_bridge.so").is_file() {
            return Ok(path);
        }
    }
    Err(anyhow!(
        "libup_lola_bridge.so not found under LD_LIBRARY_PATH or target/debug/build"
    ))
}

fn target_debug_binary(repo_root: &Path, name: &str) -> PathBuf {
    repo_root.join("target/debug").join(name)
}

fn repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let mut cursor = Some(cwd.as_path());
    while let Some(path) = cursor {
        if path.join("Cargo.toml").is_file() && path.join("configurable-streamer").is_dir() {
            return Ok(path.to_path_buf());
        }
        cursor = path.parent();
    }
    Err(anyhow!("unable to locate up-streamer-rust workspace root"))
}

fn endpoint_name(profile: EndpointProfile, side: &str) -> String {
    format!("{}-{side}", profile.id)
}

fn lola_info(
    row: &MatrixRow,
    authority: &str,
    side: &str,
    lola_run_namespace: &str,
) -> Option<LolaEndpointInfo> {
    let profile = if side == "source" {
        row.source
    } else {
        row.sink
    };
    if profile.physical != PhysicalTransport::Lola {
        return None;
    }
    let safe_namespace = sanitize(lola_run_namespace);
    let safe_authority = sanitize(authority);
    let safe_side = sanitize(side);
    Some(LolaEndpointInfo {
        instance_specifier: format!(
            "uprotocol/streamerTransportTest/{safe_namespace}/{safe_side}/primary"
        ),
        service_type: format!(
            "/uprotocol/StreamerTransportTest/{safe_namespace}/{safe_authority}/{safe_side}/Primary"
        ),
        event_name: format!("frame{safe_namespace}{safe_authority}{safe_side}Primary"),
        response_instance_specifier: Some(format!(
            "uprotocol/streamerTransportTest/{safe_namespace}/{safe_side}/response"
        )),
        response_service_type: Some(format!(
            "/uprotocol/StreamerTransportTest/{safe_namespace}/{safe_authority}/{safe_side}/Response"
        )),
        response_event_name: Some(format!("frame{safe_namespace}{safe_authority}{safe_side}Response")),
    })
}

fn lola_run_namespace(artifacts_root: &Path, row: &MatrixRow) -> String {
    let seed = format!(
        "{}:{}:{}:{}:{}",
        artifacts_root.display(),
        row.id,
        row.ordinal,
        std::process::id(),
        Utc::now().timestamp_micros()
    );
    format!("r{:08x}", stable_hash(&seed))
}

fn lola_application_id(lola_run_namespace: &str, role: u32) -> u32 {
    (stable_hash(lola_run_namespace) & 0x1fff_ffff) * 4 + role
}

fn stable_hash(value: &str) -> u32 {
    value.bytes().fold(0x811C_9DC5, |hash, byte| {
        hash.wrapping_mul(0x0100_0193) ^ u32::from(byte)
    })
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

impl EndpointKind {
    fn is_selected_wire(self) -> bool {
        matches!(self, Self::OwnedFrame | Self::CopyMinimized)
    }

    fn route_family(self) -> &'static str {
        match self {
            Self::OwnedFrame => "owned-frame",
            Self::CopyMinimized => "copy-minimized",
            Self::Classic => "classic",
        }
    }

    fn routing_mode(self) -> &'static str {
        match self {
            Self::OwnedFrame => "owned_frame",
            Self::CopyMinimized => "copy_minimized",
            Self::Classic => "owned",
        }
    }
}

impl RoleStyle {
    fn id(self) -> &'static str {
        match self {
            Self::PublisherSubscriber => "publisher-subscriber",
            Self::NotifierNotifyee => "notifier-notifyee",
            Self::ClientServerRpc => "client-server-rpc",
        }
    }
}

impl WireEncoding {
    fn id(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Protobuf => "protobuf",
            Self::Xcdrv2 => "xcdrv2",
            Self::Arrow => "arrow",
            Self::Omgidl => "omgidl",
        }
    }

    fn cli_value(self) -> &'static str {
        self.id()
    }

    fn wire_format(self) -> &'static str {
        match self {
            Self::Native => "up_native",
            Self::Protobuf => "protobuf",
            Self::Xcdrv2 => "xcdrv2",
            Self::Arrow => "arrow",
            Self::Omgidl => "omgidl",
        }
    }

    fn payload_encoding_literal(self) -> &'static str {
        match self {
            Self::Native => "up.stable-container",
            Self::Protobuf => "up.protobuf",
            Self::Xcdrv2 => "up.xcdr-v2",
            Self::Arrow => "up.arrow-ipc-stream",
            Self::Omgidl => "up.omgidl-xcdr1-le",
        }
    }

    fn payload_encoding_content_type(self) -> &'static str {
        match self {
            Self::Native => "application/vnd.uprotocol.stable-container;type=\"org.eclipse.uprotocol.examples.SelectedWireNativePayloadV1\";variant=fixed;size=272;align=4",
            Self::Protobuf => "application/protobuf",
            Self::Xcdrv2 => "application/vnd.uprotocol.xcdr-v2;endianness=little;version=2",
            Self::Arrow => "application/vnd.apache.arrow.stream",
            Self::Omgidl => "application/vnd.omg.dds.xcdr1;endianness=little",
        }
    }
}

impl MatrixRow {
    fn uses_classic(&self) -> bool {
        self.source.kind == EndpointKind::Classic || self.sink.kind == EndpointKind::Classic
    }

    fn uses_lola(&self) -> bool {
        self.source.physical == PhysicalTransport::Lola
            || self.sink.physical == PhysicalTransport::Lola
    }

    fn uses_zenoh(&self) -> bool {
        self.source.physical == PhysicalTransport::Zenoh
            || self.sink.physical == PhysicalTransport::Zenoh
    }

    fn uses_mqtt5(&self) -> bool {
        self.source.physical == PhysicalTransport::Mqtt5
            || self.sink.physical == PhysicalTransport::Mqtt5
    }

    fn uses_iceoryx2(&self) -> bool {
        self.source.physical == PhysicalTransport::Iceoryx2
            || self.sink.physical == PhysicalTransport::Iceoryx2
    }

    fn uses_vsomeip(&self) -> bool {
        self.source.physical == PhysicalTransport::Vsomeip
            || self.sink.physical == PhysicalTransport::Vsomeip
    }

    fn uses_dds(&self) -> bool {
        self.source.physical == PhysicalTransport::Dds
            || self.sink.physical == PhysicalTransport::Dds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_criteria() -> MatrixCriteria {
        MatrixCriteria {
            expected: ExpectedCounts {
                pass: 0,
                unsupported: 0,
                blocked: 0,
                failed: 0,
            },
            unsupported_reason_allowlist: vec![
                "source and sink endpoint profiles are identical".to_string()
            ],
            retry: RetryCriteria {
                max_retries_lola_rows: 1,
                max_retries_zenoh_rows: 0,
                all_other_rows: 0,
                max_retried_rows: 0,
                max_retries_consumed: 0,
            },
        }
    }

    fn empty_summary() -> MatrixSummary {
        MatrixSummary {
            schema_version: "1.2",
            generated_at: String::new(),
            row_count: 0,
            pass_count: 0,
            unsupported_count: 0,
            blocked_count: 0,
            failed_count: 0,
            iterations: 1,
            jobs: 4,
            lola_jobs: 1,
            retry_policy: RetryPolicySummary {
                disabled: false,
                lola_max_retries: 1,
                zenoh_max_retries: 0,
                default_max_retries: 0,
            },
            retried_row_count: 0,
            max_retries_consumed: 0,
            artifacts_root: String::new(),
            rows: Vec::new(),
        }
    }

    #[test]
    fn copy_minimized_sink_filter_keeps_full_family() {
        let rows = filter_copy_minimized_sinks(matrix_rows(), true);
        assert_eq!(rows.len(), 720);
        assert_eq!(
            rows.iter()
                .filter(|row| support_status(row).classification == RowClassification::Pass)
                .count(),
            612
        );
    }

    #[test]
    fn repeated_runs_get_unique_artifact_ids() {
        let row = matrix_rows().remove(0);
        assert_eq!(row_run_id(&row, 1, 1), row.id);
        assert_eq!(row_run_id(&row, 100, 7), format!("{}-iteration007", row.id));
    }

    #[test]
    fn concurrency_validation_rejects_zero_and_invalid_limits() {
        assert!(validate_concurrency(0, 1).is_err());
        assert!(validate_concurrency(4, 0).is_err());
        assert!(validate_concurrency(2, 3).is_err());
        assert!(validate_concurrency(4, 1).is_ok());
    }

    #[test]
    fn duplicate_only_selection_is_rejected() {
        let cli = Cli::try_parse_from(["orchestrator", "--only", "same-row", "--only", "same-row"])
            .expect("CLI should parse before semantic validation");
        assert!(validate_cli(&cli)
            .expect_err("duplicate should fail")
            .to_string()
            .contains("duplicate --only"));
    }

    #[test]
    fn canonical_results_are_ordered_by_slot() {
        let ordered = canonical_order(4, vec![(2, "two"), (0, "zero"), (3, "three"), (1, "one")])
            .expect("all canonical slots are present");
        assert_eq!(ordered, ["zero", "one", "two", "three"]);
    }

    #[test]
    fn max_runnable_planning_is_deterministic_across_iterations() {
        let all_rows = matrix_rows();
        let unsupported = all_rows
            .iter()
            .find(|row| support_status(row).classification == RowClassification::Unsupported)
            .expect("matrix has unsupported rows")
            .clone();
        let runnable = all_rows
            .iter()
            .find(|row| support_status(row).classification == RowClassification::Pass)
            .expect("matrix has runnable rows")
            .clone();
        let plan = plan_rows(&[unsupported, runnable], 3, Some(1));

        assert_eq!(plan.slot_count, 6);
        assert_eq!(plan.runnable.len(), 1);
        assert_eq!(plan.runnable[0].slot, 1);
        assert_eq!(plan.runnable[0].payload.iteration, 1);
        let completed: Vec<_> = plan
            .completed
            .iter()
            .map(|(slot, result)| (*slot, result.iteration, result.classification))
            .collect();
        assert_eq!(
            completed,
            [
                (0, 1, RowClassification::Unsupported),
                (2, 2, RowClassification::Unsupported),
                (3, 2, RowClassification::Blocked),
                (4, 3, RowClassification::Unsupported),
                (5, 3, RowClassification::Blocked),
            ]
        );
    }

    #[test]
    fn full_matrix_plan_derives_r11_profile_wire_compatibility_counts() {
        let plan = plan_rows(&matrix_rows(), 1, None);
        assert_eq!(plan.slot_count, 2160);
        assert_eq!(plan.runnable.len(), 1728);
        assert_eq!(plan.completed.len(), 432);
        assert!(plan
            .completed
            .iter()
            .all(|(_, result)| result.classification == RowClassification::Unsupported));
    }

    #[test]
    fn existing_profiles_are_preserved_and_dds_families_are_appended() {
        let ids: Vec<_> = profiles().into_iter().map(|profile| profile.id).collect();
        assert_eq!(
            &ids[..9],
            [
                "zenoh-classic",
                "mqtt5-classic",
                "vsomeip-classic",
                "zenoh-owned-frame",
                "zenoh-copy-minimized",
                "iceoryx2-owned-frame",
                "iceoryx2-copy-minimized",
                "lola-owned-frame",
                "lola-copy-minimized",
            ]
        );
        assert_eq!(
            &ids[9..],
            ["dds-classic", "dds-owned-frame", "dds-copy-minimized"]
        );
    }

    #[test]
    fn derived_criteria_matches_actual_profiles_and_compatibility() {
        let criteria = derived_criteria(&matrix_rows());
        assert_eq!(criteria.expected.pass, 1728);
        assert_eq!(criteria.expected.unsupported, 432);
        assert_eq!(criteria.expected.blocked, 0);
        assert_eq!(criteria.expected.failed, 0);
    }

    #[test]
    fn arrow_and_omgidl_classic_legacy_limit_has_exact_reason() {
        let row = matrix_rows()
            .into_iter()
            .find(|row| {
                row.source.id == "mqtt5-classic"
                    && row.sink.id == "dds-copy-minimized"
                    && row.encoding == WireEncoding::Arrow
            })
            .expect("representative unsupported row exists");
        let status = support_status(&row);
        assert_eq!(status.classification, RowClassification::Unsupported);
        assert_eq!(
            status.reason,
            "Arrow and OMGIDL are not implemented by MQTT5 or vSomeIP classic role binaries"
        );
    }

    #[test]
    fn dds_domains_are_unique_and_origins_are_deterministic_per_row_and_process() {
        let rows: Vec<_> = matrix_rows()
            .into_iter()
            .filter(MatrixRow::uses_dds)
            .collect();
        let domains: BTreeSet<_> = rows.iter().map(dds_domain).collect();
        assert_eq!(domains.len(), rows.len());
        assert!(domains.iter().all(|domain| {
            (DDS_PORT_BASE + DDS_DOMAIN_GAIN * domain) % DDS_PORT_MODULUS
                >= DDS_MIN_UNPRIVILEGED_PORT
        }));

        let row = &rows[17];
        assert_ne!(dds_streamer_origin(row), dds_role_origin(row, true));
        assert_ne!(dds_role_origin(row, true), dds_role_origin(row, false));
        assert_eq!(dds_role_origin(row, true), dds_role_origin(row, true));
    }

    #[test]
    fn dds_role_command_contains_family_wire_domain_origin_and_flow_options() {
        let row = matrix_rows()
            .into_iter()
            .find(|row| {
                row.source.id == "dds-copy-minimized"
                    && row.sink.id == "zenoh-owned-frame"
                    && row.role == RoleStyle::ClientServerRpc
                    && row.encoding == WireEncoding::Omgidl
            })
            .expect("DDS command row exists");
        let cli = Cli::try_parse_from(["orchestrator"]).expect("default CLI parses");
        let command = role_command(
            &row,
            true,
            &ZenohConfigPaths::new(Path::new("/tmp/row")),
            &VsomeipConfigPaths::new(Path::new("/tmp/row")),
            None,
            "unused",
            &cli,
        )
        .expect("DDS command builds");

        assert_eq!(command.binary, "dds_client");
        let rendered = command.args.join(" ");
        assert!(rendered.contains("--route-family copy-minimized"));
        assert!(rendered.contains("--encoding omgidl"));
        assert!(rendered.contains(&format!("--domain-id {}", dds_domain(&row))));
        assert!(rendered.contains(&format!("--origin-id {}", dds_role_origin(&row, true))));
    }

    #[test]
    fn generated_dds_config_contains_lifecycle_and_qos_fields() {
        let row = matrix_rows()
            .into_iter()
            .find(|row| {
                row.source.id == "dds-owned-frame"
                    && row.sink.id == "dds-copy-minimized"
                    && row.role == RoleStyle::PublisherSubscriber
                    && row.encoding == WireEncoding::Arrow
            })
            .expect("DDS config row exists");
        let root = std::env::temp_dir().join(format!(
            "streamer-r11-dds-config-{}-{}",
            std::process::id(),
            row.ordinal
        ));
        fs::create_dir(&root).expect("create config test root");
        let config_path = root.join("config.json");
        write_config(
            Path::new("/workspace"),
            &row,
            &config_path,
            &ZenohConfigPaths::new(&root),
            &VsomeipConfigPaths::new(&root),
            None,
            "unused",
        )
        .expect("write DDS config");
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(&config_path).expect("read config"))
                .expect("parse config");
        let dds = &config["transports"]["dds"];
        assert_eq!(dds["domain_id"], dds_domain(&row));
        assert_eq!(dds["origin_id"], dds_streamer_origin(&row));
        assert_eq!(dds["qos"]["reliability"], "reliable");
        assert_eq!(dds["history_depth"], 32);
        assert_eq!(dds["readiness"]["required_matched_readers"], 1);
        assert_eq!(dds["endpoints"].as_array().unwrap().len(), 2);
        assert_eq!(
            dds["endpoints"][0]["forwarding_routes"][0]["wire_format"],
            "arrow"
        );
        fs::remove_dir_all(root).expect("remove config test root");
    }

    #[test]
    fn scheduler_enforces_bounds_and_skips_blocked_lola_work() {
        #[derive(Default)]
        struct State {
            started: usize,
            active: usize,
            max_active: usize,
            active_lola: usize,
            max_active_lola: usize,
            regular_started: usize,
            release: bool,
        }

        let gate = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let tasks = [true, true, false, false]
            .into_iter()
            .enumerate()
            .map(|(slot, lola_sensitive)| ScheduledTask {
                slot,
                lola_sensitive,
                payload: lola_sensitive,
            })
            .collect();
        let worker_gate = Arc::clone(&gate);
        let scheduler = thread::spawn(move || {
            run_bounded(tasks, 3, 1, &Cancellation::default(), move |is_lola| {
                let (state_lock, wake) = &*worker_gate;
                let mut state = state_lock.lock().expect("test state mutex poisoned");
                state.started += 1;
                state.active += 1;
                state.max_active = state.max_active.max(state.active);
                if is_lola {
                    state.active_lola += 1;
                    state.max_active_lola = state.max_active_lola.max(state.active_lola);
                } else {
                    state.regular_started += 1;
                }
                wake.notify_all();
                while !state.release {
                    state = wake.wait(state).expect("test state mutex poisoned");
                }
                state.active -= 1;
                if is_lola {
                    state.active_lola -= 1;
                }
                Ok(())
            })
        });

        let (state_lock, wake) = &*gate;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = state_lock.lock().expect("test state mutex poisoned");
        while state.started < 3 && Instant::now() < deadline {
            let (next, _) = wake
                .wait_timeout(state, Duration::from_millis(20))
                .expect("test state mutex poisoned");
            state = next;
        }
        assert_eq!(
            state.started, 3,
            "three globally eligible tasks should start"
        );
        assert_eq!(
            state.regular_started, 2,
            "regular work must bypass queued LoLa work"
        );
        assert_eq!(state.max_active, 3);
        assert_eq!(state.max_active_lola, 1);
        state.release = true;
        wake.notify_all();
        drop(state);

        let completed = scheduler
            .join()
            .expect("scheduler test thread should not panic")
            .expect("synthetic scheduler should complete");
        assert_eq!(completed.len(), 4);
    }

    #[test]
    fn single_job_scheduler_preserves_fifo_order() {
        let tasks = [false, true, false, true]
            .into_iter()
            .enumerate()
            .map(|(slot, lola_sensitive)| ScheduledTask {
                slot,
                lola_sensitive,
                payload: slot,
            })
            .collect();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let worker_observed = Arc::clone(&observed);

        run_bounded(tasks, 1, 1, &Cancellation::default(), move |slot| {
            worker_observed
                .lock()
                .expect("observed mutex poisoned")
                .push(slot);
            Ok(())
        })
        .expect("single-worker scheduler should complete");

        assert_eq!(
            *observed.lock().expect("observed mutex poisoned"),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn panicking_lola_task_releases_scheduler_without_deadlock() {
        let tasks = vec![
            ScheduledTask {
                slot: 0,
                lola_sensitive: true,
                payload: true,
            },
            ScheduledTask {
                slot: 1,
                lola_sensitive: true,
                payload: false,
            },
        ];

        let error = run_bounded(tasks, 2, 1, &Cancellation::default(), |should_panic| {
            assert!(!should_panic, "synthetic worker panic");
            Ok(())
        })
        .expect_err("worker panic should become a scheduler error");

        assert!(error.to_string().contains("task panicked"));
    }

    #[test]
    fn scheduler_cancellation_stops_pending_dispatch() {
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let started = Arc::new(Mutex::new(0_usize));
        let worker_started = Arc::clone(&started);
        let tasks = (0..4)
            .map(|slot| ScheduledTask {
                slot,
                lola_sensitive: false,
                payload: (),
            })
            .collect();

        let result = run_bounded(tasks, 1, 1, &cancellation, move |()| {
            *worker_started.lock().expect("started mutex poisoned") += 1;
            worker_cancellation.cancel();
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(*started.lock().expect("started mutex poisoned"), 1);
    }

    #[test]
    fn generated_roots_are_unique_and_include_process_id() {
        let repo = Path::new("/repo");
        let first = generated_artifacts_root(
            repo,
            DateTime::parse_from_rfc3339("2026-07-12T10:11:12.123456Z")
                .expect("valid timestamp")
                .with_timezone(&Utc),
            42,
        );
        let second = generated_artifacts_root(
            repo,
            DateTime::parse_from_rfc3339("2026-07-12T10:11:12.123457Z")
                .expect("valid timestamp")
                .with_timezone(&Utc),
            42,
        );
        assert_ne!(first, second);
        assert!(first.to_string_lossy().contains(".123456Z-pid42"));
    }

    #[test]
    fn artifact_root_must_be_empty_and_exclusively_locked() {
        let base = std::env::temp_dir().join(format!(
            "streamer-orchestrator-root-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().expect("timestamp fits")
        ));
        fs::create_dir(&base).expect("create test parent");
        let root = base.join("artifacts");
        let lock = ArtifactRootLock::acquire(&root).expect("fresh root should lock");
        assert!(ArtifactRootLock::acquire(&root).is_err());
        drop(lock);
        fs::write(root.join("evidence.txt"), "preserve").expect("write evidence");
        assert!(ArtifactRootLock::acquire(&root).is_err());
        fs::remove_dir_all(base).expect("remove test directory");
    }

    #[test]
    fn criteria_rejects_consumed_retry() {
        let mut summary = empty_summary();
        assert!(validate_criteria(&summary, &zero_criteria()).is_empty());
        summary.retried_row_count = 1;
        summary.max_retries_consumed = 1;
        let errors = validate_criteria(&summary, &zero_criteria());
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn lola_application_ids_are_unique_per_process_role() {
        let ids = [
            lola_application_id("r12345678", 1),
            lola_application_id("r12345678", 2),
            lola_application_id("r12345678", 3),
        ];
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[0], ids[2]);
        assert_ne!(ids[1], ids[2]);
        assert!(ids.into_iter().all(|id| id < u32::MAX));
    }
}
