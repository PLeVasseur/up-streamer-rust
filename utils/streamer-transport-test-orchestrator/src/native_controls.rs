// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Real-process native mode controls through the ordinary matrix runner.
use super::*;

fn verify_negative_control_proof(
    name: &str,
    status: &std::process::ExitStatus,
    counts: &Option<serde_json::Value>,
    text: &mut String,
    type_name: &str,
    other: &str,
) -> Result<()> {
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
                            let records = structured_capture::read(&path)?;
                            for body in records.lines().filter_map(|line| {
                                line.strip_prefix(streamer_flow_evidence::MARKER)
                            }) {
                                let observation: streamer_flow_evidence::Observation =
                                    serde_json::from_str(body).context(
                                        "malformed SingleType negative-control message evidence",
                                    )?;
                                anyhow::ensure!(
                                    observation.stage != "rx" && observation.stage != "egress",
                                    "negative control received or forwarded payload"
                                );
                            }
                            text.push_str(&records);
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
    Ok(())
}

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
                process.terminate()?;
                return Err(anyhow!("{name}: control deadline expired"));
            }
            thread::sleep(Duration::from_millis(20));
        };
        // Group cleanup only: this driver's exit contract is checked explicitly below.
        process.terminate()?;
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
            verify_negative_control_proof(name, &status, &counts, &mut text, type_name, other)?;
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

#[cfg(test)]
mod dedicated_negative_tests {
    use super::*;
    const MODE: &str = "UPROTOCOL_PRIVATE_NATIVE_NEGATIVE_CHILD";
    const CHILD: &str = "native_controls::dedicated_negative_tests::actual_negative_producer_child";
    const WRONG: &str = "fixture.WrongNativeType";

    #[test]
    fn actual_negative_producer_child() {
        let Ok(mode) = std::env::var(MODE) else {
            println!("NATIVE_NEGATIVE_CHILD_STUB_NO_CREDIT");
            return;
        };
        if mode.starts_with("driver") {
            if mode != "driver_missing_reason" {
                println!("UnknownRepresentation: controlled driver rejection");
            }
            println!("NATIVE_NEGATIVE_DRIVER_DONE");
            std::io::stdout().flush().unwrap();
            std::process::exit(if mode == "driver_success" { 0 } else { 42 });
        }
        streamer_flow_evidence::prepare_output_from_env().unwrap();
        if mode != "empty" {
            let message = up_rust::UMessageBuilder::publish(
                up_rust::UUri::try_from("//source/1234/1/1/8001").unwrap(),
            )
            .build_with_payload(vec![1_u8, 2, 3], up_rust::PayloadEncoding::RAW)
            .unwrap();
            let stage = if mode == "legacy_raw" {
                "tx"
            } else {
                mode.as_str()
            };
            let record = streamer_flow_evidence::Observation::message(
                stage,
                (stage == "egress").then_some("fixture-source -> fixture-sink"),
                &message,
            )
            .unwrap()
            .expect("enabled actual writer");
            if mode == "legacy_raw" {
                println!(
                    "{}{}",
                    streamer_flow_evidence::MARKER,
                    serde_json::to_string(&record).unwrap()
                );
            } else {
                streamer_flow_evidence::Observation::emit(Some(record));
            }
        }
        println!("NATIVE_NEGATIVE_PRODUCER_DONE mode={mode}");
    }

