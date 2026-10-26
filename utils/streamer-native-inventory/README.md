# Streamer native inventory exporter

This executable directly links `streamer-native-types` and requires both actual
application representations to appear in the SDK inventory. It exports complete
canonical descriptors for the target it was compiled for, without loading any
transport backend or substituting a host-layout description.

```text
cargo run -p streamer-native-inventory -- --output native-inventory.json \
  --expect-target x86_64-unknown-linux-gnu
```

Build and execute the exporter for a different target to obtain that target's
inventory. `--expect-target` validates the compiled target; it cannot relabel it.
The output is the provisional inventory format consumed by `up-native-profile`.
Use the Streamer native-deployment guide for generation and startup admission.

The orchestrator also supplies `--agreement-output`, `--domain`, `--seeds` and
`--identities` to invoke the owning `up-native-profile` table generator over the
actual exported inventory. Those explicit generation inputs and both output
hashes are retained in the immutable run bundle. Export-only use needs none of
the generation arguments.
# Large-fixture export

Build with `payload-contract-large-fixtures` to require both actual SDK sensor
types in the linked inventory alongside the ordinary example types. This adds
their target-qualified descriptors; a host export is never relabelled as another
target. Worker configurations explicitly select the intended fixture and agreement.
See the [worker guide](../../example-streamer-uses/docs/sustained-workloads.md).
