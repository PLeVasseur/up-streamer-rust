// SPDX-License-Identifier: Apache-2.0
//! Sustained workloads over existing prepared matrix rows and owned process execution.
use super::*;
use std::io::{Seek, SeekFrom};
use streamer_soak::control::{Phase, PhaseCommand};
use streamer_soak::{JournalWriter, Reconciler, ReplayOptions};
use streamer_transport_test_orchestrator::platform::{process_tree_samples, ProcessSample};

mod representative;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(super) enum Lane {
    #[default]
    Fast,
    Representative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Mode {
    Diagnostic,
    Calibration,
    Acceptance,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    #[serde(default)]
    lane: Lane,
    schema: String,
    mode: Mode,
    source_identity: String,
    bundle_sha256: String,
    executable_identities: BTreeMap<String, String>,
    driver_sha256: String,
    calibration_sha256: Option<String>,
    rows: Vec<String>,
    warmup_seconds: u64,
    normal_seconds: u64,
    slow_seconds: u64,
    drain_seconds: u64,
    maximum_seconds: u64,
    offered_per_second: u32,
    consumer_delay_ms: u64,
    queue_capacity: u64,
    pending_limit: usize,
    require_fifo: bool,
    maximum_payload_bytes: usize,
    rss_max: u64,
    pss_max: u64,
    shared_pss_max: u64,
    growth_bytes_per_minute: u64,
    diagnostic_log_level: String,
    allocator_arena_limit: u32,
    fd_max: u64,
    thread_max: u64,
}

impl Plan {
    fn validate(&self) -> Result<()> {
        if self.lane == Lane::Representative {
            return representative::validate_plan(self);
        }
        let durations = (
            self.warmup_seconds,
            self.normal_seconds,
            self.slow_seconds,
            self.drain_seconds,
            self.maximum_seconds,
        );
        anyhow::ensure!(
            self.schema == "uprotocol.sustained-workload.v1"
                && durations
                    == if self.mode == Mode::Diagnostic {
                        (3, 20, 20, 5, 180)
                    } else {
                        (20, 210, 210, 30, 600)
                    }
                && self.offered_per_second == 10
                && self.consumer_delay_ms == 250
                && self.queue_capacity == 4
                && self.pending_limit == 4096
                && !self.require_fifo
                && self.maximum_payload_bytes == 8192
                && self.rss_max == 6 * 1024u64.pow(3)
                && self.pss_max == 5 * 1024u64.pow(3)
                && self.shared_pss_max == 1024u64.pow(3)
                && self.growth_bytes_per_minute == 8 * 1024 * 1024
                && self.fd_max == 32768
                && self.thread_max == 4096
                && self.diagnostic_log_level == "warn,up_streamer=info"
                && self.allocator_arena_limit == 2,
            "workload profile was changed or is unsupported"
        );
        let canonical = matrix_rows();
        let mut ids = BTreeSet::new();
        for id in &self.rows {
            let row = canonical
                .iter()
                .find(|row| &row.id == id)
                .context("unknown workload row")?;
            anyhow::ensure!(
                ids.insert(id) && support_status(row).classification == RowClassification::Pass,
                "duplicate or unsupported workload row"
            );
        }
        anyhow::ensure!(
            self.rows.len() == if self.mode == Mode::Diagnostic { 1 } else { 30 },
            "partial cohort"
        );
        if self.mode != Mode::Diagnostic {
            let expected: BTreeSet<_> = canonical
                .iter()
                .filter(|row| {
                    [RoleStyle::PublisherSubscriber, RoleStyle::ClientServerRpc].contains(&row.role)
                        && [
                            ("iceoryx2-copy-minimized", "zenoh-copy-minimized"),
                            ("dds-classic", "zenoh-classic"),
                            ("iceoryx2-copy-minimized", "dds-classic"),
                        ]
                        .contains(&(row.source.id, row.sink.id))
                })
                .map(|r| &r.id)
                .collect();
            anyhow::ensure!(ids == expected, "cohort coverage changed");
        }
        Ok(())
    }
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "bounded workload input exceeded"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn selected(cli: &Cli, mode: Mode) -> Result<Vec<MatrixRow>> {
    if cli.soak_lane == Lane::Representative {
        anyhow::ensure!(cli.only.is_empty(), "representative cohort is fixed");
        return representative::rows(mode);
    }
    let rows = matrix_rows();
    if mode == Mode::Diagnostic {
        let rows = select_rows(rows, &cli.only)?;
        anyhow::ensure!(
            rows.len() == 1 && support_status(&rows[0]).classification == RowClassification::Pass,
            "diagnostic requires exactly one supported --only row"
        );
        return Ok(rows);
    }
    anyhow::ensure!(
        cli.only.is_empty(),
        "cohort coverage is fixed; use diagnostic for a selected row"
    );
    let selected: Vec<_> = rows
        .into_iter()
        .filter(|row| {
            [RoleStyle::PublisherSubscriber, RoleStyle::ClientServerRpc].contains(&row.role)
                && [
                    ("iceoryx2-copy-minimized", "zenoh-copy-minimized"),
                    ("dds-classic", "zenoh-classic"),
                    ("iceoryx2-copy-minimized", "dds-classic"),
                ]
                .contains(&(row.source.id, row.sink.id))
        })
        .collect();
    anyhow::ensure!(
        selected.len() == 30
            && selected
                .iter()
                .all(|r| support_status(r).classification == RowClassification::Pass),
        "cohort must cover thirty supported canonical rows"
    );
    Ok(selected)
}

fn set_context(env: &mut [(String, String)], options: &ReplayOptions, actor: &str) {
    for (key, value) in env {
        if key == "UPROTOCOL_FLOW_EVIDENCE_RUN" {
            value.clone_from(&options.run);
        }
        if key == "UPROTOCOL_FLOW_EVIDENCE_ACTOR" {
            *value = actor.into();
        }
    }
}

struct LogCursor {
    actor: &'static str,
    reader: BufReader<File>,
}
struct Stream {
    directory: PathBuf,
    options: ReplayOptions,
    control: PathBuf,
    namespace: RunningProcess,
    bridge: RunningProcess,
    source: RunningProcess,
    sink: RunningProcess,
    logs: Vec<LogCursor>,
    journal: Option<JournalWriter>,
    oracle: Option<Reconciler>,
}

impl Stream {
    fn record(&mut self, actor: &str, kind: &str, value: serde_json::Value) -> Result<()> {
        let record = self
            .journal
            .as_mut()
            .context("journal closed")?
            .append(actor, kind, value)?;
        self.oracle
            .as_mut()
            .context("oracle closed")?
            .observe(record)
            .with_context(|| format!("{} {actor} {kind}", self.directory.display()))
    }

