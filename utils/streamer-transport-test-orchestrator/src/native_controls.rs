// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Real-process native mode controls through the ordinary matrix runner.
use super::*;

pub(super) fn run(cli: &Cli) -> Result<()> {
    anyhow::ensure!(
        cli.soak.is_none()
            && !cli.prepare_shards
            && cli.merge_shard_roots.is_empty()
            && cli.native_single_type.is_none()
            && !cli.allow_single_type_over_classic_hops
            && !cli.native_remote_source_classic
            && cli.only.is_empty()
            && !cli.list
            && cli.generate_criteria.is_none()
            && cli.replay_soak.is_none()
            && cli.replay_workload.is_none()
            && cli.shard_count.is_none()
            && cli.run_bundle.is_none()
            && cli.shard_manifest.is_none(),
        "SingleType controls use fixed cases and cannot combine other modes"
    );
    let repo = fs::canonicalize(repo_root()?)?;
    let cancellation = Cancellation::default();
    install_signal_handlers(&cancellation)?;
    let root = cli
        .artifacts_root
        .clone()
        .context("SingleType controls require --artifacts-root")?;
    anyhow::ensure!(!root.exists(), "control output already exists");
    fs::create_dir_all(&root)?;
    let binary = std::env::current_exe()?;
    let selected = "org.eclipse.uprotocol.examples.SelectedWireNativePayloadV1";
    let other = "org.eclipse.uprotocol.streamer.payload_flow.NativeFlowPayloadV1";
    let native =
        "matrix-iceoryx2-copy-minimized-to-zenoh-copy-minimized-publisher-subscriber-native";
    let classic = "matrix-dds-classic-to-zenoh-classic-publisher-subscriber-native";
    let remote = "matrix-zenoh-owned-frame-to-iceoryx2-owned-frame-publisher-subscriber-native";
    let mut results = Vec::new();
    for (name, row, type_name, allow, remote_classic, positive) in [
        ("native_positive", native, selected, false, false, true),
        ("classic_opt_in", classic, selected, true, false, true),
        ("classic_no_opt_in", classic, selected, false, false, false),
        (
            "remote_classic_no_opt_in",
            remote,
            selected,
            false,
            true,
            false,
        ),
        ("equal_size_wrong_type", native, other, false, false, false),
    ] {
        let artifacts = root.join(name);
        let mut argv = vec![
            "--native-single-type".into(),
            type_name.into(),
            "--only".into(),
            row.into(),
            "--artifacts-root".into(),
            artifacts.to_string_lossy().into_owned(),
            "--jobs".into(),
            "1".into(),
            "--hard-max-jobs".into(),
            "2".into(),
            "--lola-jobs".into(),
            "1".into(),
            "--dds-jobs".into(),
            "1".into(),
            "--zenoh-shm-jobs".into(),
            "1".into(),
            "--vsomeip-jobs".into(),
            "1".into(),
            "--mqtt-jobs".into(),
            "1".into(),
            "--send-count".into(),
            "5".into(),
            "--send-interval-ms".into(),
            "50".into(),
            "--timeout-ms".into(),
            "5000".into(),
            "--scenario-timeout-secs".into(),
            "30".into(),
            "--tokio-worker-threads".into(),
            "2".into(),
        ];
        if cli.skip_build {
            argv.push("--skip-build".into());
        }
        if allow {
            argv.push("--allow-single-type-over-classic-hops".into());
        }
        if remote_classic {
            argv.push("--native-remote-source-classic".into());
        }
        argv.push("--disable-row-retries".into());
        let parsed = Cli::try_parse_from(
            std::iter::once(binary.to_string_lossy().into_owned()).chain(argv.iter().cloned()),
        )?;
        validate_cli(&parsed)?;
        let mut command = Command::new(&binary);
        command.args(&argv).current_dir(&repo);
        let log = root.join(format!("{name}.log"));
        let file = File::create(&log)?;
        command
            .stdout(Stdio::from(file.try_clone()?))
            .stderr(Stdio::from(file));
        let mut process = RunningProcess::spawn(&mut command, name, log.clone())?;
        let deadline = Instant::now() + Duration::from_secs(180);
        let status = loop {
            cancellation.check()?;
            if let Some(status) = process.child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                terminate(&mut process)?;
                return Err(anyhow!("{name}: control deadline expired"));
            }
            thread::sleep(Duration::from_millis(20));
        };
        terminate(&mut process)?;
        anyhow::ensure!(
            !streamer_transport_test_orchestrator::supervision::process_group_exists(
                process.process_group()
            )?,
            "control driver process group leaked"
        );
        let summary = artifacts.join("matrix-summary.json");
        let counts = if summary.is_file() {
            Some(serde_json::from_slice::<serde_json::Value>(&fs::read(
                &summary,
            )?)?)
        } else {
            None
        };
        let mut text = fs::read_to_string(&log)?;
        if positive {
            let summary = counts
                .as_ref()
                .context("positive control has no matrix summary")?;
            anyhow::ensure!(
                status.success()
                    && summary["pass_count"] == 1
                    && summary["failed_count"] == 0
                    && summary["retried_row_count"] == 0,
                "{name}: actual traffic control failed"
            );
        } else {
            anyhow::ensure!(!status.success(), "{name}: expected rejection not observed");
            if let Some(summary) = &counts {
                anyhow::ensure!(
                    summary["pass_count"] == 0 && summary["retried_row_count"] == 0,
                    "negative control admitted traffic"
                );
                text.push_str(&summary.to_string());
                if let Some(rows) = summary["rows"].as_array() {
                    for row in rows {
                        if let Some(dir) = row["artifact_dir"].as_str() {
                            for file in ["streamer.log", "active.log", "passive.log"] {
                                let path = Path::new(dir).join(file);
                                if path.is_file() {
                                    text.push_str(&fs::read_to_string(path)?);
                                }
                            }
                        }
                    }
                }
            }
            let expected = if type_name == other {
                "UnknownRepresentation"
            } else {
                "allow_single_type_over_classic_hops"
            };
            anyhow::ensure!(
                text.contains(expected),
                "{name}: expected typed/policy rejection missing"
            );
            anyhow::ensure!(
                !text.contains("\"stage\":\"rx\"") && !text.contains("\"stage\":\"egress\""),
                "negative control received or forwarded payload"
            );
        }
        if let Some(summary) = &counts {
            anyhow::ensure!(
                summary["cleanup"]["process_group_leaks"] == 0
                    && summary["cleanup"]["process_groups_started"]
                        == summary["cleanup"]["process_group_leak_checks"],
                "row cleanup incomplete"
            );
        }
        results.push(json!({"name":name,"expected_positive":positive,"result":"pass","argv":argv,"exit_code":status.code(),
            "artifacts":artifacts,"summary":counts,"log_sha256":sha256_file(&log)?,"driver_group_gone":true}));
        atomic_write_json(
            &root.join("controls-summary.json"),
            &json!({"schema":1,"complete":false,"cases":results}),
        )?;
    }
    atomic_write_json(
        &root.join("controls-summary.json"),
        &json!({"schema":1,"complete":true,"control_count":5,"cases":results,
        "driver_sha256":sha256_file(&binary)?,"source_commit":git_output(&repo,&["rev-parse","HEAD"]),"dependency_sha256":dependency_sha256(&repo)?}),
    )?;
    Ok(())
}
