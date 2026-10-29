// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Direct peer execution using the matrix runner's native deployment and workers.
use super::*;

pub(super) fn run(cli: &Cli) -> Result<()> {
    anyhow::ensure!(
        !cli.single_type_controls
            && cli.native_single_type.is_none()
            && cli.soak.is_none()
            && !cli.prepare_shards
            && cli.shard_count.is_none()
            && cli.merge_shard_roots.is_empty(),
        "direct peers cannot combine bridge or native-mode workflows"
    );
    validate_cli(cli)?;
    HostOs::current().require_linux_namespaces()?;
    let repo = fs::canonicalize(repo_root()?)?;
    let root = cli
        .artifacts_root
        .clone()
        .context("direct peers require --artifacts-root")?;
    anyhow::ensure!(!root.exists(), "direct output already exists");
    fs::create_dir_all(&root)?;
    let cancellation = Cancellation::default();
    install_signal_handlers(&cancellation)?;
    reset_cleanup_counters();
    let all = matrix_rows();
    let mut rows: Vec<_> = all
        .iter()
        .filter(|row| row.source.id == row.sink.id)
        .cloned()
        .collect();
    for (index, row) in rows.iter_mut().enumerate() {
        row.id = format!(
            "direct-{}-{}-{}",
            row.source.id,
            row.role.id(),
            row.encoding.id()
        );
        row.ordinal = index;
    }
    anyhow::ensure!(rows.len() == 180, "direct universe changed");
    let selected = select_rows(rows, &cli.only)?;
    anyhow::ensure!(!selected.is_empty(), "direct selection is empty");
    let target = resolve_target_directory(&repo)?;
    let _lock = BuildLock::acquire(&target)?;
    let target = if cli.skip_build {
        discover_existing_matrix_target_directory(&target, &all)?
    } else {
        build_required_binaries(&repo, &target, &all, false, false)?.1
    };
    let bundle = stage_run_bundle(&repo, &target, &root, &all)?;
    probe_user_namespaces(&bundle)?;
    let mut results = Vec::new();
    for row in selected {
        cancellation.check()?;
        let started = Instant::now();
        let result = execute(&repo, &root, &bundle, &row, cli, &cancellation);
        results.push(json!({"row_id":row.id,"profile":row.source.id,"role":row.role.id(),"encoding":row.encoding.id(),
            "streamer_started":false,"result":if result.is_ok(){"pass"}else{"failed"},
            "error":result.err().map(|e|format!("{e:#}")),"elapsed_seconds":started.elapsed().as_secs_f64(),"retries":0}));
        atomic_write_json(
            &root.join("direct-summary.json"),
            &json!({"schema":1,"complete":false,"candidate_universe":180,"rows":results}),
        )?;
    }
    let success = results.iter().all(|row| row["result"] == "pass");
    let cleanup = process_cleanup_summary();
    let metadata_tools = native_libraries::metadata_tool_summary();
    let matrix_files = bundle
        .manifest
        .files
        .iter()
        .filter(|file| file.kind == BundleFileKind::MatrixExecutable)
        .count() as u64;
    let library_files = bundle
        .manifest
        .files
        .iter()
        .filter(|file| file.kind == BundleFileKind::NativeLibrary)
        .count() as u64;
    // Direct stages once and validates the staged bundle once. Every tool group
    // remains included in global cleanup, independently of the native actor set.
    let expected_metadata_groups = 2 * (matrix_files + library_files);
    let success = success
        && cleanup.process_group_leaks == 0
        && cleanup.process_groups_started == cleanup.process_group_leak_checks
        && metadata_tools.started == expected_metadata_groups
        && metadata_tools.source_executable_inspections == matrix_files
        && metadata_tools.source_library_inspections == library_files
        && metadata_tools.bundle_file_inspections == matrix_files + library_files
        && metadata_tools.started == metadata_tools.cleanup_attempts
        && metadata_tools.cleanup_failures == 0;
    atomic_write_json(
        &root.join("direct-summary.json"),
        &json!({"schema":1,"complete":true,"success":success,"candidate_universe":180,"cleanup":cleanup,"metadata_tool_processes":metadata_tools,"expected_metadata_tool_process_groups":expected_metadata_groups,
        "executed_rows":results.len(),"rows":results,"source_commit":git_output(&repo,&["rev-parse","HEAD"]),
        "dependency_sha256":dependency_sha256(&repo)?,"bundle_sha256":sha256_file(&bundle.manifest_path)?}),
    )?;
    anyhow::ensure!(success, "direct pair failures; inspect direct-summary.json");
    Ok(())
}

fn set_arg(args: &mut [String], flag: &str, value: String) -> Result<()> {
    let index = args
        .iter()
        .position(|arg| arg == flag)
        .with_context(|| format!("missing role argument {flag}"))?;
    *args.get_mut(index + 1).context("missing argument value")? = value;
    Ok(())
}

