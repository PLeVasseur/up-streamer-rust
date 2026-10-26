# Bounded Streamer workload evidence

`streamer-soak` is the OS-independent journal and reconciliation library used by
the existing Streamer transport-test orchestrator. `JournalWriter` captures complete
generation-bound records, `Reconciler` checks a bounded live message window, and
`replay` independently checks every retained chunk and message disposition.
`inbox` provides bounded, nonblocking consumer admission and explicit retained-value
accounting without configuring transport storage or copying payloads.
`control` validates plan-bound phase progression and cancellation independently of
the controller's file/channel or OS execution mechanism.

Deployment configuration, process supervision and OS resource sampling are owned
by `streamer-transport-test-orchestrator`; physical behavior remains with bindings.
Sustained live-workload integration into the existing runner is in progress. This
library does not provide a separate executable or a second transport fixture.

```sh
cargo test --locked -p streamer-soak

# Offline replay with independently supplied expected identity and bounds.
cargo run --locked -p streamer-transport-test-orchestrator -- \
  --replay-workload /path/to/stream-journal \
  --replay-options /path/to/expected-replay-options.json
```

The replay command needs retained journals and expected inputs, not live middleware,
Linux namespaces or `/proc`. See [workload evidence contracts](docs/workload.md) and
the [replay input schema](schemas/replay-options.schema.json). Public handwritten
APIs use the workspace's `missing_docs = "deny"` and strict documentation policy.
