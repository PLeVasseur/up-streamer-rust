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
use chrono::Utc;
use clap::Parser;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const AUTHORITY_A: &str = "authority-a";
const AUTHORITY_B: &str = "authority-b";
const UE_ID: u32 = 0x5BA0;
const UE_VERSION_MAJOR: u8 = 1;
const TOPIC_RESOURCE_ID: u16 = 0x8001;
const METHOD_RESOURCE_ID: u16 = 0x1000;
const ZENOH_ENDPOINT: &str = "tcp/127.0.0.1:7447";
const READY_STREAMER: &str = "READY streamer_initialized";
const READY_LISTENER: &str = "READY listener_registered";

#[derive(Debug, Parser)]
#[command(name = "streamer-transport-test-orchestrator")]
#[command(
    about = "Endpoint-profile matrix orchestrator for configurable-streamer plus role binaries"
)]
struct Cli {
    #[arg(long)]
    list: bool,

    #[arg(long = "only")]
    only: Vec<String>,

    #[arg(long)]
    skip_build: bool,

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PhysicalTransport {
    Zenoh,
    Iceoryx2,
    Lola,
    Mqtt5,
    Vsomeip,
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

#[derive(Debug)]
struct RunningProcess {
    name: String,
    log_path: PathBuf,
    child: Child,
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
    artifacts_root: String,
    rows: Vec<RowResult>,
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
    classification: RowClassification,
    reason: String,
    artifact_dir: Option<String>,
    config_path: Option<String>,
    lola_manifest_path: Option<String>,
    logs: BTreeMap<String, String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo_root = repo_root()?;
    let rows = matrix_rows();

    if cli.list {
        for row in &rows {
            let support = support_status(row);
            println!(
                "{}\t{:?}\t{} -> {}\t{:?}",
                row.id, support.classification, row.source.id, row.sink.id, support.reason
            );
        }
        return Ok(());
    }

    let selected_rows = select_rows(rows, &cli.only)?;
    let artifacts_root = cli.artifacts_root.clone().unwrap_or_else(|| {
        repo_root
            .join("target")
            .join("streamer-transport-test")
            .join(Utc::now().format("%Y%m%dT%H%M%SZ").to_string())
    });
    fs::create_dir_all(&artifacts_root)
        .with_context(|| format!("unable to create {}", artifacts_root.display()))?;

    if !cli.skip_build {
        build_required_binaries(&repo_root)?;
    }

    let mut results = Vec::new();
    let mut runnable_seen = 0_usize;
    for row in selected_rows {
        let support = support_status(&row);
        if support.classification != RowClassification::Pass {
            results.push(row_result(
                &row,
                support.classification,
                support.reason,
                None,
                None,
                None,
                BTreeMap::new(),
            ));
            continue;
        }

        runnable_seen += 1;
        if let Some(max) = cli.max_runnable_rows {
            if runnable_seen > max {
                results.push(row_result(
                    &row,
                    RowClassification::Blocked,
                    format!("not executed because --max-runnable-rows={max} was reached"),
                    None,
                    None,
                    None,
                    BTreeMap::new(),
                ));
                continue;
            }
        }

        println!("RUNNING {}", row.id);
        results.push(run_row(&repo_root, &artifacts_root, &row, &cli)?);
    }

    let summary = MatrixSummary {
        schema_version: "1.0",
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
        artifacts_root: artifacts_root.display().to_string(),
        rows: results,
    };
    let summary_path = artifacts_root.join("matrix-summary.json");
    fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)
        .with_context(|| format!("unable to write {}", summary_path.display()))?;

    println!(
        "STREAMER_TRANSPORT_TEST_SUMMARY_JSON={}",
        summary_path.display()
    );
    println!(
        "STREAMER_TRANSPORT_TEST_COUNTS pass={} unsupported={} blocked={} failed={}",
        summary.pass_count, summary.unsupported_count, summary.blocked_count, summary.failed_count
    );

    if summary.failed_count > 0 || summary.blocked_count > 0 {
        std::process::exit(1);
    }
    Ok(())
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
    if row.source.physical == PhysicalTransport::Vsomeip
        || row.sink.physical == PhysicalTransport::Vsomeip
    {
        return unsupported("configurable-streamer has no vSomeIP endpoint in its config schema");
    }
    if row.source.kind == EndpointKind::Classic || row.sink.kind == EndpointKind::Classic {
        if row.encoding != WireEncoding::Protobuf {
            return unsupported("classic UTransport examples in this repo carry protobuf UMessages, not native selected-wire or XCDRv2 frames");
        }
        if row.role == RoleStyle::NotifierNotifyee {
            return unsupported("classic MQTT5/Zenoh/vSomeIP example surface has publisher/subscriber and client/server roles, but no notifier/notifyee binaries");
        }
        return unsupported("classic endpoint rows are visible but are not executed by this selected-wire matrix harness yet; keep using the existing classic examples/smoke coverage until a classic matrix runner is added here");
    }
    if row.source.kind != row.sink.kind {
        return blocked("mixed selected-wire route families are blocked by current configurable-streamer route wiring: owned-frame endpoints can route to owned-frame endpoints and copy-minimized endpoints can route to copy-minimized endpoints, but owned-frame <-> copy-minimized conversion routes are not implemented");
    }
    if row.role == RoleStyle::ClientServerRpc
        && row.source.physical == PhysicalTransport::Zenoh
        && row.sink.physical == PhysicalTransport::Lola
    {
        return blocked("Zenoh selected-wire client -> LoLa selected-wire server RPC is blocked by the current response return path: the LoLa server observes the request payload, but the response does not return to the Zenoh client through configurable-streamer");
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

fn blocked(reason: &str) -> SupportStatus {
    SupportStatus {
        classification: RowClassification::Blocked,
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

fn run_row(
    repo_root: &Path,
    artifacts_root: &Path,
    row: &MatrixRow,
    cli: &Cli,
) -> Result<RowResult> {
    let row_dir = artifacts_root.join(&row.id);
    fs::create_dir_all(&row_dir)
        .with_context(|| format!("unable to create {}", row_dir.display()))?;
    let mut logs = BTreeMap::new();

    let lola_manifest_path = if row.uses_lola() {
        let path = row_dir.join("mw_com_config_lola.json");
        write_lola_manifest(row, &path)?;
        Some(path)
    } else {
        None
    };
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

    let config_path = row_dir.join("configurable-streamer.json");
    write_config(repo_root, row, &config_path, lola_manifest_path.as_deref())?;
    let zenoh_client_config_path = row_dir.join("zenoh-client.json5");
    write_zenoh_client_config(&zenoh_client_config_path)?;

    let mut streamer = spawn_process(
        "streamer",
        &target_debug_binary(repo_root, "configurable-streamer"),
        &["--config".to_string(), config_path.display().to_string()],
        &repo_root.join("configurable-streamer"),
        &row_env(row, lola_bridge_lib_dir.as_deref()),
        &row_dir,
    )?;
    logs.insert(
        "streamer".to_string(),
        streamer.log_path.display().to_string(),
    );

    let started = Instant::now();
    let result = (|| -> Result<()> {
        wait_for_marker(&streamer.log_path, READY_STREAMER, Duration::from_secs(10))?;

        let passive_spec = role_command(
            row,
            false,
            &zenoh_client_config_path,
            lola_manifest_path.as_deref(),
            cli,
        )?;
        let mut passive = spawn_process(
            "passive",
            &target_debug_binary(repo_root, &passive_spec.binary),
            &passive_spec.args,
            repo_root,
            &row_env(row, lola_bridge_lib_dir.as_deref()),
            &row_dir,
        )?;
        logs.insert(
            "passive".to_string(),
            passive.log_path.display().to_string(),
        );
        wait_for_marker(&passive.log_path, READY_LISTENER, Duration::from_secs(10))?;

        let active_spec = role_command(
            row,
            true,
            &zenoh_client_config_path,
            lola_manifest_path.as_deref(),
            cli,
        )?;
        let mut active = spawn_process(
            "active",
            &target_debug_binary(repo_root, &active_spec.binary),
            &active_spec.args,
            repo_root,
            &row_env(row, lola_bridge_lib_dir.as_deref()),
            &row_dir,
        )?;
        logs.insert("active".to_string(), active.log_path.display().to_string());

        wait_for_exit(
            &mut active,
            remaining_timeout(started, cli.scenario_timeout_secs, "active")?,
        )?;
        assert_success(&mut active)?;
        wait_for_exit(
            &mut passive,
            remaining_timeout(started, cli.scenario_timeout_secs, "passive")?,
        )?;
        assert_success(&mut passive)?;

        validate_flow_logs(row, &active.log_path, &passive.log_path)?;
        terminate(&mut passive);
        terminate(&mut active);
        Ok(())
    })();

    terminate(&mut streamer);

    match result {
        Ok(()) => Ok(row_result(
            row,
            RowClassification::Pass,
            "payload proof completed through configurable-streamer and stand-alone role binaries"
                .to_string(),
            Some(row_dir),
            Some(config_path),
            lola_manifest_path,
            logs,
        )),
        Err(error) => Ok(row_result(
            row,
            RowClassification::Failed,
            error.to_string(),
            Some(row_dir),
            Some(config_path),
            lola_manifest_path,
            logs,
        )),
    }
}

fn build_required_binaries(repo_root: &Path) -> Result<()> {
    let common_args = cargo_patch_args(repo_root);
    run_cargo(
        repo_root,
        [
            "build",
            "-p",
            "configurable-streamer",
            "--features",
            "experimental-copy-minimized-routing,zenoh-zero-copy,iceoryx2-zero-copy,lola-transport,zenoh-owned-frame,iceoryx2-owned-frame,lola-owned-frame",
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
            "zenoh-transport,iceoryx2-selected-wire,lola-selected-wire",
            "--no-default-features",
        ],
        &common_args,
    )?;
    Ok(())
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
    lola_manifest_path: Option<&Path>,
) -> Result<()> {
    let zenoh_config = repo_root.join("configurable-streamer/ZENOH_CONFIG.json5");
    let mqtt_config = repo_root.join("configurable-streamer/MQTT_CONFIG.json5");
    let subscription_data = repo_root.join("configurable-streamer/subscription_data.json");
    let source_endpoint = endpoint_name(row.source, "source");
    let sink_endpoint = endpoint_name(row.sink, "sink");
    let source_lola = lola_info(row, AUTHORITY_A, "source");
    let sink_lola = lola_info(row, AUTHORITY_B, "sink");

    let mut zenoh_endpoints = Vec::new();
    let mut iceoryx2_endpoints = Vec::new();
    let mut lola_endpoints = Vec::new();

    push_endpoint(
        &mut zenoh_endpoints,
        &mut iceoryx2_endpoints,
        &mut lola_endpoints,
        row.source,
        AUTHORITY_A,
        &source_endpoint,
        &sink_endpoint,
        row.encoding,
        source_lola.as_ref(),
        lola_manifest_path,
        row.role,
    );
    push_endpoint(
        &mut zenoh_endpoints,
        &mut iceoryx2_endpoints,
        &mut lola_endpoints,
        row.sink,
        AUTHORITY_B,
        &sink_endpoint,
        &source_endpoint,
        row.encoding,
        sink_lola.as_ref(),
        lola_manifest_path,
        row.role,
    );

    let config = json!({
        "up_streamer_config": { "message_queue_size": 32 },
        "streamer_uuri": { "authority": "authority-streamer", "ue_id": 78, "ue_version_major": 1 },
        "usubscription_config": { "mode": "static_file", "file_path": subscription_data },
        "transports": {
            "zenoh": { "config_file": zenoh_config, "endpoints": zenoh_endpoints },
            "mqtt": { "config_file": mqtt_config, "endpoints": [] },
            "iceoryx2": { "endpoints": iceoryx2_endpoints },
            "lola": { "endpoints": lola_endpoints },
        }
    });
    fs::write(config_path, serde_json::to_string_pretty(&config)?)
        .with_context(|| format!("unable to write {}", config_path.display()))
}

fn push_endpoint(
    zenoh_endpoints: &mut Vec<serde_json::Value>,
    iceoryx2_endpoints: &mut Vec<serde_json::Value>,
    lola_endpoints: &mut Vec<serde_json::Value>,
    profile: EndpointProfile,
    authority: &str,
    endpoint: &str,
    forward_endpoint: &str,
    encoding: WireEncoding,
    lola: Option<&LolaEndpointInfo>,
    lola_manifest_path: Option<&Path>,
    role: RoleStyle,
) {
    let mut value = json!({
        "authority": authority,
        "endpoint": endpoint,
        "routing_mode": profile.kind.routing_mode(),
        "forwarding_routes": [{ "endpoint": forward_endpoint, "wire_format": encoding.wire_format() }],
    });
    if profile.kind == EndpointKind::CopyMinimized {
        value["copy_minimized_payload_alignment"] = json!(8);
    }
    if let Some(lola) = lola {
        value["lola_instance_specifier"] = json!(lola.instance_specifier);
        value["lola_service_type"] = json!(lola.service_type);
        value["lola_event_name"] = json!(lola.event_name);
        value["lola_sample_size"] = json!(65536);
        value["lola_sample_alignment"] = json!(8);
        value["lola_max_samples"] = json!(16);
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
            value["lola_default_rx_channel"] = json!("both");
        }
    }
    match profile.physical {
        PhysicalTransport::Zenoh => zenoh_endpoints.push(value),
        PhysicalTransport::Iceoryx2 => iceoryx2_endpoints.push(value),
        PhysicalTransport::Lola => lola_endpoints.push(value),
        PhysicalTransport::Mqtt5 | PhysicalTransport::Vsomeip => {}
    }
}

fn write_lola_manifest(row: &MatrixRow, path: &Path) -> Result<()> {
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
        let info = lola_info(row, authority, side).expect("LoLa profile has LoLa info");
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
            "queue-size": { "QM-receiver": 16, "QM-sender": 16 },
            "shm-size-calc-mode": "SIMULATION"
        }
    });
    fs::write(path, serde_json::to_string_pretty(&manifest)?)
        .with_context(|| format!("unable to write {}", path.display()))
}

fn write_zenoh_client_config(path: &Path) -> Result<()> {
    let config = r#"{
  mode: "client",
  connect: {
    endpoints: ["tcp/127.0.0.1:7447"],
  },
  scouting: {
    multicast: {
      enabled: false,
    },
    gossip: {
      enabled: false,
    },
  },
}
"#;
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
                "numberOfSampleSlots": 16,
                "maxSubscribers": 8,
                "numberOfIpcTracingSlots": 0
            }]
        }]
    }));
    *service_id += 1;
}

