# Streamer Transport Test Orchestrator

`streamer-transport-test-orchestrator` executes the canonical endpoint-profile
matrix through `configurable-streamer` and independent active/passive role
processes. It preserves matrix order, records structural unsupported rows, and
runs supported rows in isolated user, mount, network, and IPC namespaces.

This is the exhaustive matrix authority, not the transport smoke suite. The
smoke suite provides short scenario-level integration checks. This orchestrator
covers every generated source/sink profile, role, and wire encoding combination,
applies matrix retry and acceptance criteria, and retains row-level evidence.

## Prerequisites

- Linux with `unshare`, `nsenter`, `mount`, `ip`, `kill`, `sha256sum`, and user
  namespaces.
- Rust 1.88 or newer for the workspace; Rust 1.95 is used for strict quality
  validation.
- `mosquitto` for MQTT rows.
- Native LoLa and vSomeIP libraries for rows using those transports. The build
  normally produces them; `LD_LIBRARY_PATH` may also supply them.
- Enough space and inodes below `target/`; a complete run retains all row logs,
  generated configs, checkpoints, the immutable run bundle, and the final
  summary.
- Only one orchestrator may build or stage from a target directory at a time.
  A target-directory advisory lock is held for the complete run, in addition to
  the exclusive artifact-root lock.

Run from the workspace root:

```bash
cargo run -p streamer-transport-test-orchestrator -- \
  --jobs 4 \
  --lola-jobs 1 \
  --criteria utils/streamer-transport-test-orchestrator/matrix-criteria.json
```

## Options

- `--list`: print generated rows and support classifications without running.
- `--generate-criteria <FILE>`: derive criteria from the canonical matrix.
- `--only <ROW_ID>`: select a row; repeat the option to select multiple rows in
  the specified order.
- `--skip-build`: stage and validate a fresh immutable run bundle from existing
  Cargo target artifacts. This is intended for focused diagnosis, not a
  complete acceptance run.
- `--use-local-sibling-patches`: add available sibling repositories as Cargo
  patch configuration during the build.
- `--artifacts-root <PATH>`: choose a new, empty evidence root. Relative paths
  resolve from the workspace root.
- `--send-count <N>`: requested sends, default `1`; role commands retain the
  established minimum of five.
- `--send-interval-ms <MS>`: active-role pacing, default `200`.
- `--timeout-ms <MS>`: role operation timeout, default `5000`; established
  transport-specific floors remain in effect.
- `--scenario-timeout-secs <S>`: row scenario timeout, default `30`.
- `--max-runnable-rows <N>`: execute only the first `N` supported rows and
  classify later supported rows as planner-blocked.
- `--jobs <N>`: global worker limit, default `4`.
- `--lola-jobs <N>`: LoLa-sensitive worker limit, default `1`.
- `--dds-jobs <N>`, `--zenoh-shm-jobs <N>`, `--vsomeip-jobs <N>`, and
  `--mqtt-jobs <N>`: optional transport-resource caps. Unset caps inherit the
  effective global worker limit. Rows acquire their complete applicable class
  set atomically in deterministic class order, so mixed-resource rows cannot
  deadlock.
- `--hard-max-jobs <N>`: configured host-policy ceiling, default `16`. Requests
  above it are rejected. The worker pool is further clamped to available CPUs
  and runnable tasks; the accepted defaults remain effectively `4/1` for
  global/LoLa work on a sufficiently capable host.
- `--tokio-worker-threads <N>`: forced Tokio worker count for matrix children,
  default `2`.
- `--preflight-memory-mib-per-job`, `--preflight-tasks-per-job`,
  `--preflight-fds-per-job`, `--preflight-disk-mib-per-row`, and
  `--preflight-inodes-per-row`: deterministic host-envelope assumptions. The
  defaults are 1024 MiB, 512 tasks, 1024 FDs, 1 MiB, and 32 inodes.
- `--iterations <N>`: repeat the selected matrix in canonical order, default
  `1`.
- `--disable-row-retries`: disable the configured transport retry policy.
- `--copy-minimized-sinks-only`: retain only copy-minimized sink profiles.
- `--criteria <FILE>`: validate final counts, unsupported reasons, and retry
  consumption against an accepted criteria file.

## Resource And Input Safety

Before row dispatch the orchestrator checks CPU policy, available RAM, free
swap reporting, process/cgroup task capacity, the FD soft limit, user namespace
availability with an executable probe, and filesystem byte/inode capacity.
`preflight.json` is written before dispatch. Filesystems that legitimately do
not report inode quotas are identified as unsupported rather than recorded as
zero; byte capacity is still enforced. Capacity comes directly from
`statvfs(3)`, not parsed `df` output.

Cargo's target directory is resolved from `CARGO_TARGET_DIR` or `cargo metadata`.
The repository, target, artifact, criteria, and native input paths are
canonicalized and rejected if a private matrix `/tmp` or `/dev/shm` mount would
shadow them. Matrix children receive an empty inherited environment plus a
small allowlist: deterministic locale/timezone and logging values, bundle-only
`PATH`/`LD_LIBRARY_PATH`, forced `TMPDIR=/tmp`, transport-specific values, and
the configured `TOKIO_WORKER_THREADS`.

