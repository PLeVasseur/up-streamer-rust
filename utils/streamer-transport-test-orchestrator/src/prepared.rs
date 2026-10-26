// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Existing deployment preparation shared by finite and sustained workload execution.
//!
//! This is the current Linux execution profile's fixture boundary. It selects
//! configured artifacts and existing binding inputs, not middleware implementation
//! or cleanup. Scheduling and workload accounting consume the prepared result.
use super::{
    lola_application_id, lola_run_namespace, role_physical_env, row_env, row_run_id,
    write_lola_manifest, Cli, LolaManifestPaths, MatrixRow, NativeProfilePaths, RunBundle,
    WireEncoding, ICEORYX2_ROOT_PATH,
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// Explicit deployment fixture budget, delegated to the binding's existing pool
// configuration. Workload orchestration does not implement allocation behavior.
const WORKLOAD_ZENOH_POOL_BYTES: usize = 16 * 1024 * 1024;

/// Reserves the actual fixture directions before bridge admission. The source
/// island receives requests/publications; the sink island transmits them. RPC
/// additionally reserves the reversed response direction on those same cores.
pub(super) fn endpoint_catalog(
    role: super::RoleStyle,
    side: &str,
) -> Vec<configurable_streamer_wire_support::prepared::PreparedAddress> {
    use super::{
        RoleStyle, AUTHORITY_A, AUTHORITY_B, METHOD_RESOURCE_ID, TOPIC_RESOURCE_ID, UE_ID,
        UE_VERSION_MAJOR,
    };
    use configurable_streamer_wire_support::prepared::PreparedAddress;
    let to_sink = side == "sink";
    let uri = |authority, resource| {
        up_rust::UUri::try_from_parts(authority, UE_ID, UE_VERSION_MAJOR, resource)
            .expect("constant fixture URI is valid")
            .to_uri(false)
    };
    let entry = |source, sink, transmit| PreparedAddress {
        source,
        sink,
        max_metadata_len: 3072,
        max_payload_len: 8192,
        payload_alignment: 8,
        max_token_len: 2048,
        allow_traceparent: true,
        transmit,
    };
    match role {
        RoleStyle::PublisherSubscriber => {
            vec![entry(uri(AUTHORITY_A, TOPIC_RESOURCE_ID), None, to_sink)]
        }
        RoleStyle::NotifierNotifyee => vec![entry(
            uri(AUTHORITY_A, TOPIC_RESOURCE_ID),
            Some(uri(AUTHORITY_B, 0)),
            to_sink,
        )],
        RoleStyle::ClientServerRpc => vec![
            entry(
                uri(AUTHORITY_A, 0),
                Some(uri(AUTHORITY_B, METHOD_RESOURCE_ID)),
                to_sink,
            ),
            entry(
                uri(AUTHORITY_B, METHOD_RESOURCE_ID),
                Some(uri(AUTHORITY_A, 0)),
                !to_sink,
            ),
        ],
    }
}

pub(super) fn configure_workload_endpoints(path: &Path) -> Result<()> {
    let mut config: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    if let Some(endpoints) = config
        .get_mut("transports")
        .and_then(|v| v.get_mut("iceoryx2"))
        .and_then(|v| v.get_mut("endpoints"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for endpoint in endpoints {
            endpoint["iceoryx2_namespace"]["subscriber_buffer_size"] = serde_json::json!(32);
            endpoint["iceoryx2_namespace"]["max_borrowed_samples"] = serde_json::json!(32);
        }
    }
    if let Some(endpoints) = config
        .get_mut("transports")
        .and_then(|v| v.get_mut("zenoh"))
        .and_then(|v| v.get_mut("endpoints"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for endpoint in endpoints {
            if endpoint["routing_mode"] == "copy_minimized" {
                endpoint["zenoh_shm_segment_size"] = serde_json::json!(WORKLOAD_ZENOH_POOL_BYTES);
                endpoint["zenoh_wait_for_subscriber"] = serde_json::json!(true);
            }
        }
    }
    super::atomic_write_json(path, &config)
}

pub(super) fn workload_role_configuration(profile: super::EndpointProfile) -> Vec<String> {
    if profile.physical == super::PhysicalTransport::Zenoh
        && profile.kind != super::EndpointKind::Classic
    {
        vec![
            "--zenoh-shm-segment-size".into(),
            WORKLOAD_ZENOH_POOL_BYTES.to_string(),
            "--zenoh-wait-for-subscriber".into(),
        ]
    } else if profile.physical == super::PhysicalTransport::Iceoryx2 {
        vec![
            "--iceoryx2-receive-capacity".into(),
            "32".into(),
            "--iceoryx2-min-subscribers".into(),
            "1".into(),
        ]
    } else {
        Vec::new()
    }
}

pub(super) struct PreparedRow {
    pub row_dir: PathBuf,
    pub lola_run_namespace: String,
    pub lola_manifest_paths: Option<LolaManifestPaths>,
    pub lola_manifest_path: Option<PathBuf>,
    pub process_env: Vec<(String, String)>,
    pub active_env: Vec<(String, String)>,
    pub passive_env: Vec<(String, String)>,
    pub native_library_paths: BTreeMap<String, String>,
}

pub(super) fn prepare_row(
    artifacts_root: &Path,
    bundle: &RunBundle,
    row: &MatrixRow,
    cli: &Cli,
    iteration: usize,
    attempt: usize,
) -> Result<PreparedRow> {
    let run_id = row_run_id(row, cli.iterations, iteration);
    let row_dir = if attempt == 0 {
        artifacts_root.join(&run_id)
    } else {
        artifacts_root.join(format!("{run_id}-retry{attempt}"))
    };
    fs::create_dir_all(&row_dir)
        .with_context(|| format!("unable to create {}", row_dir.display()))?;
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
    let lola_bridge_lib = row
        .uses_lola()
        .then(|| bundle.native_library("libup_lola_bridge.so"))
        .transpose()?;
    let vsomeip_lib = row
        .uses_vsomeip()
        .then(|| bundle.native_library("libvsomeip3.so.3"))
        .transpose()?;
    let vsomeip_cfg_lib = row
        .uses_vsomeip()
        .then(|| bundle.native_library("libvsomeip3-cfg.so.3"))
        .transpose()?;
    let vsomeip_sd_lib = row
        .uses_vsomeip()
        .then(|| bundle.native_library("libvsomeip3-sd.so.3"))
        .transpose()?;
    let iceoryx2_root = row
        .uses_iceoryx2()
        .then(|| PathBuf::from(ICEORYX2_ROOT_PATH));
    let mut process_env = row_env(
        row,
        iceoryx2_root.as_deref(),
        bundle,
        cli.tokio_worker_threads,
    );
    if cli.cloud_events {
        process_env.push((
            "UPROTOCOL_CLOUDEVENTS_PROOF".into(),
            "protobuf-roundtrip-v1".into(),
        ));
    }
    let native_profiles = if row.encoding == WireEncoding::Native {
        let paths = NativeProfilePaths::new(&row_dir);
        paths.write(&bundle.manifest.native_profile)?;
        Some(paths)
    } else {
        None
    };
    let active_base_env = role_physical_env(row, true, &process_env);
    let passive_base_env = role_physical_env(row, false, &process_env);
    let active_env = native_profiles.as_ref().map_or_else(
        || active_base_env.clone(),
        |paths| paths.role_env(true, &active_base_env),
    );
    let passive_env = native_profiles.as_ref().map_or_else(
        || passive_base_env.clone(),
        |paths| paths.role_env(false, &passive_base_env),
    );
    let mut native_library_paths = BTreeMap::new();
    for (name, path) in [
        ("lola", lola_bridge_lib),
        ("vsomeip", vsomeip_lib),
        ("vsomeip_cfg", vsomeip_cfg_lib),
        ("vsomeip_sd", vsomeip_sd_lib),
    ] {
        if let Some(path) = path {
            native_library_paths.insert(name.to_string(), path.display().to_string());
        }
    }
    Ok(PreparedRow {
        row_dir,
        lola_run_namespace,
        lola_manifest_paths,
        lola_manifest_path,
        process_env,
        active_env,
        passive_env,
        native_library_paths,
    })
}