fn native_peers(prepared: &mut prepared::PreparedRow, bundle: &RunBundle) -> Result<()> {
    let manifest_path = prepared.row_dir.join("native-deployment.json");
    if !manifest_path.exists() {
        return Ok(());
    }
    let artifact = &bundle.manifest.native_profile.0;
    let endpoints = [
        ("source", "native-source-profile.json"),
        ("sink", "native-sink-profile.json"),
    ]
    .into_iter()
    .map(|(id, path)| up_native_profile::DeploymentEndpoint {
        id: id.into(),
        agreement_path: path.into(),
        content_sha256: artifact.content_sha256.clone(),
        target: artifact.target.clone(),
        domain: artifact.domain.clone(),
        generation: artifact.generation,
    })
    .collect();
    let manifest = up_native_profile::DeploymentManifest::new(
        endpoints,
        vec![up_native_profile::DeploymentRoute {
            id: "native-route".into(),
            endpoints: vec!["source".into(), "sink".into()],
        }],
    )?;
    atomic_write_json(&manifest_path, &manifest)?;
    fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o444))?;
    for (env, peer) in [
        (&mut prepared.active_env, "sink"),
        (&mut prepared.passive_env, "source"),
    ] {
        let pair = env
            .iter_mut()
            .find(|(key, _)| key == PEER_ENDPOINT_ENV)
            .context("native peer environment missing")?;
        pair.1 = peer.into();
    }
    Ok(())
}

