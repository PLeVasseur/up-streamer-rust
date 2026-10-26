# Workload evidence contracts

## Ownership

The existing matrix orchestrator owns execution, endpoint/role configuration,
native deployment artifacts, immutable executable bundles, readiness and process
cleanup. Its shared platform adapters own OS measurements. Bindings own middleware
discovery, physical storage and transport shutdown. This library owns bounded
journals and message reconciliation, independent of all those host operations.

## Identity and journal bounds

`control::PhaseTracker` validates plan-bound waiting, warm-up, normal, slow-consumer,
drain and stop commands. Only traffic phases permit new offers. Repeated polling is
idempotent; foreign generations, skipped windows and regressions leave state unchanged
and fail. Stop is allowed early for cancellation and cannot be followed by resumed
traffic. A terminal command is not proof that required workload windows completed.
The controller supplies I/O/barriers and verifies elapsed durations separately.

`ReplayOptions` declares the expected run, source qualification SHA-256, frozen
plan SHA-256, role, application queue capacity and pending-message limit. These
come from the independently frozen workload inputs, not a journal choosing its own
acceptance context. Each record and manifest carries that identity. The pending
limit is explicit and at most65,536 identities; retired duplicate detection keeps
at most twice that limit. Late facts outside the retired window still cannot
form a complete source-sequenced message and must fail final reconciliation.

Records are newline-terminated and at most64KiB. Compressed chunks contain at most
the chosen64KiB-to4MiB uncompressed limit. Every chunk has a content hash, ordinal
extent and previous-chunk hash. Replay checks all chunks and their complete extents,
not a sampled subset. Closing with `complete = false` preserves failed evidence and
cannot yield a passing replay. Existing chunk/manifest files are never overwritten.

## Message accounting

`inbox::bounded` supplies a transport-independent application queue for slow
consumers. Listener admission never waits for processing; a full or closed queue
returns a retained delivery so its exact identity can be journalled before release.
The consumer retains the original value during its delay and explicitly completes
it. Dropping accepted work without completion records abandonment. Full-queue loss,
closed-queue rejection and abandonment are distinct. Closing a receiver does not
revoke a delivery already held by a consumer.

Counters measure values retained by inbox/delivery handles, not middleware-internal
loans or all storage before admission. Queue capacity does not configure a native
sample pool; deployment uses the binding's existing capacity contract separately.

Worker lifecycle records surround message facts. For each accepted publication or
request the oracle requires actual Streamer ingress/egress and exactly one sink
disposition: processed or explicit queue drop. Payload hashes, full attribute hashes,
source sequence and route-registration continuity must match. Receive ordering is
an explicit deployment policy: `require_fifo` rejects reordering when that contract
is guaranteed. Otherwise the oracle counts out-of-order receipts and their maximum
backward distance within the bounded reconciliation window. It never sorts away
observations, tolerates duplicates or treats missing messages as reordered delivery.
Asynchronous SDK listeners do not acquire a universal FIFO guarantee from the oracle.
RPC additionally requires a complete response leg, echoed payload, correlation and
reversed endpoints. Duplicate, foreign, corrupt or unexplained observations fail.

Workers emit transport-neutral `OP_TRANSPORT_HEALTH` error-counter objects when
their adapters expose backend health. Every reported counter must be zero; empty,
malformed or nonzero reports fail. The adapter defines its backend's diagnostics;
the oracle has no DDS-specific lifecycle policy.

Final endpoint metrics must match journal receipt/processing/drop counts, with
queues and application-held storage leases drained. This is separate from OS
process-memory return and binding-internal resource cleanup. The orchestrator
additionally verifies phase durations, offered rate, declared loss budget, resource
requirements and process cleanup; journal success alone is not complete soak acceptance.

## Portable replay

The same replay and failure semantics apply on Linux and macOS. Replay never
needs namespaces, `/proc`, native runtime installations or middleware filenames.
Keep all journal chunks with the manifest and frozen inputs. A packet containing
only manifests/samples must explicitly state that absent journals cannot be replayed.

```no_run
# fn main() -> anyhow::Result<()> {
let expected: streamer_soak::ReplayOptions = serde_json::from_slice(
    &std::fs::read("expected-replay-options.json")?
)?;
let report = streamer_soak::replay(std::path::Path::new("stream-journal"), &expected)?;
println!("{}", serde_json::to_string_pretty(&report)?);
# Ok(())
# }
```