fn lola_service_id_base(row: &MatrixRow) -> u32 {
    10_000 + (row.ordinal as u32 * 8)
}

struct RoleCommand {
    binary: String,
    args: Vec<String>,
}

fn role_command(
    row: &MatrixRow,
    active: bool,
    zenoh_client_config: &Path,
    lola_manifest_path: Option<&Path>,
    cli: &Cli,
) -> Result<RoleCommand> {
    let profile = if active { row.source } else { row.sink };
    let local_authority = if active { AUTHORITY_A } else { AUTHORITY_B };
    let peer_authority = if active { AUTHORITY_B } else { AUTHORITY_A };
    let role_name = role_binary_suffix(row.role, active);
    let binary = binary_name(profile, role_name);
    let mut args = if profile.physical == PhysicalTransport::Zenoh {
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
        )?;
    }
    Ok(RoleCommand { binary, args })
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
        TOPIC_RESOURCE_ID.to_string(),
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
) -> Result<()> {
    let info = lola_info(row, authority, side).ok_or_else(|| anyhow!("missing LoLa info"))?;
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
        "16".to_string(),
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

fn role_send_count(row: &MatrixRow, cli: &Cli) -> usize {
    if row.role == RoleStyle::ClientServerRpc && row.sink.physical == PhysicalTransport::Lola {
        cli.send_count.max(1)
    } else {
        cli.send_count.max(5)
    }
}

fn binary_name(profile: EndpointProfile, role_name: &str) -> String {
    match profile.physical {
        PhysicalTransport::Zenoh => format!("zenoh_{role_name}"),
        PhysicalTransport::Iceoryx2 => format!("iceoryx2_{role_name}"),
        PhysicalTransport::Lola => format!("lola_{role_name}"),
        PhysicalTransport::Mqtt5 => format!("mqtt_{role_name}"),
        PhysicalTransport::Vsomeip => format!("someip_{role_name}"),
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

fn spawn_process(
    name: &str,
    executable: &Path,
    args: &[String],
    workdir: &Path,
    env: &[(String, String)],
    artifact_dir: &Path,
) -> Result<RunningProcess> {
    let log_path = artifact_dir.join(format!("{name}.log"));
    let stdout = log_file(&log_path)?;
    let stderr = stdout.try_clone()?;
    let mut command = Command::new(executable);
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

fn log_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("unable to open {}", path.display()))
}

fn wait_for_marker(path: &Path, marker: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let contents = fs::read_to_string(path).unwrap_or_default();
        if contents.contains(marker) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for marker {marker} in {}",
                path.display()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_exit(process: &mut RunningProcess, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
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
        classification,
        reason,
        artifact_dir: artifact_dir.map(|path| path.display().to_string()),
        config_path: config_path.map(|path| path.display().to_string()),
        lola_manifest_path: lola_manifest_path.map(|path| path.display().to_string()),
        logs,
    }
}

fn row_env(_row: &MatrixRow, lola_bridge_lib_dir: Option<&Path>) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "RUST_LOG".to_string(),
            "info,configurable_streamer=debug,up_streamer=debug,example_streamer_uses=debug,up_transport_zenoh=debug,up_transport_iceoryx2_rust=debug,up_transport_lola_rust=debug".to_string(),
        ),
        ("CARGO_NET_GIT_FETCH_WITH_CLI".to_string(), "true".to_string()),
    ];
    if let Some(lib_dir) = lola_bridge_lib_dir {
        let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let value = if existing.is_empty() {
            lib_dir.display().to_string()
        } else {
            format!("{}:{existing}", lib_dir.display())
        };
        env.push(("LD_LIBRARY_PATH".to_string(), value));
    }
    env
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