After the single build, every executable used by row supervision and every
selected LoLa/vSomeIP native library is staged once under
`run-bundle/{bin,lib,objects}`. Content-addressed objects use hard links where
possible and copy fallback otherwise; no binary is copied per row. Launch names
are read-only hard links/copies, and `run-bundle/manifest.json` records source
and bundle paths, kind, size, mode, transfer method, and SHA-256. The complete
bundle is hash/permission validated before preflight, and rows launch project
and supervision executables only from the bundle. Replacing a mutable target
path after staging cannot redirect an active run.

Every child is a process-group leader. Teardown and cancellation signal the
whole group with bounded SIGINT, SIGTERM, and SIGKILL escalation, reap the
leader, verify that no group descendants remain, and expose cleanup/leak counts
in the summary. Both SIGINT and SIGTERM set the same cooperative cancellation
flag.

## Instrumentation

Summary schema `3.0` is written to `matrix-summary.json`. Checkpoints use a
versioned `1.0` envelope containing `schema_version` and `row`. The final
summary, preflight, criteria result, manifest, and all checkpoints use
same-directory temporary files, file fsync, atomic rename, and parent-directory
fsync. A checkpoint is updated after each attempt and retained under a canonical
slot/row/iteration name.

`completed_at` and `total_us` are sampled after a complete provisional summary
serialization, file fsync, rename, and directory fsync. The visible final
metadata refresh necessarily follows its own embedded timestamp; the
`completion_boundary` field explicitly states that only this unavoidable final
refresh and output printing are excluded. `summary_generation_us` includes the
complete provisional durable commit.

Run-level evidence includes:

- exact argument vector, parsed options, working directory, repository commit,
  branch, dirty state, target directory, and SHA-256 binary/native-library
  provenance;
- build features, commands, status, phase timestamps, and durations;
- before/after CPU, memory, swap, process/FD limits, cgroup task limits, and
  filesystem byte/inode snapshots;
- low-frequency peak observations for descendant process count, tasks, RSS,
  swap, open FDs, and host memory/swap use;
- scheduler dispatch/completion events with queued, active, LoLa-active, and
  per-resource active/available permit counts, plus concurrency-time
  aggregates;
- criteria, summary-generation, scheduler, build, and total run durations.

Every duration ending in `_us` is derived from `std::time::Instant`; RFC3339 UTC
timestamps exist only for correlation. Instrumentation does not infer timing by
rescanning logs. Existing readiness checks still read logs for their functional
markers.

Per-attempt timing has these boundaries:

- `queue_wait_us`: scheduler start to task dispatch. It appears on the first
  attempt and is included in that attempt and row total.
- `permit_wait_us`: the longest observed applicable resource-permit wait. It is
  absent when no separate permit wait occurred. `resource_permit_wait_us`
  retains the per-class LoLa/DDS-heavy/Zenoh-SHM/vSomeIP/MQTT breakdown.
- `preparation_us`: artifact directory, manifests, native-library resolution,
  and environment preparation.
- `namespace_us`: namespace-holder spawn through its readiness marker.
- `broker_us`: MQTT broker spawn and the existing liveness wait; zero for rows
  without MQTT.
- `config_us`: generated router/client, transport, and Streamer configuration.
- `streamer_readiness_us`: Streamer spawn through `READY streamer_initialized`.
- `passive_readiness_us`: passive command/spawn through listener readiness.
- `stabilization`: each existing fixed wait with its LoLa, Zenoh-sink, or
  vSomeIP-sink reason.
- `active_us`: active command/spawn through successful completion.
- `passive_observation_or_completion_us`: passive exit or classic observation.
- `validation_us`: established flow-log validation.
- `teardown_us`: measured explicit process teardown.
- `cooldown_us`: established LoLa cooldown plus any retry cooldown attributable
  to the attempt.
- `total_us`: complete attempt envelope; row timing separately records queue,
  execution, and complete row envelope.

Every attempt records its artifact directory, classification, failure phase,
reason, logs, native-library paths, whether it is a retry, the reason that caused
the retry, and whether another retry was scheduled. Unsupported and
planner-blocked rows have no attempts and zero row duration; an unobserved
permit wait is `null`.

## Focused Runs

Existing binaries can be used for a representative no-build check without
changing matrix policy:

```bash
cargo run -p streamer-transport-test-orchestrator -- \
  --skip-build \
  --jobs 4 \
  --lola-jobs 2 \
  --dds-jobs 2 \
  --zenoh-shm-jobs 2 \
  --vsomeip-jobs 1 \
  --mqtt-jobs 1 \
  --only <ROW_ID> \
  --artifacts-root target/streamer-transport-test/focused
```

Do not use `--skip-build`, selected rows, reduced criteria, or increased
timeouts as complete matrix acceptance evidence.