    fn poll(&mut self) -> Result<()> {
        for index in 0..self.logs.len() {
            for _ in 0..4096 {
                let cursor = &mut self.logs[index];
                let offset = cursor.reader.stream_position()?;
                let mut bytes = Vec::new();
                cursor
                    .reader
                    .by_ref()
                    .take(65_537)
                    .read_until(b'\n', &mut bytes)?;
                anyhow::ensure!(bytes.len() <= 65_536, "worker log line exceeds bound");
                if bytes.is_empty() {
                    break;
                }
                if bytes.last() != Some(&b'\n') {
                    cursor.reader.seek(SeekFrom::Start(offset))?;
                    break;
                }
                let line = std::str::from_utf8(&bytes)?.trim_end();
                let actor = cursor.actor;
                let Some((kind, body)) = line.split_once(' ') else {
                    continue;
                };
                if kind.starts_with("OP_") || (kind == "FLOW_MESSAGE_EVIDENCE" && actor == "bridge")
                {
                    self.record(actor, kind, serde_json::from_str(body)?)?;
                }
            }
        }
        Ok(())
    }

    fn phase(&self, phase: Phase) -> Result<()> {
        atomic_write_json(
            &self.control,
            &PhaseCommand {
                plan_sha256: self.options.plan_sha256.clone(),
                phase,
            },
        )
    }

    fn alive(&mut self) -> Result<()> {
        for process in [
            &mut self.source,
            &mut self.sink,
            &mut self.bridge,
            &mut self.namespace,
        ] {
            anyhow::ensure!(
                process.child.try_wait()?.is_none(),
                "{} exited during workload; {}",
                process.name,
                process.log_path.display()
            );
        }
        Ok(())
    }

    fn finish(&mut self, cancellation: &Cancellation) -> Result<serde_json::Value> {
        self.phase(Phase::Stop)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            self.poll()?;
            if self.source.child.try_wait()?.is_some() && self.sink.child.try_wait()?.is_some() {
                break;
            }
            cancellation.check()?;
            anyhow::ensure!(Instant::now() < deadline, "role shutdown exceeded deadline");
            thread::sleep(Duration::from_millis(25));
        }
        assert_success(&mut self.source)?;
        assert_success(&mut self.sink)?;
        terminate(&mut self.source)?;
        terminate(&mut self.sink)?;
        terminate(&mut self.bridge)?;
        assert_success(&mut self.bridge)?;
        self.poll()?;
        self.record("bridge", "OP_FINISH", json!({"success": true}))?;
        terminate(&mut self.namespace)?;
        let report = self.oracle.take().context("missing oracle")?.finish()?;
        self.journal
            .take()
            .context("missing journal")?
            .finish(true)?;
        atomic_write_json(&self.directory.join("live-report.json"), &report)?;
        atomic_write_json(
            &self.directory.join("cleanup.json"),
            &json!({"success": true,
            "groups": ([&self.source, &self.sink, &self.bridge, &self.namespace].iter().map(|p| json!({
                "group": p.process_group(), "gone": !streamer_transport_test_orchestrator::supervision::process_group_exists(p.process_group()).unwrap_or(true)
            })).collect::<Vec<_>>())}),
        )?;
        Ok(report)
    }

