# up-streamer-rust: what is in this repository

## up-streamer

Generic, pluggable uStreamer that should be usable in most places we need
to write a uStreamer application to bridge from one transport to another.

Reference its README.md for further details.

## example-streamer-implementations

Two concrete implementations of a uStreamer as a binary. These can be used out of the box either to try running different UStreamer setups or to directly use them in a project!

Reference the README.md there for more details.

## example-streamer-uses

A number of UEntity examples for SOME/IP, Zenoh and MQTT5. These can be used together with the example streamer implementations to run basic setups of either a publisher and a subscriber, or a service and a client.

Reference the README.md there for more details.

## Native Frame Migration Status

This branch migrates the streamer workspace to native `UOwnedFrame` and `UZeroCopyTransport` APIs. The supported runnable streamer surfaces in this branch are:

- `up-streamer`: the library API for routing native frames between endpoints.
- `configurable-streamer`: the configurable binary entry point.
- `example-streamer-implementations`: concrete example streamer binaries.
- `utils/transport-smoke-suite`: deterministic transport smoke scenarios.

The old `up-linux-streamer-plugin` crate is intentionally not restored in this migration branch. Its README already stated that it was not usable, and its implementation depended on the removed generated-envelope `UTransport` surface and `Endpoint::new` API. Restoring it should be a follow-up task after upstream PR packaging replaces local path dependencies and the plugin can be rebuilt on `Endpoint::from_owned` plus native owned transports.

The old bundled/unbundled lint workflows are replaced by `.github/workflows/native-frame-ci.yaml`. The scheduled smoke coverage is retained through `.github/workflows/transport-smoke-capstone.yaml`.

## Building

### Only `up-streamer`

If you only want to compile the library itself, you can as normal:

```
cargo build
```

### Build the native reference implementation

The migrated workspace uses local native `up-rust`, Zenoh owned-frame transport, and iceoryx2 zero-copy transport integrations.

```bash
cargo build --workspace
cargo test --workspace
```

## Deterministic transport smoke capstone

The workspace includes a deterministic smoke runner crate at `utils/transport-smoke-suite`.

Run all 8 canonical scenarios with one command:

```bash
cargo run -p transport-smoke-suite --bin transport-smoke-matrix -- --all
```

Run a single deterministic scenario:

```bash
cargo run -p transport-smoke-suite --bin smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service -- --send-count 12 --send-interval-ms 1000
```

Scenario claims are file-backed and loaded strictly from JSON:

- Default claims directory: `utils/transport-smoke-suite/claims/`
- Default per-scenario file: `utils/transport-smoke-suite/claims/<scenario-id>.json`
- Missing, malformed, or scenario-id-mismatched claim files fail the run (no in-code fallback)

`--claims-path <path>` behavior for scenario and matrix binaries:

- Omitted: use the default per-scenario file in `utils/transport-smoke-suite/claims/`
- Directory path: load `<path>/<scenario-id>.json`
- File path: use that exact file for the selected scenario

Matrix restriction:

- `transport-smoke-matrix` rejects `--claims-path <file>` when multiple scenarios are selected
- Use a directory override for multi-scenario matrix runs, or `--only <scenario-id>` for a single scenario

Single-scenario custom claims file example:

```bash
cargo run -p transport-smoke-suite --bin smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service -- --claims-path utils/transport-smoke-suite/tests/fixtures/custom-claims/smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service.json --send-count 12 --send-interval-ms 1000
```

Matrix custom claims directory example:

```bash
cargo run -p transport-smoke-suite --bin transport-smoke-matrix -- --all --claims-path utils/transport-smoke-suite/claims
```

Scenario binaries:

- `smoke-zenoh-mqtt-rr-zenoh-client-mqtt-service`
- `smoke-zenoh-mqtt-rr-mqtt-client-zenoh-service`
- `smoke-zenoh-mqtt-ps-zenoh-publisher-mqtt-subscriber`
- `smoke-zenoh-mqtt-ps-mqtt-publisher-zenoh-subscriber`
- `smoke-zenoh-someip-rr-zenoh-client-someip-service`
- `smoke-zenoh-someip-rr-someip-client-zenoh-service`
- `smoke-zenoh-someip-ps-zenoh-publisher-someip-subscriber`
- `smoke-zenoh-someip-ps-someip-publisher-zenoh-subscriber`

Default artifacts are written under:

- Scenario run: `target/transport-smoke/<scenario-id>/<timestamp>/`
- Matrix run: `target/transport-smoke/matrix/<timestamp>/`

Each scenario artifact directory includes:

- `streamer.log`
- endpoint logs (`client.log`/`service.log`/`publisher.log`/`subscriber.log`)
- `scenario-report.json`
- `scenario-report.txt`

Matrix output includes:

- `matrix-summary.json`
- `matrix-summary.txt`

Failure triage order:

1. preflight (missing tools/config/build prerequisites)
2. readiness (missing `READY streamer_initialized` or `READY listener_registered`)
3. claims (required evidence below thresholds or forbidden signatures)
4. teardown (processes failing to stop cleanly)

Rerun one scenario deterministically:

```bash
cargo run -p transport-smoke-suite --bin <scenario-id> -- --send-count 12 --send-interval-ms 1000
```

Rerun only failed scenarios from a prior matrix summary:

```bash
for s in $(jq -r '.failed_scenarios[].scenario_id' target/transport-smoke/matrix/<timestamp>/matrix-summary.json); do
  cargo run -p transport-smoke-suite --bin "$s" -- --send-count 12 --send-interval-ms 1000
done
```

When filing regressions, include:

- the failing scenario ID(s)
- the exact repro command
- `scenario-report.json` and `matrix-summary.json`
- relevant excerpts from `streamer.log` and endpoint logs
