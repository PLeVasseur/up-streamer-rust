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
  --jobs 8 \
  --lola-jobs 3 \
  --criteria utils/streamer-transport-test-orchestrator/matrix-criteria.json
```

## Options

- `--list`: print generated rows and support classifications without running.
- `--generate-criteria <FILE>`: derive criteria from the canonical matrix.
- `--only <ROW_ID>`: select a row; repeat the option to select multiple rows in
  the specified order.
- `--skip-build`: stage and validate a fresh immutable run bundle from existing
  `target/matrix` profile artifacts. This is intended for focused diagnosis,
  not a complete acceptance run.
- `--use-local-sibling-patches`: add available sibling repositories as Cargo
  patch configuration during the build.
- `--artifacts-root <PATH>`: choose a new, empty evidence root. Relative paths
  resolve from the workspace root.
- `--send-count <N>`: requested sends, default `1`; role commands retain the
  established minimum of five.
- `--send-interval-ms <MS>`: active-role pacing, default `50`.
- `--mqtt-readiness-timeout-ms <MS>`: timeout for a namespaced MQTT 5
  CONNECT/CONNACK readiness probe, default `250`.
- `--lola-pre-active-stabilization-ms <MS>`,
  `--zenoh-sink-stabilization-ms <MS>`, and
  `--vsomeip-sink-stabilization-ms <MS>`: optional post-readiness gates,
  default `0`. These preserve an explicit diagnostic control without imposing
  a blind wait after the corresponding readiness contract succeeds.
- `--lola-success-cooldown-ms <MS>`: post-success LoLa resource-permit hold,
  default `0`. The hold does not consume a global worker permit.
- `--lola-failed-retry-backoff-ms <MS>`: backoff before a configured LoLa
  retry, default `1000`.
- `--timeout-ms <MS>`: role operation timeout, default `5000`; established
  transport-specific floors remain in effect.
- `--scenario-timeout-secs <S>`: row scenario timeout, default `30`.
- `--max-runnable-rows <N>`: execute only the first `N` supported rows and
  classify later supported rows as planner-blocked.
- `--jobs <N>`: global worker limit, default `8`.
- `--lola-jobs <N>`: LoLa-sensitive worker limit, default `3`.
- `--dds-jobs <N>`, `--zenoh-shm-jobs <N>`, `--vsomeip-jobs <N>`, and
  `--mqtt-jobs <N>`: transport-resource caps, defaulting to DDS `4`, Zenoh-SHM
  `4`, vSomeIP `3`, and MQTT `3`. Rows acquire their complete applicable class
  set atomically in deterministic class order, so mixed-resource rows cannot
  deadlock.
- `--hard-max-jobs <N>`: configured host-policy ceiling, default `16`. Requests
  above it are rejected. The worker pool is further clamped to available CPUs
  and runnable tasks; smaller hosts therefore clamp the accepted `8/3` profile
  automatically.
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
- `--prepare-shards --shard-count <N>`: build once, stage one immutable bundle,
  and generate all deterministic shard manifests without executing rows.
- `--shard-count <N> --shard-index <I> --run-bundle <DIR>`: execute one
  zero-based shard from a prepared bundle. `--shard-manifest <FILE>` additionally
  verifies the generated preparation manifest byte-for-byte semantically.
- `--merge-shard-root <DIR>`: merge a completed shard artifact root; repeat once
  for every shard. `--merge-output <FILE>` selects the merged summary path.

## Resource And Input Safety

Before row dispatch the orchestrator checks CPU policy, available RAM, free
swap reporting, process/cgroup task capacity, the FD soft limit, user namespace
availability with an executable probe, and filesystem byte/inode capacity.
`preflight.json` is written before dispatch. Filesystems that legitimately do
not report inode quotas are identified as unsupported rather than recorded as
zero; byte capacity is still enforced. Capacity comes directly from
`statvfs(3)`, not parsed `df` output.

Cargo's target directory is resolved through `cargo metadata`, including its
handling of `CARGO_TARGET_DIR`. After a build, Cargo's JSON executable artifacts
are authoritative for the actual profile output directory; this supports both
the normal `<cargo-target>/matrix` layout and a target-qualified
`<cargo-target>/<target>/matrix` layout. `--skip-build` accepts exactly one
complete matching matrix directory and rejects missing, ambiguous, or
debug-only artifacts. The repository, target, artifact, criteria, and native
input paths are canonicalized and rejected if a private matrix `/tmp` or
`/dev/shm` mount would shadow them. Matrix children receive an empty inherited
environment plus a small allowlist: deterministic locale/timezone and logging
values, bundle-only `PATH`/`LD_LIBRARY_PATH`, forced `TMPDIR=/tmp`,
transport-specific values, and the configured `TOKIO_WORKER_THREADS`.

Orchestrator-invoked Cargo builds use the custom `matrix` profile and the
isolated `<cargo-target>/matrix` output directory. It inherits `dev`, preserving
the development optimization level, debug assertions, overflow checks, and
unwind behavior, and overrides only these artifact-size settings:

```toml
[profile.matrix]
inherits = "dev"
debug = 0
strip = "debuginfo"
incremental = false
```

Each build also sets the following exact Cargo environment so ambient values
cannot re-enable debug or incremental artifacts. Ambient `CARGO_PROFILE_*` and
`CARGO_INCREMENTAL` values are removed before these values are applied:

```text
CARGO_INCREMENTAL=0
CARGO_PROFILE_MATRIX_DEBUG=0
CARGO_PROFILE_MATRIX_INCREMENTAL=false
CARGO_PROFILE_MATRIX_STRIP=debuginfo
```

The profile name, inheritance, output directory, overrides, and environment are
recorded in build provenance, the bundle manifest, and every shard identity;
build phases also record Cargo's emitted executable paths. The bundle identity
covers the profile plus its ordered file identities. Cargo fingerprints the
isolated profile, and staging reads matrix executables only from the one
discovered matrix directory, so a stale `<cargo-target>/debug` executable cannot
enter a bundle. Bundle loading and shard merge reject profile, environment,
target-directory, or executable source-directory drift across build, bundle,
identity, and run provenance.

After the single build, every executable used by row supervision and every
selected LoLa/vSomeIP native library is staged once under
`run-bundle/{bin,lib,objects}`. Inputs are copied once into content-and-mode
addressed objects so the immutable snapshot shares no inode with mutable target
artifacts; no binary is copied per row. Launch names are read-only hard links or
copies, and `run-bundle/manifest.json` records source and bundle paths, kind,
size, mode, transfer method, SHA-256, and the exact Cargo profile contract. The
complete bundle is checked for exact directory contents, hashes, permissions,
profile, and schema before preflight and again after execution, and rows launch
project and supervision executables only from the bundle. Replacing or
modifying a mutable target path after staging cannot alter or redirect an active
run.

Every child is a process-group leader. Teardown and cancellation signal the
whole group with bounded SIGINT, SIGTERM, and SIGKILL escalation, reap the
leader, verify that no group descendants remain, and expose cleanup/leak counts
in the summary. Both SIGINT and SIGTERM set the same cooperative cancellation
flag.

## Scheduling Policy

Runnable rows use policy `deterministic_weighted_transport_role_lane_fair_v1`.
Rows are grouped by ordered physical-transport pair and role. The scheduler
repeatedly chooses the least-served lane by estimated work, prefers a lane that
does not overlap the previous row's transports when fairness is tied, and uses
the lane identity as the final deterministic tie-breaker. Dispatch still scans
past a permit-blocked row, so it remains work-conserving with XR's atomic
multi-resource permits. Results are restored to canonical matrix slots.

The static estimates are rounded from the accepted R11-XI `4/2` equivalence
endpoint/role class means. They are ordering units, not timeouts or performance
claims:

| Endpoint class | Pooled endpoint mean | Cost units |
| --- | ---: | ---: |
| DDS | 2.44 s | 12 |
| iceoryx2 | 2.70 s | 14 |
| MQTT5 | 3.80 s | 19 |
| Zenoh | 3.86 s | 20 |
| LoLa | 4.18 s | 21 |
| vSomeIP | 4.27 s | 22 |

A row adds its two endpoint weights. Client/server RPC adds four units for its
measured class delta; publish/subscribe and notifier/notifyee add zero. A
structurally unsupported row costs zero because it launches no work. Row
results, scheduler events, shard manifests, and scheduler totals record the
estimate; runnable rows also record their scheduling priority.

Shard assignment is deterministic LoLa-first multidimensional longest-
processing-time placement. LoLa rows choose the least-loaded LoLa shard, then
total work; other runnable rows choose least total work; structural rows balance
row counts. Canonical ordinal is the final tie-breaker. Tests require every row
exactly once and bound total/LoLa estimate skew by one maximum row estimate.

## Sharded Execution

Shards never build or stage from mutable `target/`. Prepare once from the exact
source and criteria:

```bash
target/debug/streamer-transport-test-orchestrator \
  --prepare-shards \
  --shard-count 2 \
  --criteria utils/streamer-transport-test-orchestrator/matrix-criteria.json \
  --artifacts-root target/streamer-transport-test/prepared-2