    fn fail(&mut self) {
        let _ = self.phase(Phase::Stop);
        for process in [
            &mut self.source,
            &mut self.sink,
            &mut self.bridge,
            &mut self.namespace,
        ] {
            let _ = terminate(process);
        }
        let _ = self.poll();
        if let Some(journal) = self.journal.take() {
            let _ = journal.finish(false);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    root: &Path,
    repo: &Path,
    bundle: &RunBundle,
    row: &MatrixRow,
    cli: &Cli,
    plan: &Plan,
    plan_sha256: &str,
    cancellation: &Cancellation,
) -> Result<Stream> {
    let prepared::PreparedRow {
        row_dir,
        lola_run_namespace,
        lola_manifest_paths,
        mut process_env,
        mut active_env,
        mut passive_env,
        ..
    } = prepared::prepare_row(root, bundle, row, cli, 0, 0)?;
    let options = ReplayOptions {
        run: format!("{:x}", Sha256::digest(row_dir.to_string_lossy().as_bytes())),
        source_identity: plan.source_identity.clone(),
        plan_sha256: plan_sha256.into(),
        role: if row.role == RoleStyle::ClientServerRpc {
            streamer_soak::Role::ClientServerRpc
        } else {
            streamer_soak::Role::PublisherSubscriber
        },
        queue_capacity: plan.queue_capacity,
        pending_limit: plan.pending_limit,
        require_fifo: plan.require_fifo,
    };
    atomic_write_json(&row_dir.join("replay-options.json"), &options)?;
    let control = row_dir.join("control.json");
    atomic_write_json(
        &control,
        &PhaseCommand {
            plan_sha256: plan_sha256.into(),
            phase: Phase::Waiting,
        },
    )?;
    let worker = row_dir.join("worker-plan.json");
    atomic_write_json(
        &worker,
        &json!({"replay": options, "control_file": control,
        "offered_per_second": plan.offered_per_second, "consumer_delay_ms": plan.consumer_delay_ms,
        "maximum_payload_bytes": plan.maximum_payload_bytes, "deadline_seconds": plan.maximum_seconds + 30}),
    )?;
    if plan.lane == Lane::Representative {
        representative::configure_worker(&worker, row)?;
    }
    set_context(&mut process_env, &options, "bridge");
    set_context(&mut active_env, &options, "source");
    set_context(&mut passive_env, &options, "sink");
    for env in [&mut process_env, &mut active_env, &mut passive_env] {
        for (key, value) in env.iter_mut() {
            if key == "RUST_LOG" {
                value.clone_from(&plan.diagnostic_log_level);
            }
        }
        // A declared libc allocator control for the Linux execution profile. It
        // does not configure middleware pools or substitute for measured bounds.
        env.push((
            "MALLOC_ARENA_MAX".into(),
            plan.allocator_arena_limit.to_string(),
        ));
    }
    let namespace = start_namespace_holder(bundle, &process_env, &row_dir, cancellation)?;
    let zenoh = ZenohConfigPaths::new(&row_dir);
    let vsomeip = VsomeipConfigPaths::new(&row_dir);
    write_zenoh_router_config(&zenoh.source_router, ZENOH_SOURCE_PORT)?;
    write_zenoh_router_config(&zenoh.sink_router, ZENOH_SINK_PORT)?;
    write_zenoh_client_config(&zenoh.source_client, ZENOH_SOURCE_PORT)?;
    write_zenoh_client_config(&zenoh.sink_client, ZENOH_SINK_PORT)?;
    let config = row_dir.join("configurable-streamer.json");
    write_config(
        repo,
        row,
        &config,
        &zenoh,
        &vsomeip,
        lola_manifest_paths
            .as_ref()
            .map(|paths| paths.streamer.as_path()),
        &lola_run_namespace,
    )?;
    let capacity = if plan.lane == Lane::Representative {
        Some(representative::configure_endpoints(
            &config,
            row,
            &bundle.manifest.native_profile,
            &row_dir,
        )?)
    } else {
        prepared::configure_workload_endpoints(&config)?;
        None
    };
    atomic_write_json(
        &row_dir.join("deployment-inputs.json"),
        &json!({
            "configuration_sha256": sha256_file(&config)?, "worker_sha256": sha256_file(&worker)?,
            "options_sha256": sha256_file(&row_dir.join("replay-options.json"))?,
        }),
    )?;
    let mut bridge = spawn_process(
        bundle,
        "streamer",
        &bundle.executable("configurable-streamer")?,
        &["--config".into(), config.display().to_string()],
        &repo.join("configurable-streamer"),
        &process_env,
        &row_dir,
        Some(&namespace),
    )?;
    wait_for_marker(
        &mut bridge,
        READY_STREAMER,
        Duration::from_secs(10),
        cancellation,
    )?;
    let spawn_role = |active: bool| -> Result<RunningProcess> {
        let profile = if active { row.source } else { row.sink };
        let mut command = if profile.kind != EndpointKind::Classic {
            let mut args = generic_args(
                row,
                profile,
                if active { AUTHORITY_A } else { AUTHORITY_B },
                if active { AUTHORITY_B } else { AUTHORITY_A },
                zenoh.client_for_active(active),
                cli,
            );
            args.extend([
                "--transport".into(),
                if profile.physical == PhysicalTransport::Iceoryx2 {
                    "iceoryx2".into()
                } else {
                    "zenoh".into()
                },
                "--role".into(),
                match (row.role, active) {
                    (RoleStyle::PublisherSubscriber, true) => "pub-sender",
                    (RoleStyle::PublisherSubscriber, false) => "pub-receiver",
                    (RoleStyle::ClientServerRpc, true) => "rpc-client",
                    _ => "rpc-server",
                }
                .into(),
            ]);
            RoleCommand {
                binary: "iceoryx2_publisher".into(),
                args,
            }
        } else {
            role_command(
                row,
                active,
                &zenoh,
                &vsomeip,
                None,
                &lola_run_namespace,
                cli,
            )?
        };
        if let Some(capacity) = &capacity {
            command
                .args
                .extend(representative::role_configuration(profile, capacity));
        } else {
            command
                .args
                .extend(prepared::workload_role_configuration(profile));
        }
        command
            .args
            .extend(["--workload-plan".into(), worker.display().to_string()]);
        spawn_process(
            bundle,
            if active { "active" } else { "passive" },
            &bundle.executable(&command.binary)?,
            &command.args,
            repo,
            if active { &active_env } else { &passive_env },
            &row_dir,
            Some(&namespace),
        )
    };
    let mut sink = spawn_role(false)?;
    wait_for_marker(
        &mut sink,
        READY_LISTENER,
        Duration::from_secs(10),
        cancellation,
    )?;
    let mut source = spawn_role(true)?;
    wait_for_marker(
        &mut source,
        READY_LISTENER,
        Duration::from_secs(10),
        cancellation,
    )?;
    atomic_write_json(
        &row_dir.join("owned-groups.json"),
        &json!({
            "source":source.process_group(), "sink":sink.process_group(),
            "bridge":bridge.process_group(), "launcher":namespace.process_group(),
            "controller":std::process::id(),
        }),
    )?;
    let logs = [(&source, "source"), (&sink, "sink"), (&bridge, "bridge")]
        .into_iter()
        .map(|(p, actor)| {
            Ok(LogCursor {
                actor,
                reader: BufReader::new(File::open(&p.log_path)?),
            })
        })
        .collect::<Result<_>>()?;
    let journal = Some(JournalWriter::new(&row_dir, &options, 4 * 1024 * 1024)?);
    let oracle = Some(Reconciler::new(&options)?);
    let mut stream = Stream {
        directory: row_dir,
        options,
        control,
        namespace,
        bridge,
        source,
        sink,
        logs,
        journal,
        oracle,
    };
    stream.record("bridge", "OP_START", json!({"observed_ready": true}))?;
    stream.poll()?;
    Ok(stream)
}

#[derive(Serialize, Deserialize)]
struct Sample {
    elapsed: f64,
    phase: Phase,
    rss: u64,
    pss: u64,
    shared_pss: u64,
    processes: Vec<ProcessSample>,
}

fn sample(_streams: &[Stream], elapsed: f64, phase: Phase) -> Result<Sample> {
    let mut sample = Sample {
        elapsed,
        phase,
        rss: 0,
        pss: 0,
        shared_pss: 0,
        processes: Vec::new(),
    };
    for process in process_tree_samples(std::process::id())? {
        let rss = process.rss;
        let pss = process.pss;
        sample.rss = sample.rss.checked_add(rss).context("RSS sum overflow")?;
        sample.pss = sample.pss.checked_add(pss).context("PSS sum overflow")?;
        sample.shared_pss = sample
            .shared_pss
            .checked_add(process.shared_pss)
            .context("shared PSS overflow")?;
        sample.processes.push(process);
    }
    Ok(sample)
}

pub(super) fn run(cli: &Cli) -> Result<()> {
    HostOs::current().require_linux_namespaces()?;
    let mode = cli.soak.context("missing workload mode")?;
    let rows = selected(cli, mode)?;
    let repo = fs::canonicalize(repo_root()?)?;
    let root = cli
        .artifacts_root
        .as_ref()
        .context("soak requires a fresh --artifacts-root")?;
    anyhow::ensure!(!root.exists(), "use a fresh workload artifact directory");
    fs::create_dir_all(root)?;
    let root = fs::canonicalize(root)?;
    reject_private_mount_path("workload artifacts", &root)?;
    let result = execute(cli, mode, &rows, &repo, &root);
    if let Err(error) = &result {
        atomic_write_json(
            &root.join("soak-summary.json"),
            &json!({"success": false, "error": format!("{error:#}")}),
        )?;
    }
    result
}

fn execute(cli: &Cli, mode: Mode, rows: &[MatrixRow], repo: &Path, root: &Path) -> Result<()> {
    let cancellation = Cancellation::default();
    install_signal_handlers(&cancellation)?;
    reset_cleanup_counters();
    let target = resolve_target_directory(repo)?;
    let _lock = BuildLock::acquire(&target)?;
    let target = if cli.skip_build {
        discover_existing_matrix_target_directory(&target, rows)?
    } else {
        build_required_binaries_with_optimization(
            repo,
            &target,
            rows,
            false,
            true,
            cli.soak_lane == Lane::Representative,
        )?
        .1
    };
    if cli.soak_lane == Lane::Representative && !cli.skip_build {
        representative::build_workers(repo, &target, rows, root)?;
    }
    let bundle = stage_run_bundle(repo, &target, root, rows)?;
    if cli.soak_lane == Lane::Representative {
        representative::require_inventory(&bundle.manifest.native_profile)?;
    }
    probe_user_namespaces(&bundle)?;
    let manifest = cli
        .soak_source_manifest
        .as_ref()
        .context("--soak-source-manifest is required")?;
    let _: serde_json::Value = read(manifest)?;
    fs::copy(manifest, root.join("source-manifest.json"))?;
    let source_identity = sha256_file(&root.join("source-manifest.json"))?;
    let executable_identities: BTreeMap<_, _> = bundle
        .manifest
        .files
        .iter()
        .map(|file| (file.name.clone(), file.sha256.clone()))
        .collect();
    let driver_sha256 = sha256_file(&std::env::current_exe()?)?;
    let calibration_sha256 = if mode == Mode::Acceptance {
        let previous = cli
            .soak_calibration_root
            .as_ref()
            .context("acceptance requires concurrent calibration")?;
        let proof = audit(previous)?;
        let previous_plan: Plan = read(&previous.join("soak-plan.json"))?;
        anyhow::ensure!(
            previous_plan.mode == Mode::Calibration
                && previous_plan.lane == cli.soak_lane
                && previous_plan.source_identity == source_identity
                && previous_plan.executable_identities == executable_identities
                && previous_plan.driver_sha256 == driver_sha256,
            "calibration source/profile mismatch"
        );
        atomic_write_json(&root.join("calibration.json"), &proof)?;
        Some(sha256_file(&root.join("calibration.json"))?)
    } else {
        None
    };
    let mut plan = Plan {
        lane: cli.soak_lane,
        schema: "uprotocol.sustained-workload.v1".into(),
        mode,
        source_identity,
        bundle_sha256: sha256_file(&bundle.manifest_path)?,
        executable_identities,
        driver_sha256,
        calibration_sha256,
        rows: rows.iter().map(|r| r.id.clone()).collect(),
        warmup_seconds: if mode == Mode::Diagnostic { 3 } else { 20 },
        normal_seconds: if mode == Mode::Diagnostic { 20 } else { 210 },
        slow_seconds: if mode == Mode::Diagnostic { 20 } else { 210 },
        drain_seconds: if mode == Mode::Diagnostic { 5 } else { 30 },
        maximum_seconds: if mode == Mode::Diagnostic { 180 } else { 600 },
        offered_per_second: 10,
        consumer_delay_ms: 250,
        queue_capacity: 4,
        pending_limit: 4096,
        require_fifo: false,
        maximum_payload_bytes: 8192,
        rss_max: 6 * 1024u64.pow(3),
        pss_max: 5 * 1024u64.pow(3),
        shared_pss_max: 1024u64.pow(3),
        growth_bytes_per_minute: 8 * 1024 * 1024,
        diagnostic_log_level: "warn,up_streamer=info".into(),
        allocator_arena_limit: 2,
        fd_max: 32768,
        thread_max: 4096,
    };
    if plan.lane == Lane::Representative {
        representative::set_limits(&mut plan);
    }
    plan.validate()?;
    atomic_write_json(&root.join("soak-plan.json"), &plan)?;
    let plan_hash = sha256_file(&root.join("soak-plan.json"))?;
    let start = Instant::now();
    let mut streams = Vec::new();
    let mut phases = Vec::new();
    let mut resources = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("resources.jsonl"))?;
    let outcome = (|| -> Result<()> {
        for row in rows {
            cancellation.check()?;
            streams.push(prepare(
                root,
                repo,
                &bundle,
                row,
                cli,
                &plan,
                &plan_hash,
                &cancellation,
            )?);
            anyhow::ensure!(
                start.elapsed() < Duration::from_secs(90),
                "cohort readiness deadline expired"
            );
        }
        for (phase, seconds) in [
            (Phase::Warmup, plan.warmup_seconds),
            (Phase::Normal, plan.normal_seconds),
            (Phase::Slow, plan.slow_seconds),
            (Phase::Drain, plan.drain_seconds),
        ] {
            for stream in &streams {
                stream.phase(phase)?;
            }
            phases.push(json!({"phase": phase, "elapsed_seconds": start.elapsed().as_secs_f64(), "seconds": seconds}));
            let until = Instant::now() + Duration::from_secs(seconds);
            let mut next_sample = Instant::now();
            while Instant::now() < until {
                cancellation.check()?;
                anyhow::ensure!(
                    start.elapsed().as_secs() < plan.maximum_seconds,
                    "cohort maximum elapsed time exceeded"
                );
                for stream in &mut streams {
                    stream.poll()?;
                    stream.alive()?;
                }
                if Instant::now() >= next_sample && matches!(phase, Phase::Normal | Phase::Slow) {
                    let value = sample(&streams, start.elapsed().as_secs_f64(), phase)?;
                    serde_json::to_writer(&mut resources, &value)?;
                    resources.write_all(b"\n")?;
                    resources.flush()?;
                    anyhow::ensure!(
                        value.rss <= plan.rss_max
                            && value.pss <= plan.pss_max
                            && value.shared_pss <= plan.shared_pss_max,
                        "aggregate memory bound exceeded: rss={} pss={} shared_pss={}",
                        value.rss,
                        value.pss,
                        value.shared_pss
                    );
                    next_sample = Instant::now() + Duration::from_secs(1);
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
        for stream in &streams {
            stream.phase(Phase::Stop)?;
        }
        for stream in &mut streams {
            stream.finish(&cancellation)?;
        }
        Ok(())
    })();
    if outcome.is_err() {
        for stream in &mut streams {
            stream.fail();
        }
    }
    resources.sync_all()?;
    atomic_write_json(
        &root.join("execution.json"),
        &json!({"success": outcome.is_ok(), "phases": phases,
        "elapsed_seconds": start.elapsed().as_secs_f64(), "cleanup": process_cleanup_summary()}),
    )?;
    outcome?;
    let proof = audit_evidence(root)?;
    atomic_write_json(&root.join("soak-audit.json"), &proof)?;
    anyhow::ensure!(
        start.elapsed().as_secs() <= plan.maximum_seconds,
        "audit exceeded workload deadline"
    );
    atomic_write_json(
        &root.join("soak-summary.json"),
        &json!({"success": true, "plan_sha256": plan_hash,
        "elapsed_seconds": start.elapsed().as_secs_f64(), "audit_sha256": sha256_file(&root.join("soak-audit.json"))?}),
    )?;
    println!(
        "SUSTAINED_WORKLOAD_RESULT={}",
        root.join("soak-summary.json").display()
    );
    Ok(())
}

fn audit_evidence(root: &Path) -> Result<serde_json::Value> {
    let plan: Plan = read(&root.join("soak-plan.json"))?;
    plan.validate()?;
    let hash = sha256_file(&root.join("soak-plan.json"))?;
    anyhow::ensure!(
        sha256_file(&root.join("source-manifest.json"))? == plan.source_identity
            && sha256_file(&root.join("run-bundle/manifest.json"))? == plan.bundle_sha256,
        "workload input identity changed"
    );
    let execution: serde_json::Value = read(&root.join("execution.json"))?;
    anyhow::ensure!(
        execution["success"] == true && execution["cleanup"]["process_group_leaks"] == 0,
        "execution/cleanup failed"
    );
    let phases = execution["phases"]
        .as_array()
        .context("missing workload schedule")?;
    anyhow::ensure!(phases.len() == 4, "partial workload schedule");
    for (index, (phase, seconds)) in [
        ("warmup", plan.warmup_seconds),
        ("normal", plan.normal_seconds),
        ("slow", plan.slow_seconds),
        ("drain", plan.drain_seconds),
    ]
    .into_iter()
    .enumerate()
    {
        let start = phases[index]["elapsed_seconds"]
            .as_f64()
            .context("missing phase start")?;
        let end = if index < 3 {
            &phases[index + 1]["elapsed_seconds"]
        } else {
            &execution["elapsed_seconds"]
        };
        let end = end.as_f64().context("missing phase end")?;
        anyhow::ensure!(
            phases[index]["phase"] == phase
                && phases[index]["seconds"] == seconds
                && start >= 0.0
                && end - start >= seconds as f64
                && end <= plan.maximum_seconds as f64,
            "incomplete or changed phase duration"
        );
    }
    if let Some(calibration_hash) = &plan.calibration_sha256 {
        anyhow::ensure!(
            &sha256_file(&root.join("calibration.json"))? == calibration_hash,
            "calibration changed"
        );
        let calibration: serde_json::Value = read(&root.join("calibration.json"))?;
        anyhow::ensure!(
            calibration["success"] == true
                && calibration["mode"] == "calibration"
                && calibration["source_identity"] == plan.source_identity,
            "invalid calibration proof"
        );
    } else {
        anyhow::ensure!(
            plan.mode != Mode::Acceptance,
            "acceptance lacks calibration"
        );
    }
    let mut reports = Vec::new();
    for id in &plan.rows {
        anyhow::ensure!(
            !id.contains('/') && !id.contains(".."),
            "invalid row locator"
        );
        let directory = root.join(id);
        let inputs: serde_json::Value = read(&directory.join("deployment-inputs.json"))?;
        anyhow::ensure!(
            inputs["configuration_sha256"]
                == sha256_file(&directory.join("configurable-streamer.json"))?
                && inputs["worker_sha256"] == sha256_file(&directory.join("worker-plan.json"))?
                && inputs["options_sha256"] == sha256_file(&directory.join("replay-options.json"))?,
            "deployment input changed"
        );
        let options: ReplayOptions = read(&directory.join("replay-options.json"))?;
        anyhow::ensure!(
            options.plan_sha256 == hash
                && options.source_identity == plan.source_identity
                && options.queue_capacity == plan.queue_capacity
                && options.pending_limit == plan.pending_limit
                && options.require_fifo == plan.require_fifo,
            "row context mismatch"
        );
        let report = streamer_soak::replay(&directory, &options)?;
        let row = if plan.lane == Lane::Representative {
            representative::rows(plan.mode)?
        } else {
            matrix_rows()
        }
        .into_iter()
        .find(|row| &row.id == id)
        .context("row absent")?;
        if row.source.kind == EndpointKind::CopyMinimized {
            anyhow::ensure!(
                report["application_lease_peaks"]["source"][1]
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= 64),
                "native source TX loans not measured"
            );
            if row.role == RoleStyle::ClientServerRpc {
                anyhow::ensure!(
                    report["application_lease_peaks"]["source"][0]
                        .as_u64()
                        .is_some_and(|n| n > 0 && n <= 64),
                    "native response RX loans not measured"
                );
            }
        }
        if row.sink.kind == EndpointKind::CopyMinimized {
            anyhow::ensure!(
                report["application_lease_peaks"]["sink"][0]
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= 64),
                "native sink RX loans not measured"
            );
        }
        if row.encoding == WireEncoding::Native {
            let manifest: BundleManifest = read(&root.join("run-bundle/manifest.json"))?;
            let expected = if plan.lane == Lane::Representative {
                representative::expected_identity(&row, &manifest.native_profile)?
            } else {
                expected_native_identity(&manifest.native_profile)?
            };
            let passive_path = directory.join("passive.log");
            validate_native_worker_log(
                &fs::read_to_string(&passive_path)?,
                &passive_path,
                row.sink.kind,
                &expected,
            )?;
            if row.role == RoleStyle::ClientServerRpc {
                let active_path = directory.join("active.log");
                validate_native_worker_log(
                    &fs::read_to_string(&active_path)?,
                    &active_path,
                    row.source.kind,
                    &expected,
                )?;
            }
            validate_native_projection_log(
                &row,
                &fs::read_to_string(directory.join("streamer.log"))?,
                &expected,
            )?;
        }
        let live: serde_json::Value = read(&directory.join("live-report.json"))?;
        if plan.lane == Lane::Representative {
            representative::audit_worker_metrics(&directory, &report)?;
        }
        anyhow::ensure!(report == live, "live/replayed accounting differs");
        let cleanup: serde_json::Value = read(&directory.join("cleanup.json"))?;
        anyhow::ensure!(
            cleanup["success"] == true
                && cleanup["groups"].as_array().is_some_and(
                    |groups| groups.len() == 4 && groups.iter().all(|g| g["gone"] == true)
                ),
            "incomplete row cleanup"
        );
        for (phase, seconds) in [("normal", plan.normal_seconds), ("slow", plan.slow_seconds)] {
            let counts = &report["phases"][phase];
            let offered = counts["offered"]
                .as_u64()
                .context("missing offered count")?;
            let expected = seconds * u64::from(plan.offered_per_second);
            anyhow::ensure!(
                offered.abs_diff(expected) <= (expected / 33).max(3),
                "{id}: offered rate outside declared window"
            );
            anyhow::ensure!(counts["rejected"] == 0, "{id}: source rejection");
            let loss = counts["explicit_application_loss"]
                .as_u64()
                .context("missing loss count")?;
            let processed = counts["processed"]
                .as_u64()
                .context("missing processed count")?;
            anyhow::ensure!(
                if phase == "normal" {
                    loss == 0
                } else {
                    loss * 100 <= offered * 70 && processed * 100 >= offered * 25
                },
                "{id}: phase loss/progress violation"
            );
        }
        reports.push(json!({"row": id, "report": report, "journal_sha256": sha256_file(&directory.join("journal-manifest.json"))?}));
    }
    let mut samples = 0;
    let mut windows: BTreeMap<String, (f64, f64, u64, u64)> = BTreeMap::new();
    let mut previous_time = 0.0;
    for line in BufReader::new(File::open(root.join("resources.jsonl"))?).lines() {
        let line = line?;
        anyhow::ensure!(line.len() <= 65536, "resource record too large");
        let value: Sample = serde_json::from_str(&line)?;
        let rss = value
            .processes
            .iter()
            .try_fold(0u64, |sum, p| sum.checked_add(p.rss))
            .context("RSS overflow")?;
        let pss = value
            .processes
            .iter()
            .try_fold(0u64, |sum, p| sum.checked_add(p.pss))
            .context("PSS overflow")?;
        let shared = value
            .processes
            .iter()
            .try_fold(0u64, |sum, p| sum.checked_add(p.shared_pss))
            .context("shared PSS overflow")?;
        let fds: u64 = value.processes.iter().map(|p| p.fds).sum();
        let threads: u64 = value.processes.iter().map(|p| p.threads).sum();
        anyhow::ensure!(
            value.processes.len() > plan.rows.len() * 4
                && value
                    .processes
                    .iter()
                    .map(|p| p.pid)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == value.processes.len()
                && shared == value.shared_pss
                && shared <= plan.shared_pss_max
                && fds <= plan.fd_max
                && threads <= plan.thread_max,
            "partial process tree or resource bound violation"
        );
        anyhow::ensure!(
            value.elapsed > previous_time
                && (previous_time == 0.0 || value.elapsed - previous_time <= 4.0),
            "invalid resource sample interval"
        );
        previous_time = value.elapsed;
        anyhow::ensure!(
            rss == value.rss && pss == value.pss && rss <= plan.rss_max && pss <= plan.pss_max,
            "resource sums/bounds mismatch"
        );
        let window = windows.entry(format!("{:?}", value.phase)).or_insert((
            value.elapsed,
            value.elapsed,
            pss,
            pss,
        ));
        window.1 = value.elapsed;
        window.3 = pss;
        samples += 1;
    }
    anyhow::ensure!(
        windows.len() == 2 && samples > 0,
        "missing resource windows"
    );
    for (name, seconds, index) in [
        ("Normal", plan.normal_seconds, 1),
        ("Slow", plan.slow_seconds, 2),
    ] {
        let window = windows.get(name).context("missing measured phase")?;
        let start = phases[index]["elapsed_seconds"]
            .as_f64()
            .context("missing phase start")?;
        anyhow::ensure!(
            window.0 >= start
                && window.0 <= start + 4.0
                && window.1 >= start + seconds as f64 - 4.0,
            "resource evidence does not cover the complete phase"
        );
    }
    for (first, last, initial, final_value) in windows.values() {
        let duration = last - first;
        anyhow::ensure!(
            duration >= 15.0
                && (plan.lane == Lane::Representative
                    || (*final_value as f64 - *initial as f64) * 60.0 / duration
                        <= plan.growth_bytes_per_minute as f64),
            "incomplete resource window or excess growth"
        );
    }
    let representative_resources = if plan.lane == Lane::Representative {
        Some(representative::audit_resources(root, &plan, phases)?)
    } else {
        None
    };
    Ok(
        json!({"success": true, "mode": plan.mode, "lane":plan.lane, "representative_resources":representative_resources, "source_identity": plan.source_identity,
        "plan_sha256": hash, "rows": reports, "resource_samples": samples,
        "resources_sha256": sha256_file(&root.join("resources.jsonl"))?, "execution_sha256": sha256_file(&root.join("execution.json"))?}),
    )
}

pub(super) fn audit(root: &Path) -> Result<serde_json::Value> {
    let summary: serde_json::Value = read(&root.join("soak-summary.json"))?;
    let plan: Plan = read(&root.join("soak-plan.json"))?;
    anyhow::ensure!(
        summary["success"] == true
            && summary["elapsed_seconds"]
                .as_f64()
                .is_some_and(|s| s > 0.0 && s <= plan.maximum_seconds as f64)
            && summary["audit_sha256"].as_str()
                == Some(&sha256_file(&root.join("soak-audit.json"))?),
        "incomplete final workload result"
    );
    let result = audit_evidence(root)?;
    let saved: serde_json::Value = read(&root.join("soak-audit.json"))?;
    anyhow::ensure!(
        result == saved,
        "replayed cohort differs from recorded audit: {}",
        first_audit_difference(&result, &saved, "$")
    );
    Ok(result)
}

fn first_audit_difference(
    actual: &serde_json::Value,
    saved: &serde_json::Value,
    path: &str,
) -> String {
    if actual == saved {
        return String::new();
    }
    match (actual, saved) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) if a.keys().eq(b.keys()) => {
            for (key, value) in a {
                let difference = first_audit_difference(value, &b[key], &format!("{path}.{key}"));
                if !difference.is_empty() {
                    return difference;
                }
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                let difference = first_audit_difference(a, b, &format!("{path}[{index}]"));
                if !difference.is_empty() {
                    return difference;
                }
            }
        }
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            return format!("{path}: actual={a}, saved={b}")
        }
        _ => return format!("{path}: value or structure differs"),
    }
    format!("{path}: difference unavailable")
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn pss_slope_json_round_trip_is_exact() {
        let actual = json!({"bytes_per_minute": -20226.821187469406_f64});
        let encoded = serde_json::to_vec(&actual).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(actual, saved);
        let changed = json!({"bytes_per_minute": -20226.82118746941_f64});
        assert_ne!(actual, changed);
        assert!(first_audit_difference(&actual, &changed, "$").starts_with("$.bytes_per_minute:"));
    }
}
