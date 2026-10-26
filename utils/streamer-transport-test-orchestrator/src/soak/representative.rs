// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Fixed representative cohort and deployment inputs over the maintained worker engine.
use super::*;

pub(super) fn build_workers(
    repo: &Path,
    target: &Path,
    rows: &[MatrixRow],
    root: &Path,
) -> Result<()> {
    let features = format!(
        "{},sustained-workload,payload-contract-large-fixtures",
        example_streamer_features(rows).join(",")
    );
    for (package, features) in [
        ("example-streamer-uses", features.as_str()),
        (
            "streamer-native-inventory",
            "payload-contract-large-fixtures",
        ),
    ] {
        let mut command = Command::new("cargo");
        command.args([
            "build",
            "--locked",
            "--offline",
            "--profile",
            MATRIX_CARGO_PROFILE,
            "-p",
            package,
            "--no-default-features",
            "--features",
            features,
        ]);
        command.current_dir(repo).env(
            "CARGO_TARGET_DIR",
            target.parent().context("matrix target parent")?,
        );
        configure_matrix_cargo_environment(&mut command);
        command.env("CARGO_PROFILE_MATRIX_OPT_LEVEL", "3");
        let output = command.output()?;
        fs::write(
            root.join(format!("{package}-sensor-build.stdout")),
            &output.stdout,
        )?;
        fs::write(
            root.join(format!("{package}-sensor-build.stderr")),
            &output.stderr,
        )?;
        anyhow::ensure!(
            output.status.success(),
            "sensor worker build failed; inspect {}",
            root.display()
        );
    }
    Ok(())
}

pub(super) fn rows(mode: Mode) -> Result<Vec<MatrixRow>> {
    anyhow::ensure!(
        cfg!(feature = "payload-contract-large-fixtures"),
        "representative lane requires payload-contract-large-fixtures"
    );
    let base = matrix_rows()
        .into_iter()
        .find(|r| {
            r.source.id == "iceoryx2-copy-minimized"
                && r.sink.id == "zenoh-copy-minimized"
                && r.role == RoleStyle::PublisherSubscriber
                && r.encoding == WireEncoding::Native
        })
        .context("missing native sensor route")?;
    let count = if mode == Mode::Diagnostic { 1 } else { 4 };
    let mut rows = Vec::new();
    for fixture in ["lidar", "camera"] {
        for instance in 1..=count {
            let mut row = base.clone();
            row.id = format!("sensor-{fixture}-{instance}");
            row.ordinal = rows.len();
            rows.push(row);
        }
    }
    Ok(rows)
}

pub(super) fn set_limits(plan: &mut Plan) {
    plan.require_pool_page_census = true;
    let diagnostic = plan.mode == Mode::Diagnostic;
    plan.warmup_seconds = if diagnostic { 10 } else { 120 };
    plan.normal_seconds = if diagnostic { 20 } else { 180 };
    plan.slow_seconds = if diagnostic { 20 } else { 180 };
    plan.drain_seconds = if diagnostic { 10 } else { 30 };
    plan.maximum_seconds = if diagnostic { 180 } else { 600 };
    plan.offered_per_second = 2;
    plan.consumer_delay_ms = 1000;
    plan.maximum_payload_bytes = 12_441_784;
}