fn lola_info(row: &MatrixRow, authority: &str, side: &str) -> Option<LolaEndpointInfo> {
    let profile = if side == "source" {
        row.source
    } else {
        row.sink
    };
    if profile.physical != PhysicalTransport::Lola {
        return None;
    }
    let safe_row = sanitize(&row.id);
    let safe_authority = sanitize(authority);
    let safe_side = sanitize(side);
    Some(LolaEndpointInfo {
        instance_specifier: format!(
            "uprotocol/streamerTransportTest/{safe_row}/{safe_side}/primary"
        ),
        service_type: format!(
            "/uprotocol/StreamerTransportTest/{safe_row}/{safe_authority}/{safe_side}/Primary"
        ),
        event_name: format!("frame{safe_authority}{safe_side}Primary"),
        response_instance_specifier: Some(format!(
            "uprotocol/streamerTransportTest/{safe_row}/{safe_side}/response"
        )),
        response_service_type: Some(format!(
            "/uprotocol/StreamerTransportTest/{safe_row}/{safe_authority}/{safe_side}/Response"
        )),
        response_event_name: Some(format!("frame{safe_authority}{safe_side}Response")),
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
        }
    }
}

impl MatrixRow {
    fn uses_lola(&self) -> bool {
        self.source.physical == PhysicalTransport::Lola
            || self.sink.physical == PhysicalTransport::Lola
    }
}