```

Preparation writes `run-bundle/` and
`shard-manifests/shard-00000-of-00002.json` (and every other shard manifest).
Run the staged orchestrator, not a mutable target binary. Repeat the same
selection and execution-affecting options used at preparation:

```bash
PREP=target/streamer-transport-test/prepared-2
"$PREP/run-bundle/bin/streamer-transport-test-orchestrator" \
  --run-bundle "$PREP/run-bundle" \
  --shard-count 2 \
  --shard-index 0 \
  --shard-manifest "$PREP/shard-manifests/shard-00000-of-00002.json" \
  --criteria utils/streamer-transport-test-orchestrator/matrix-criteria.json \
  --artifacts-root target/streamer-transport-test/shard-0
```

Run multiple shards sequentially on one host. Concurrent same-host shards are
not authorized because their combined host envelope is not budgeted. For
separate hosts, copy the complete read-only `run-bundle/` and that host's shard
manifest while preserving permissions, use an identical source checkout for
configuration inputs, validate the bundle by starting its staged orchestrator,
and execute one shard per independently budgeted host. No host builds or stages.

After collecting every complete shard artifact root, merge with the staged
orchestrator:

```bash
"$PREP/run-bundle/bin/streamer-transport-test-orchestrator" \
  --merge-shard-root target/streamer-transport-test/shard-0 \
  --merge-shard-root target/streamer-transport-test/shard-1 \
  --criteria utils/streamer-transport-test-orchestrator/matrix-criteria.json \
  --merge-output target/streamer-transport-test/merged-matrix-summary.json