pub(super) fn validate_plan(plan: &Plan) -> Result<()> {
    let diagnostic = plan.mode == Mode::Diagnostic;
    anyhow::ensure!(
        plan.schema == "uprotocol.sustained-workload.v1"
            && plan.lane == Lane::Representative
            && (
                plan.warmup_seconds,
                plan.normal_seconds,
                plan.slow_seconds,
                plan.drain_seconds,
                plan.maximum_seconds
            ) == if diagnostic {
                (10, 20, 20, 10, 180)
            } else {
                (120, 180, 180, 30, 600)
            }
            && plan.offered_per_second == 2
            && plan.consumer_delay_ms == 1000
            && plan.queue_capacity == 4
            && plan.maximum_payload_bytes == 12_441_784
            && plan.pending_limit == 4096
            && !plan.require_fifo
            && plan.rss_max == 6 * 1024u64.pow(3)
            && plan.pss_max == 5 * 1024u64.pow(3)
            && plan.shared_pss_max == 1024u64.pow(3)
            && plan.growth_bytes_per_minute == 8 * 1024 * 1024
            && plan.fd_max == 32768
            && plan.thread_max == 4096
            && plan.allocator_arena_limit == 2
            && plan.diagnostic_log_level == "warn,up_streamer=info",
        "representative profile changed"
    );
    anyhow::ensure!(
        plan.rows
            == rows(plan.mode)?
                .iter()
                .map(|r| r.id.clone())
                .collect::<Vec<_>>(),
        "representative cohort changed"
    );
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub(super) struct Geometry {
    fixture: String,
    payload_bytes: usize,
    metadata_bytes: usize,
    alignment: usize,
    sample_slice_bytes: usize,
    publisher_slots: usize,
    zenoh_pool_bytes: usize,
    identity: NativeVerification,
}

#[cfg(feature = "payload-contract-large-fixtures")]
fn fixture(row: &MatrixRow) -> Result<configurable_streamer_wire_support::sensor::SensorFixture> {
    use configurable_streamer_wire_support::sensor::SensorFixture;
    if row.id.starts_with("sensor-lidar-") {
        Ok(SensorFixture::LidarHesaiAt128)
    } else if row.id.starts_with("sensor-camera-") {
        Ok(SensorFixture::CameraBayerRggb12p8mp)
    } else {
        Err(anyhow!("unknown sensor deployment"))
    }
}

#[cfg(feature = "payload-contract-large-fixtures")]
pub(super) fn expected_identity(
    row: &MatrixRow,
    document: &NativeProfileDocument,
) -> Result<NativeVerification> {
    let fixture = fixture(row)?;
    let representation =
        up_rust::bench_fixtures::payload_contract::stable_payload_representation(fixture.case())
            .map_err(|e| anyhow!("{e}"))?;
    let profile = document.resolve().map_err(|e| anyhow!("{e}"))?;
    let agreement = up_rust::NativeProfileAgreement::new(Arc::new(profile.clone()), &profile)
        .map_err(|e| anyhow!("{e}"))?;
    let identity = agreement
        .identity_for(&representation)
        .map_err(|e| anyhow!("{e}"))?;
    let type_name = if row.id.starts_with("sensor-lidar-") {
        "org.eclipse.uprotocol.bench.v1.LidarPointCloudHesaiAt128V1"
    } else {
        "org.eclipse.uprotocol.bench.v1.CameraBayerRggb12pFrame8mpV1"
    };
    Ok(NativeVerification {
        domain: profile.domain().into(),
        version: profile.version(),
        profile_digest: profile.content_digest(),
        type_name: type_name.into(),
        encoding_id: identity.encoding().id(),
        native_type_token: identity.token().as_u32(),
        identity_source: String::new(),
    })
}

#[cfg(not(feature = "payload-contract-large-fixtures"))]
pub(super) fn expected_identity(
    _: &MatrixRow,
    _: &NativeProfileDocument,
) -> Result<NativeVerification> {
    Err(anyhow!("sensor feature disabled"))
}

pub(super) fn require_inventory(document: &NativeProfileDocument) -> Result<()> {
    for row in rows(Mode::Diagnostic)? {
        expected_identity(&row, document)?;
    }
    Ok(())
}

#[cfg(feature = "payload-contract-large-fixtures")]
pub(super) fn configure_worker(path: &Path, row: &MatrixRow) -> Result<()> {
    let fixture = fixture(row)?;
    let mut value: serde_json::Value = read(path)?;
    value["native_fixture"] = serde_json::to_value(fixture)?;
    value["maximum_payload_bytes"] = json!(fixture.payload_len());
    atomic_write_json(path, &value)
}

#[cfg(not(feature = "payload-contract-large-fixtures"))]
pub(super) fn configure_worker(_: &Path, _: &MatrixRow) -> Result<()> {
    Err(anyhow!("sensor feature disabled"))
}

#[cfg(feature = "payload-contract-large-fixtures")]
pub(super) fn configure_endpoints(
    path: &Path,
    row: &MatrixRow,
    document: &NativeProfileDocument,
    directory: &Path,
) -> Result<Geometry> {
    use up_rust::{
        NativePrefixFrameMetadataCodec, StableContainerWireFormat, UWireMetadataCodec,
        UWireMetadataContext,
    };
    let sensor = fixture(row)?;
    let profile = document.resolve().map_err(|e| anyhow!("{e}"))?;
    let agreement = up_rust::NativeProfileAgreement::new(Arc::new(profile.clone()), &profile)
        .map_err(|e| anyhow!("{e}"))?;
    let expected = expected_identity(row, document)?;
    let message = up_rust::UMessageBuilder::publish(
        up_rust::UUri::try_from_parts(AUTHORITY_A, 0x5BA0, 1, 0x8001)
            .map_err(|e| anyhow!("{e}"))?,
    )
    .build_with_payload(
        Vec::<u8>::new(),
        up_rust::PayloadEncoding::from_id(expected.encoding_id).map_err(|e| anyhow!("{e}"))?,
    )
    .map_err(|e| anyhow!("{e}"))?;
    let metadata =
        up_rust::frame::metadata::try_project_umessage_to_frame_metadata_with_native_profile(
            &message, &agreement,
        )
        .map_err(|e| anyhow!("{e}"))?;
    let encoded = NativePrefixFrameMetadataCodec
        .encode_frame_metadata(
            UWireMetadataContext::from_wire::<
                StableContainerWireFormat,
                NativePrefixFrameMetadataCodec,
            >(),
            &metadata,
        )
        .map_err(|e| anyhow!("{e}"))?;
    let alignment = 8usize;
    let payload_bytes = sensor.payload_len();
    let sample_slice_bytes = encoded
        .len()
        .checked_add(alignment - 1)
        .and_then(|n| n.checked_add(payload_bytes))
        .context("sensor sample overflow")?;
    let geometry = Geometry {
        fixture: serde_json::to_value(sensor)?
            .as_str()
            .context("fixture name")?
            .into(),
        payload_bytes,
        metadata_bytes: encoded.len(),
        alignment,
        sample_slice_bytes,
        publisher_slots: 9,
        zenoh_pool_bytes: if row.id.starts_with("sensor-lidar-") {
            48 * 1024 * 1024
        } else {
            96 * 1024 * 1024
        },
        identity: expected,
    };
    let mut config: serde_json::Value = read(path)?;
    let ice = config
        .pointer_mut("/transports/iceoryx2/endpoints")
        .and_then(|v| v.as_array_mut())
        .context("missing iceoryx2 endpoint")?;
    anyhow::ensure!(ice.len() == 1, "representative needs one iceoryx2 ingress");
    for endpoint in ice {
        let namespace = &mut endpoint["iceoryx2_namespace"];
        namespace["subscriber_buffer_size"] = json!(2);
        namespace["max_borrowed_samples"] = json!(2);
        namespace["publisher_initial_max_slice_len"] = json!(geometry.sample_slice_bytes);
        namespace["publisher_allocation_strategy"] = json!("static");
        namespace["max_subscribers"] = json!(2);
        namespace["publisher_max_loaned_samples"] = json!(1);
        namespace["publisher_prewarm"] = json!(true);
    }
    let zenoh = config
        .pointer_mut("/transports/zenoh/endpoints")
        .and_then(|v| v.as_array_mut())
        .context("missing Zenoh endpoint")?;
    anyhow::ensure!(zenoh.len() == 1, "representative needs one Zenoh provider");
    zenoh[0]["zenoh_shm_segment_size"] = json!(geometry.zenoh_pool_bytes);
    zenoh[0]["zenoh_shm_prewarm"] = json!(true);
    zenoh[0]["zenoh_wait_for_subscriber"] = json!(true);
    atomic_write_json(path, &config)?;
    atomic_write_json(&directory.join("sensor-geometry.json"), &geometry)?;
    Ok(geometry)
}

#[cfg(not(feature = "payload-contract-large-fixtures"))]
pub(super) fn configure_endpoints(
    _: &Path,
    _: &MatrixRow,
    _: &NativeProfileDocument,
    _: &Path,
) -> Result<Geometry> {
    Err(anyhow!("sensor feature disabled"))
}

pub(super) fn role_configuration(profile: EndpointProfile, geometry: &Geometry) -> Vec<String> {
    if profile.physical == PhysicalTransport::Iceoryx2 {
        [
            "--iceoryx2-receive-capacity".into(),
            "2".into(),
            "--iceoryx2-min-subscribers".into(),
            "2".into(),
            "--iceoryx2-publisher-initial-max-slice-len".into(),
            geometry.sample_slice_bytes.to_string(),
            "--iceoryx2-publisher-allocation-strategy".into(),
            "static".into(),
            "--iceoryx2-max-subscribers".into(),
            "2".into(),
            "--iceoryx2-publisher-max-loaned-samples".into(),
            "1".into(),
            "--iceoryx2-publisher-prewarm".into(),
        ]
        .into()
    } else {
        [
            "--zenoh-shm-segment-size".into(),
            geometry.zenoh_pool_bytes.to_string(),
            "--zenoh-wait-for-subscriber".into(),
        ]
        .into()
    }
}

pub(super) fn audit_worker_metrics(directory: &Path, report: &serde_json::Value) -> Result<()> {
    anyhow::ensure!(
        report["application_lease_peaks"]["source"][1] == 1,
        "representative TX bound differs"
    );
    let mut final_count = 0;
    for actor in ["active", "passive"] {
        for line in BufReader::new(File::open(directory.join(format!("{actor}.log")))?).lines() {
            let line = line?;
            let Some(body) = line.strip_prefix("OP_METRICS ") else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(body)?;
            for (field, limit) in [
                ("max_application_rx_loans", 5),
                ("max_transient_rx_loans", 1),
                ("max_application_tx_loans", 1),
            ] {
                anyhow::ensure!(
                    value[field].as_u64().is_some_and(|n| n <= limit),
                    "sensor {actor} {field} bound exceeded"
                );
            }
            if value["final"] == true {
                final_count += 1;
                for field in [
                    "live_application_rx_loans",
                    "live_transient_rx_loans",
                    "live_application_tx_loans",
                ] {
                    anyhow::ensure!(value[field] == 0, "sensor retained loan after shutdown");
                }
            }
        }
    }
    anyhow::ensure!(final_count == 2, "missing final sensor loan metrics");
    Ok(())
}

pub(super) fn audit_resources(
    root: &Path,
    plan: &Plan,
    phases: &[serde_json::Value],
) -> Result<serde_json::Value> {
    let mut groups = BTreeMap::new();
    let mut launchers = BTreeSet::new();
    let mut controller = None;
    for row in &plan.rows {
        let value: serde_json::Value = read(&root.join(row).join("owned-groups.json"))?;
        for actor in ["source", "sink", "bridge"] {
            let group = value[actor].as_i64().context("group id missing")? as i32;
            anyhow::ensure!(
                groups.insert(group, format!("{row}/{actor}")).is_none(),
                "group alias"
            );
        }
        launchers.insert(value["launcher"].as_i64().context("launcher id missing")? as i32);
        let pid = value["controller"]
            .as_u64()
            .context("controller pid missing")? as u32;
        if let Some(previous) = controller {
            anyhow::ensure!(previous == pid, "controller changed");
        }
        controller = Some(pid);
    }
    let mut identities = BTreeMap::new();
    let mut windows: BTreeMap<(String, String), Vec<(f64, u64)>> = BTreeMap::new();
    let mut separate = Vec::new();
    for line in BufReader::new(File::open(root.join("resources.jsonl"))?).lines() {
        let sample: Sample = serde_json::from_str(&line?)?;
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        let mut controller_pss = 0;
        let mut launcher_pss = 0;
        for process in &sample.processes {
            anyhow::ensure!(
                process.start_time_ticks > 0,
                "process start identity missing"
            );
            if let Some(old) = identities.insert(
                process.pid,
                (process.start_time_ticks, process.process_group),
            ) {
                anyhow::ensure!(
                    old == (process.start_time_ticks, process.process_group),
                    "PID identity changed"
                );
            }
            if let Some(group) = groups.get(&process.process_group) {
                *totals.entry(group.clone()).or_default() += process.pss;
            } else if Some(process.pid) == controller {
                controller_pss += process.pss;
            } else if launchers.contains(&process.process_group) {
                launcher_pss += process.pss;
            } else {
                return Err(anyhow!(
                    "unclassified process in representative tree: {}",
                    process.pid
                ));
            }
        }
        anyhow::ensure!(
            totals.len() == groups.len(),
            "missing endpoint group sample"
        );
        anyhow::ensure!(
            totals.values().sum::<u64>() + controller_pss + launcher_pss == sample.pss,
            "partial inclusive PSS accounting"
        );
        separate.push(json!({"elapsed":sample.elapsed,"controller_pss":controller_pss,"launcher_pss":launcher_pss,"endpoints_pss":totals.values().sum::<u64>(),"inclusive_pss":sample.pss}));
        for (index, phase) in [(1, "normal"), (2, "slow")] {
            if serde_json::to_value(sample.phase)? != phase {
                continue;
            }
            let seconds = if phase == "normal" {
                plan.normal_seconds
            } else {
                plan.slow_seconds
            };
            let end = phases[index]["elapsed_seconds"]
                .as_f64()
                .context("phase start")?
                + seconds as f64;
            if sample.elapsed >= end - 90.0 {
                for (group, bytes) in &totals {
                    windows
                        .entry((phase.into(), group.clone()))
                        .or_default()
                        .push((sample.elapsed, *bytes));
                }
            }
        }
    }
    let mut slopes = Vec::new();
    for ((phase, group), points) in windows {
        let duration = points.last().context("empty PSS window")?.0 - points[0].0;
        let mean_x = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
        let mean_y = points.iter().map(|p| p.1 as f64).sum::<f64>() / points.len() as f64;
        let denominator = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum::<f64>();
        anyhow::ensure!(denominator > 0.0, "insufficient PSS slope samples");
        let slope = points
            .iter()
            .map(|p| (p.0 - mean_x) * (p.1 as f64 - mean_y))
            .sum::<f64>()
            / denominator
            * 60.0;
        let limit = if phase == "normal" {
            1_048_576.0
        } else {
            2_097_152.0
        };
        if plan.mode != Mode::Diagnostic {
            anyhow::ensure!(
                duration >= 86.0 && points.len() >= 80 && slope <= limit,
                "{group}/{phase}: incomplete90s window or PSS slope {slope} exceeds {limit}"
            );
        }
        slopes.push(json!({"group":group,"phase":phase,"duration":duration,"samples":points.len(),"bytes_per_minute":slope,"limit":limit}));
    }
    anyhow::ensure!(
        slopes.len() == groups.len() * 2,
        "missing group slope window"
    );
    Ok(
        json!({"endpoint_group_slopes":slopes,"separate_accounting":separate,"diagnostic_only":plan.mode==Mode::Diagnostic}),
    )
}
