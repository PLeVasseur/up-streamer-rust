// SPDX-License-Identifier: Apache-2.0
//! Live negative controls on the exact role binaries and isolated native domains.
use super::*;

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
        let active_log = fs::read_to_string(&active.log_path)?;
        let passive_log = fs::read_to_string(&passive.log_path)?;
        let received = [active_log.as_str(), passive_log.as_str()]
            .iter()
            .any(|log| {
                log.lines()
                    .filter_map(|line| line.strip_prefix(streamer_flow_evidence::MARKER))
                    .filter_map(|json| {
                        serde_json::from_str::<streamer_flow_evidence::Observation>(json).ok()
                    })
                    .any(|record| record.stage == "rx")
            });
        if received || passive_log.contains("FLOW observed_payload_bytes") {
            return Err(anyhow!(
                "bridge-off control delivered a message without Streamer for {}",
                row.id
            ));
        }
        if !active_log.contains(streamer_flow_evidence::MARKER)
            || !passive_log.contains("DeadlineExceeded")
            || active_log.contains("FLOW_EVIDENCE_ERROR")
            || passive_log.contains("FLOW_EVIDENCE_ERROR")
        {
            return Err(anyhow!(
                "bridge-off control did not establish a valid source and listening sink for {}",
                row.id
            ));
        }
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