```

Merge rejects incompatible schemas; any Cargo profile/environment/target,
matrix, selection, criteria, orchestrator, dependency, bundle, binary,
native-library, or normalized-option identity mismatch; duplicate/missing shards
or rows; manifest/count/cost drift; failed/blocked rows; and any consumed retry.
A full selection is reconstructed in canonical 2160-row order and validated
against the unchanged full criteria.
Focused selections validate their exact generated counts and the same
unsupported/retry policy but are not full-matrix acceptance evidence.

## Instrumentation

Summary schema `6.0` is written to `matrix-summary.json`. Checkpoints use a
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
  branch, dirty state, target directory, Cargo profile/environment, and SHA-256
  binary/native-library provenance;
- build features, commands, status, phase timestamps, and durations;
- before/after CPU, memory, swap, process/FD limits, cgroup task limits, and
  filesystem byte/inode snapshots;
- low-frequency peak observations for descendant process count, tasks, RSS,
  swap, open FDs, and host memory/swap use;
- scheduler dispatch/completion/resource-hold-completion events with queued,
  active, LoLa-active, per-resource active/held/available permit counts, plus
  scheduling priority/cost and concurrency-time aggregates;
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
- `broker_us`: MQTT broker spawn through a successful namespaced MQTT 5
  CONNECT/CONNACK exchange; zero for rows without MQTT.
- `config_us`: generated router/client, transport, and Streamer configuration.
- `streamer_readiness_us`: Streamer spawn through `READY streamer_initialized`.
- `passive_readiness_us`: passive command/spawn through listener registration,
  including explicit selected-wire Zenoh subscriber declaration. vSomeIP publish rows first
  establish local listener readiness, then require `SUBSCRIBE ACK` after the
  active publisher's first send causes the provider to offer its event.
- `readiness`: contract, target marker/probe, timeout, check count, configured
  stabilization, and measured duration for every readiness phase and optional
  post-readiness gate.
- `stabilization`: each configurable LoLa, Zenoh-sink, or vSomeIP-sink gate and
  its measured duration.
- `active_us`: active command/spawn through successful completion.
- `passive_observation_or_completion_us`: passive exit or classic observation.
- `validation_us`: established flow-log validation.
- `teardown_us`: measured explicit process teardown.
- `cooldown_us`: retry backoff attributable to the attempt. `cooldown` records
  the configured contract and scope for retry backoff and post-success LoLa
  resource-permit holds.
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