    struct Fixture {
        directory: PathBuf,
        driver: RunningProcess,
        actors: Vec<RunningProcess>,
        summary: Option<serde_json::Value>,
    }
    impl Fixture {
        fn child(directory: &Path, actor: &str, mode: &str) -> RunningProcess {
            let log = directory.join(format!("{actor}.log"));
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", CHILD, "--nocapture"])
                .env(MODE, mode);
            if actor != "driver" {
                let environment = structured_capture::prepare(
                    directory,
                    actor,
                    &[
                        (
                            "UPROTOCOL_FLOW_EVIDENCE_RUN".into(),
                            "native-negative-fixture".into(),
                        ),
                        ("UPROTOCOL_FLOW_EVIDENCE_ACTOR".into(), actor.into()),
                    ],
                )
                .unwrap();
                atomic_write_json(
                    &log.with_extension("launch.json"),
                    &json!({"environment":environment,"log_path":log}),
                )
                .unwrap();
                command.envs(environment);
            }
            let stdout = log_file(&log).unwrap();
            let stderr = stdout.try_clone().unwrap();
            command
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr));
            let mut child = RunningProcess::spawn(&mut command, actor, log).unwrap();
            wait_for_exit(
                &mut child,
                Duration::from_secs(30),
                &Cancellation::default(),
            )
            .expect("INCONCLUSIVE: controlled producer completion");
            let status = child.child.try_wait().unwrap().unwrap();
            if actor == "driver" {
                assert_eq!(
                    status.code(),
                    Some(if mode == "driver_success" { 0 } else { 42 })
                );
            } else {
                assert!(
                    status.success(),
                    "INCONCLUSIVE: actual record producer failed"
                );
            }
            child
                .terminate()
                .expect("INCONCLUSIVE: controlled producer group cleanup");
            assert!(
                !streamer_transport_test_orchestrator::supervision::process_group_exists(
                    child.process_group()
                )
                .unwrap()
            );
            child
        }
        fn new(label: &str, passive: &str, streamer: &str, driver: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "native-negative-proof-{label}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir(&directory).unwrap();
            let driver = Self::child(&directory, "driver", driver);
            let actors = [
                ("active", "tx"),
                ("passive", passive),
                ("streamer", streamer),
            ]
            .into_iter()
            .map(|(actor, mode)| Self::child(&directory, actor, mode))
            .collect();
            let summary = Some(
                json!({"pass_count":0,"retried_row_count":0,"cleanup":{"process_group_leaks":0,"process_groups_started":4,"process_group_leak_checks":4},"rows":[{"artifact_dir":directory}]}),
            );
            atomic_write_json(&directory.join("summary.json"), &summary).unwrap();
            Self {
                directory,
                driver,
                actors,
                summary,
            }
        }
        fn proof(&mut self) -> Result<()> {
            let status = self.driver.child.try_wait().unwrap().unwrap();
            let mut text = fs::read_to_string(&self.driver.log_path).unwrap();
            verify_negative_control_proof(
                "fixture-negative",
                &status,
                &self.summary,
                &mut text,
                WRONG,
                WRONG,
            )
        }
        fn causal_prior(&mut self, actor: &str, stage: &str) {
            assert_eq!(
                self.driver.child.try_wait().unwrap().unwrap().code(),
                Some(42)
            );
            assert!(fs::read_to_string(&self.driver.log_path)
                .unwrap()
                .contains("UnknownRepresentation"));
            assert_eq!(self.summary.as_ref().unwrap()["pass_count"], 0);
            assert_eq!(self.summary.as_ref().unwrap()["retried_row_count"], 0);
            assert_eq!(
                self.summary.as_ref().unwrap()["cleanup"]["process_group_leaks"],
                0
            );
            assert_eq!(
                self.summary.as_ref().unwrap()["cleanup"]["process_groups_started"],
                self.summary.as_ref().unwrap()["cleanup"]["process_group_leak_checks"]
            );
            for process in &self.actors {
                let raw = fs::read_to_string(&process.log_path).unwrap();
                assert!(!raw.contains("\"stage\":\"rx\"") && !raw.contains("\"stage\":\"egress\""));
            }
            let records =
                structured_capture::read(&self.directory.join(format!("{actor}.log"))).unwrap();
            let observations: Vec<streamer_flow_evidence::Observation> = records
                .lines()
                .filter_map(|line| line.strip_prefix(streamer_flow_evidence::MARKER))
                .map(|body| serde_json::from_str(body).unwrap())
                .collect();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].stage, stage);
        }
    }
    // Real RunningProcess Drop cleans each owned group; preserve all small
    // raw/records/launch/summary files for parent-owned causal receipt collection.

    #[test]
    fn quiet_valid_rejection_with_source_tx_remains_accepted() {
        let mut f = Fixture::new("quiet", "empty", "empty", "driver_reject");
        assert!(f.proof().is_ok());
    }
    #[test]
    fn declared_rx_is_rejected_despite_valid_driver_rejection() {
        let mut f = Fixture::new("hidden-rx", "rx", "empty", "driver_reject");
        f.causal_prior("passive", "rx");
        println!("NATIVE_NEGATIVE_VALID_REJECTION_WITH_DEDICATED_RX");
        assert!(
            f.proof().is_err(),
            "actual negative control accepted hidden dedicated RX"
        );
    }
    #[test]
    fn declared_egress_is_rejected_despite_valid_driver_rejection() {
        let mut f = Fixture::new("hidden-egress", "empty", "egress", "driver_reject");
        f.causal_prior("streamer", "egress");
        println!("NATIVE_NEGATIVE_VALID_REJECTION_WITH_DEDICATED_EGRESS");
        assert!(
            f.proof().is_err(),
            "actual negative control accepted hidden dedicated EGRESS"
        );
    }
    #[test]
    fn status_reason_summary_and_human_no_traffic_conditions_remain_enforced() {
        for mode in [
            "driver_success",
            "driver_missing_reason",
            "pass_count",
            "retry_count",
            "raw_rx",
            "raw_egress",
        ] {
            let driver = if mode.starts_with("driver") {
                mode
            } else {
                "driver_reject"
            };
            let mut f = Fixture::new(mode, "empty", "empty", driver);
            match mode {
                "pass_count" => f.summary.as_mut().unwrap()["pass_count"] = json!(1),
                "retry_count" => f.summary.as_mut().unwrap()["retried_row_count"] = json!(1),
                "raw_rx" | "raw_egress" => {
                    let mut out = OpenOptions::new()
                        .append(true)
                        .open(&f.driver.log_path)
                        .unwrap();
                    writeln!(
                        out,
                        "{{\"stage\":\"{}\"}}",
                        if mode == "raw_rx" { "rx" } else { "egress" }
                    )
                    .unwrap();
                    out.sync_all().unwrap();
                }
                _ => {}
            }
            assert!(f.proof().is_err());
        }
    }
    #[test]
    fn malformed_missing_fields_and_truncated_role_records_are_rejected() {
        for mutation in ["malformed", "missing_fields", "truncated"] {
            let mut f = Fixture::new(mutation, "empty", "empty", "driver_reject");
            let records = f.directory.join("active.records.jsonl");
            match mutation {
                "malformed" => fs::write(records, "FLOW_MESSAGE_EVIDENCE {broken}\n").unwrap(),
                "missing_fields" => fs::write(records, "FLOW_MESSAGE_EVIDENCE {}\n").unwrap(),
                "truncated" => {
                    let mut bytes = fs::read(&records).unwrap();
                    assert_eq!(bytes.pop(), Some(b'\n'));
                    fs::write(records, bytes).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(f.proof().is_err());
        }
    }
    #[test]
    fn missing_and_foreign_declared_role_capture_are_rejected() {
        for mutation in ["missing_records", "missing_launch", "foreign_both_bindings"] {
            let mut f = Fixture::new(mutation, "empty", "empty", "driver_reject");
            let log = f.directory.join("active.log");
            if mutation == "missing_records" {
                fs::remove_file(log.with_extension("records.jsonl")).unwrap();
            } else if mutation == "missing_launch" {
                fs::remove_file(log.with_extension("launch.json")).unwrap();
            } else {
                let launch = log.with_extension("launch.json");
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&launch).unwrap()).unwrap();
                let foreign = Path::new("/foreign").join(f.directory.file_name().unwrap());
                for pair in value["environment"].as_array_mut().unwrap() {
                    let key = pair[0].as_str().unwrap();
                    if key == "UPROTOCOL_STRUCTURED_EVIDENCE_DIRECTORY" {
                        pair[1] = json!(foreign);
                    } else if key == "UPROTOCOL_STRUCTURED_EVIDENCE_PATH" {
                        pair[1] = json!(foreign.join("active.records.jsonl"));
                    }
                }
                atomic_write_json(&launch, &value).unwrap();
            }
            assert!(f.proof().is_err());
        }
    }
    #[test]
    fn declared_empty_capture_never_accepts_raw_only_source_proof() {
        let mut f = Fixture::new("raw-only", "empty", "empty", "driver_reject");
        let raw = fs::read_to_string(f.directory.join("active.records.jsonl")).unwrap();
        fs::write(f.directory.join("active.records.jsonl"), b"").unwrap();
        let mut out = OpenOptions::new()
            .append(true)
            .open(f.directory.join("active.log"))
            .unwrap();
        out.write_all(raw.as_bytes()).unwrap();
        out.sync_all().unwrap();
        assert!(f.proof().is_err());
    }
}
