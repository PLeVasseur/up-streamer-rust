// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Live negative controls on the exact role binaries and isolated native domains.
use super::*;

fn verify_bridge_off_proof(
    row: &MatrixRow,
    active: &RunningProcess,
    passive: &RunningProcess,
) -> Result<()> {
    let mut active_log = fs::read_to_string(&active.log_path)?;
    active_log.push('\n');
    active_log.push_str(&structured_capture::read(&active.log_path)?);
    let mut passive_log = fs::read_to_string(&passive.log_path)?;
    passive_log.push('\n');
    passive_log.push_str(&structured_capture::read(&passive.log_path)?);
    let mut received = false;
    let mut source_committed = false;
    for (is_active, log) in [(true, active_log.as_str()), (false, passive_log.as_str())] {
        for body in log
            .lines()
            .filter_map(|line| line.strip_prefix(streamer_flow_evidence::MARKER))
        {
            let record: streamer_flow_evidence::Observation =
                serde_json::from_str(body).context("malformed bridge-off message evidence")?;
            received |= record.stage == "rx";
            source_committed |= is_active && record.stage == "tx";
        }
    }
    if received || passive_log.contains("FLOW observed_payload_bytes") {
        return Err(anyhow!(
            "bridge-off control delivered a message without Streamer for {}",
            row.id
        ));
    }
    if !source_committed
        || !passive_log.contains("DeadlineExceeded")
        || active_log.contains("FLOW_EVIDENCE_ERROR")
        || passive_log.contains("FLOW_EVIDENCE_ERROR")
    {
        return Err(anyhow!(
            "bridge-off control did not establish a valid source and listening sink for {}",
            row.id
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn verify(
    row: &MatrixRow,
    bundle: &RunBundle,
    repo_root: &Path,
    row_dir: &Path,
    process_env: &[(String, String)],
    active_env: &[(String, String)],
    passive_env: &[(String, String)],
    zenoh: &ZenohConfigPaths,
    vsomeip: &VsomeipConfigPaths,
    cli: &Cli,
    cancellation: &Cancellation,
) -> Result<()> {
    if row.source.physical != PhysicalTransport::Iceoryx2
        || row.sink.physical != PhysicalTransport::Iceoryx2
    {
        return Ok(());
    }
    let control = row_dir.join("bridge-off");
    fs::create_dir(&control)?;
    let mut namespace = start_namespace_holder(bundle, process_env, &control, cancellation)?;
    let result = (|| {
        let passive_spec = role_command(row, false, zenoh, vsomeip, None, "", cli)?;
        let active_spec = role_command(row, true, zenoh, vsomeip, None, "", cli)?;
        let mut passive = spawn_process(
            bundle,
            "passive",
            &bundle.executable(&passive_spec.binary)?,
            &passive_spec.args,
            repo_root,
            passive_env,
            &control,
            Some(&namespace),
        )?;
        wait_for_marker(
            &mut passive,
            READY_LISTENER,
            Duration::from_secs(10),
            cancellation,
        )?;
        let mut active = spawn_process(
            bundle,
            "active",
            &bundle.executable(&active_spec.binary)?,
            &active_spec.args,
            repo_root,
            active_env,
            &control,
            Some(&namespace),
        )?;
        wait_for_exit(
            &mut active,
            Duration::from_secs(cli.scenario_timeout_secs),
            cancellation,
        )?;
        wait_for_exit(
            &mut passive,
            Duration::from_secs(cli.scenario_timeout_secs),
            cancellation,
        )?;
        verify_bridge_off_proof(row, &active, &passive)?;
        terminate(&mut passive)?;
        terminate(&mut active)?;
        atomic_write_json(
            &row_dir.join("bridge-control.json"),
            &json!({
                "schema": 1, "row_id": row.id, "streamer_started": false,
                "source_committed": true, "sink_ready": true, "received": 0,
                "verified": true, "source_namespace": iceoryx2_namespace_prefix(&row.id, "source"),
                "sink_namespace": iceoryx2_namespace_prefix(&row.id, "sink"),
                "control_root": control,
            }),
        )
    })();
    result.and(terminate(&mut namespace))
}

#[cfg(test)]
mod dedicated_predicate_tests {
    use super::*;

    const MODE: &str = "UPROTOCOL_PRIVATE_BRIDGE_PROOF_CHILD";
    const CHILD: &str = "bridge_control::dedicated_predicate_tests::actual_record_writer_child";

    #[test]
    fn actual_record_writer_child() {
        let Ok(mode) = std::env::var(MODE) else {
            println!("BRIDGE_PROOF_CHILD_STUB_NO_CREDIT");
            return;
        };
        streamer_flow_evidence::prepare_output_from_env().unwrap();
        let message = up_rust::UMessageBuilder::publish(
            up_rust::UUri::try_from("//source/1234/1/8001").unwrap(),
        )
        .build_with_payload(vec![1_u8, 2, 3], up_rust::PayloadEncoding::RAW)
        .unwrap();
        for stage in match mode.as_str() {
            "tx" | "error" => vec!["tx"],
            "both" => vec!["tx", "rx"],
            "rx" | "rx_deadline" => vec!["rx"],
            "legacy_raw" => vec!["tx"],
            "deadline" | "deadline_error" | "empty" => vec![],
            _ => panic!("unknown owned bridge-proof child mode"),
        } {
            let record = streamer_flow_evidence::Observation::message(stage, None, &message)
                .unwrap()
                .expect("explicit evidence mode enabled");
            if mode == "legacy_raw" {
                // Deliberate current-mode raw-injection negative control.
                println!(
                    "{}{}",
                    streamer_flow_evidence::MARKER,
                    serde_json::to_string(&record).unwrap()
                );
            } else {
                streamer_flow_evidence::Observation::emit(Some(record));
            }
        }
        if matches!(mode.as_str(), "deadline" | "deadline_error" | "rx_deadline") {
            eprintln!("DeadlineExceeded: controlled listening-sink fixture");
        }
        if matches!(mode.as_str(), "error" | "deadline_error") {
            eprintln!("FLOW_EVIDENCE_ERROR controlled fixture");
        }
        println!("BRIDGE_PROOF_WRITER_CHILD_DONE mode={mode}");
    }

    struct Fixture {
        row: MatrixRow,
        directory: PathBuf,
        active: RunningProcess,
        passive: RunningProcess,
    }
    impl Fixture {
        fn child(directory: &Path, role: &str, mode: &str) -> RunningProcess {
            let log = directory.join(format!("{role}.log"));
            let environment = structured_capture::prepare(
                directory,
                role,
                &[
                    (
                        "UPROTOCOL_FLOW_EVIDENCE_RUN".into(),
                        "bridge-proof-fixture".into(),
                    ),
                    ("UPROTOCOL_FLOW_EVIDENCE_ACTOR".into(), role.into()),
                ],
            )
            .unwrap();
            atomic_write_json(
                &log.with_extension("launch.json"),
                &json!({"environment":environment,"log_path":log}),
            )
            .unwrap();
            let stdout = log_file(&log).unwrap();
            let stderr = stdout.try_clone().unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", CHILD, "--nocapture"])
                .envs(environment)
                .env(MODE, mode)
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr));
            let mut process = RunningProcess::spawn(&mut command, role, log).unwrap();
            wait_for_exit(
                &mut process,
                Duration::from_secs(30),
                &Cancellation::default(),
            )
            .expect("INCONCLUSIVE: actual writer fixture child timeout");
            assert_success(&mut process).expect("INCONCLUSIVE: actual writer fixture child failed");
            assert!(fs::read_to_string(&process.log_path)
                .unwrap()
                .contains("BRIDGE_PROOF_WRITER_CHILD_DONE"));
            process
        }
        fn new(label: &str, active_mode: &str, passive_mode: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "streamer-bridge-proof-{label}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap()
            ));
            fs::create_dir(&directory).unwrap();
            let row = matrix_rows()
                .into_iter()
                .find(|row| {
                    row.source.physical == PhysicalTransport::Iceoryx2
                        && row.sink.physical == PhysicalTransport::Iceoryx2
                        && row.role == RoleStyle::PublisherSubscriber
                        && row.encoding == WireEncoding::Protobuf
                        && support_status(row).classification == RowClassification::Pass
                })
                .unwrap();
            let active = Self::child(&directory, "active", active_mode);
            let passive = Self::child(&directory, "passive", passive_mode);
            Self {
                row,
                directory,
                active,
                passive,
            }
        }
        fn proof(&self) -> Result<()> {
            verify_bridge_off_proof(&self.row, &self.active, &self.passive)
        }
    }
    // Actual RunningProcess Drop retains bounded owned-group cleanup. Preserve
    // this small fixture's raw/records/launch files for causal receipt collection.

    #[test]
    fn dedicated_tx_only_with_expected_deadline_is_accepted() {
        let f = Fixture::new("valid", "tx", "deadline");
        let raw = fs::read_to_string(&f.active.log_path).unwrap();
        assert!(!raw.contains(streamer_flow_evidence::MARKER));
        let records = structured_capture::read(&f.active.log_path).unwrap();
        let tx: Vec<streamer_flow_evidence::Observation> = records
            .lines()
            .filter_map(|line| line.strip_prefix(streamer_flow_evidence::MARKER))
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        assert_eq!(tx.len(), 1);
        assert_eq!(tx[0].stage, "tx");
        assert!(structured_capture::read(&f.passive.log_path)
            .unwrap()
            .is_empty());
        assert!(fs::read_to_string(&f.passive.log_path)
            .unwrap()
            .contains("DeadlineExceeded"));
        println!("BRIDGE_PROOF_VALID_DEDICATED_TX_NO_RX_EXPECTED_DEADLINE");
        f.proof()
            .expect("actual unchanged bridge predicate rejected the declared TX channel");
    }

    #[test]
    fn any_declared_reception_remains_rejected() {
        for (active, passive) in [("both", "deadline"), ("tx", "rx_deadline")] {
            let f = Fixture::new("rx", active, passive);
            assert!(f.proof().is_err());
        }
    }

    #[test]
    fn missing_source_or_deadline_remains_rejected() {
        for (active, passive) in [("empty", "deadline"), ("tx", "empty")] {
            let f = Fixture::new("missing", active, passive);
            assert!(f.proof().is_err());
        }
    }

    #[test]
    fn either_human_evidence_error_remains_rejected() {
        for (active, passive) in [("error", "deadline"), ("tx", "deadline_error")] {
            let f = Fixture::new("error", active, passive);
            assert!(f.proof().is_err());
        }
    }

    #[test]
    fn declared_empty_channel_never_accepts_raw_only_tx() {
        let f = Fixture::new("raw-only", "legacy_raw", "deadline");
        assert!(fs::read_to_string(&f.active.log_path)
            .unwrap()
            .contains(streamer_flow_evidence::MARKER));
        assert!(
            fs::read_to_string(f.active.log_path.with_extension("records.jsonl"))
                .unwrap()
                .is_empty()
        );
        println!("BRIDGE_PROOF_DECLARED_EMPTY_RAW_ONLY_TX_NEGATIVE_CONTROL");
        assert!(
            f.proof().is_err(),
            "declared evidence mode accepted raw-only structured proof"
        );
    }

    #[test]
    fn missing_malformed_and_foreign_declared_capture_are_rejected() {
        for mutation in [
            "missing_records",
            "malformed_records",
            "missing_observation_fields",
            "non_tx_source",
            "truncated_records",
            "missing_launch",
            "foreign_both_bindings",
        ] {
            let f = Fixture::new(mutation, "tx", "deadline");
            let records = f.active.log_path.with_extension("records.jsonl");
            let launch = f.active.log_path.with_extension("launch.json");
            match mutation {
                "missing_records" => fs::remove_file(records).unwrap(),
                "malformed_records" => {
                    fs::write(records, "FLOW_MESSAGE_EVIDENCE {broken}\n").unwrap()
                }
                "missing_observation_fields" => {
                    fs::write(records, "FLOW_MESSAGE_EVIDENCE {}\n").unwrap()
                }
                "non_tx_source" => {
                    let text = fs::read_to_string(&records).unwrap();
                    let body = text
                        .lines()
                        .next()
                        .unwrap()
                        .strip_prefix(streamer_flow_evidence::MARKER)
                        .unwrap();
                    let mut value: serde_json::Value = serde_json::from_str(body).unwrap();
                    value["stage"] = json!("egress");
                    fs::write(
                        records,
                        format!("{}{}\n", streamer_flow_evidence::MARKER, value),
                    )
                    .unwrap();
                }
                "truncated_records" => {
                    let mut bytes = fs::read(&records).unwrap();
                    assert_eq!(bytes.pop(), Some(b'\n'));
                    fs::write(records, bytes).unwrap();
                }
                "missing_launch" => fs::remove_file(launch).unwrap(),
                "foreign_both_bindings" => {
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
                _ => unreachable!(),
            }
            assert!(f.proof().is_err());
        }
    }

    #[test]
    fn human_payload_observation_remains_rejected() {
        let f = Fixture::new("human-rx", "tx", "deadline");
        let mut raw = OpenOptions::new()
            .append(true)
            .open(&f.passive.log_path)
            .unwrap();
        writeln!(raw, "FLOW observed_payload_bytes=3").unwrap();
        raw.sync_all().unwrap();
        assert!(f.proof().is_err());
    }
}
