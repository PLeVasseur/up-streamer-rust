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
  generated configs, checkpoints, and the final summary.
- Only one orchestrator may build or run against a mutable target tree at a
  time. The artifact-root lock protects evidence, not Cargo build outputs.

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
- `--skip-build`: use existing `target/debug` artifacts. This is intended for
  focused diagnosis, not a complete acceptance run.
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
- `--iterations <N>`: repeat the selected matrix in canonical order, default
  `1`.
- `--disable-row-retries`: disable the configured transport retry policy.
- `--copy-minimized-sinks-only`: retain only copy-minimized sink profiles.
- `--criteria <FILE>`: validate final counts, unsupported reasons, and retry
  consumption against an accepted criteria file.

## Instrumentation

Summary schema `2.0` is written to `matrix-summary.json`. The final summary and
all `checkpoints/*.json` files use same-directory temporary files followed by
atomic rename. A checkpoint is updated after each attempt and retained under a
canonical slot/row/iteration name.

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
  available permit counts, plus concurrency-time aggregates;
- criteria, summary-generation, scheduler, build, and total run durations.

Every duration ending in `_us` is derived from `std::time::Instant`; RFC3339 UTC
timestamps exist only for correlation. Instrumentation does not infer timing by
rescanning logs. Existing readiness checks still read logs for their functional
markers.

Per-attempt timing has these boundaries:

- `queue_wait_us`: scheduler start to task dispatch. It appears on the first
  attempt and is included in that attempt and row total.
- `permit_wait_us`: the meaningful subset for which a LoLa row was observed
  blocked by the LoLa permit. It is absent when no separate permit wait occurred.
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
  --only <ROW_ID> \
  --artifacts-root target/streamer-transport-test/focused
```

Do not use `--skip-build`, selected rows, reduced criteria, or increased
timeouts as complete matrix acceptance evidence.