fn execute(
    repo: &Path,
    root: &Path,
    bundle: &RunBundle,
    row: &MatrixRow,
    cli: &Cli,
    cancellation: &Cancellation,
) -> Result<()> {
    let before = process_cleanup_summary();
    let mut prepared = prepared::prepare_row(root, bundle, row, cli, 1, 0)?;
    native_peers(&mut prepared, bundle)?;
    if row.uses_iceoryx2() {
        let prefix = prepared
            .active_env
            .iter()
            .find(|(key, _)| key == "UP_ICEORYX2_PREFIX")
            .context("source namespace missing")?
            .1
            .clone();
        prepared
            .passive_env
            .iter_mut()
            .find(|(key, _)| key == "UP_ICEORYX2_PREFIX")
            .context("sink namespace missing")?
            .1 = prefix;
    }
    let directory = &prepared.row_dir;
    let source_start_file = directory.join("source-start");
    anyhow::ensure!(
        !source_start_file.exists(),
        "source start marker must be fresh"
    );
    prepared.active_env.push((
        "UPROTOCOL_SOURCE_START_FILE".into(),
        source_start_file.display().to_string(),
    ));
    let source_finish_file = directory.join("source-finish");
    let retain_source = retains_finite_source(row);
    if retain_source {
        anyhow::ensure!(
            !source_finish_file.exists(),
            "source completion marker must be fresh"
        );
        prepared.active_env.push((
            "UPROTOCOL_SOURCE_FINISH_FILE".into(),
            source_finish_file.display().to_string(),
        ));
    }

    let zenoh = ZenohConfigPaths::new(directory);
    write_zenoh_router_config(&zenoh.sink_client, ZENOH_SINK_PORT)?;
    write_zenoh_client_config(&zenoh.source_client, ZENOH_SINK_PORT)?;
    let vsomeip = VsomeipConfigPaths::new(directory);
    if row.uses_vsomeip() {
        let base = 0x1000 + row.ordinal as u16 * 3;
        let source_id = if row.role == RoleStyle::PublisherSubscriber {
            base + 1
        } else {
            UE_ID as u16
        };
        let applications = [("source_app", source_id), ("sink_app", base + 2)];
        let network = format!("direct_vs_{:08x}", stable_hash(&row.id));
        for (path, name, id) in [
            (&vsomeip.source, "source_app", source_id),
            (&vsomeip.sink, "sink_app", base + 2),
        ] {
            write_vsomeip_config(
                path,
                &network,
                name,
                id,
                (UE_ID & 0xffff) as u16,
                ((UE_ID >> 16) as u16).max(1),
                &applications,
            )?;
            let mut config: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
            config["routing"] = json!("sink_app");
            atomic_write_json(path, &config)?;
        }
    }
    let mut active = role_command(
        row,
        true,
        &zenoh,
        &vsomeip,
        prepared
            .lola_manifest_paths
            .as_ref()
            .map(|paths| paths.role(true)),
        &prepared.lola_run_namespace,
        cli,
    )?;
    let mut passive = role_command(
        row,
        false,
        &zenoh,
        &vsomeip,
        prepared
            .lola_manifest_paths
            .as_ref()
            .map(|paths| paths.role(false)),
        &prepared.lola_run_namespace,
        cli,
    )?;
    if row.source.physical == PhysicalTransport::Dds {
        let value = passive.args[passive
            .args
            .iter()
            .position(|a| a == "--domain-id")
            .context("DDS domain missing")?
            + 1]
        .clone();
        set_arg(&mut active.args, "--domain-id", value)?;
    }
    if row.uses_lola() {
        for flag in [
            "--lola-instance-specifier",
            "--lola-service-type",
            "--lola-event-name",
            "--lola-rpc-response-instance-specifier",
            "--lola-rpc-response-service-type",
            "--lola-rpc-response-event-name",
        ] {
            if let Some(index) = active.args.iter().position(|a| a == flag) {
                set_arg(&mut passive.args, flag, active.args[index + 1].clone())?;
            }
        }
    }
    let mut namespace =
        start_namespace_holder(bundle, &prepared.process_env, directory, cancellation)?;
    let mut broker = if row.uses_mqtt5() {
        Some(
            start_mqtt_broker(
                bundle,
                directory,
                &prepared.process_env,
                &namespace,
                cli.mqtt_readiness_timeout_ms,
                cancellation,
            )?
            .0,
        )
    } else {
        None
    };
    let result = (|| {
        let mut receiver = spawn_process(
            bundle,
            "passive",
            &bundle.executable(&passive.binary)?,
            &passive.args,
            repo,
            &prepared.passive_env,
            directory,
            Some(&namespace),
        )?;
        let mut sender = spawn_process(
            bundle,
            "active",
            &bundle.executable(&active.binary)?,
            &active.args,
            repo,
            &prepared.active_env,
            directory,
            Some(&namespace),
        )?;
        // Native receiver setup may need the peer's already-offered service.
        // Keep first traffic behind both readiness checks and the start barrier.
        let (marker, _) = passive_readiness_contract(row);
        wait_for_marker(&mut receiver, marker, Duration::from_secs(10), cancellation)?;
        let started = Instant::now();
        wait_for_marker(
            &mut sender,
            READY_SOURCE_WAITING,
            Duration::from_secs(cli.scenario_timeout_secs),
            cancellation,
        )?;
        allocation::snapshot(directory, "ready-before-first-send", 2)?;
        atomic_write(&source_start_file, b"START\n")?;
        if retain_source {
            wait_for_marker(
                &mut sender,
                "READY source_complete_waiting",
                remaining_timeout(started, cli.scenario_timeout_secs, "direct sends")?,
                cancellation,
            )?;
        } else {
            wait_for_exit(
                &mut sender,
                remaining_timeout(started, cli.scenario_timeout_secs, "direct sender")?,
                cancellation,
            )?;
            assert_success(&mut sender)?;
        }
        if row.uses_classic() {
            wait_for_any_marker(
                &receiver.log_path,
                passive_observation_markers(row),
                remaining_timeout(started, cli.scenario_timeout_secs, "direct receiver")?,
                cancellation,
            )?;
        } else {
            wait_for_exit(
                &mut receiver,
                remaining_timeout(started, cli.scenario_timeout_secs, "direct receiver")?,
                cancellation,
            )?;
            assert_success(&mut receiver)?;
        }
        if retain_source {
            atomic_write(&source_finish_file, b"STOP\n")?;
            wait_for_exit(
                &mut sender,
                remaining_timeout(
                    started,
                    cli.scenario_timeout_secs,
                    "direct source completion",
                )?,
                cancellation,
            )?;
            assert_success(&mut sender)?;
        }
        allocation::snapshot(directory, "completed-direct-peers", 2)?;
        terminate(&mut sender)?;
        terminate(&mut receiver)?;
        flow_oracle::verify_direct(row, directory, bundle)?;
        if row.encoding == WireEncoding::Native {
            let expected = expected_native_identity(&bundle.manifest.native_profile)?;
            let path = directory.join("passive.log");
            validate_native_worker_log(
                &structured_capture::diagnostic_text(&path)?,
                &path,
                row.sink.kind,
                &expected,
            )?;
            if row.role == RoleStyle::ClientServerRpc {
                let path = directory.join("active.log");
                validate_native_worker_log(
                    &structured_capture::diagnostic_text(&path)?,
                    &path,
                    row.source.kind,
                    &expected,
                )?;
            }
            let manifest: up_native_profile::DeploymentManifest =
                up_native_profile::read_artifact(&directory.join("native-deployment.json"))?;
            validate_native_startup_log(
                &structured_capture::read(&directory.join("active.log"))?,
                &manifest,
                &bundle.manifest.native_profile,
                &[("source", "sink")],
            )?;
            validate_native_startup_log(
                &structured_capture::read(&directory.join("passive.log"))?,
                &manifest,
                &bundle.manifest.native_profile,
                &[("sink", "source")],
            )?;
        }
        Ok(())
    })();
    let mut cleanup = Ok(());
    if let Some(broker) = &mut broker {
        cleanup = terminate(broker);
    }
    cleanup = cleanup.and(terminate(&mut namespace));
    let after = process_cleanup_summary();
    if after.process_group_leaks != before.process_group_leaks
        || after.process_groups_started - before.process_groups_started
            != after.process_group_leak_checks - before.process_group_leak_checks
    {
        cleanup = Err(anyhow!("direct row owned-process cleanup incomplete"));
    }
    atomic_write_json(
        &directory.join("direct-cleanup.json"),
        &json!({"streamer_started":false,"success":cleanup.is_ok(),
        "started_groups":after.process_groups_started-before.process_groups_started,
        "checked_groups":after.process_group_leak_checks-before.process_group_leak_checks,
        "leaks":after.process_group_leaks-before.process_group_leaks,
        "active_args":active.args,"passive_args":passive.args}),
    )?;
    result.and(cleanup)
}
