# Sustained role behavior

The optional `sustained-workload` feature adds bounded application traffic to the
existing role binaries. It reuses their endpoint construction and transport-owned
configuration rather than introducing another transport setup path. The common
worker receives an already configured SDK endpoint, existing filters/native context,
and a role-specific message generator. Readiness, acknowledgements and backend health
stay at the role's existing binding boundary.

The current entry point is `--workload-plan <FILE>` on classic DDS publisher,
subscriber, client and server roles built with `dds-transport,sustained-workload`.
The classic Zenoh subscriber uses the same SDK-level worker when built with
`zenoh-transport,sustained-workload`; its existing Zenoh session/configuration path
is reused unchanged.
Other families reject that mode explicitly until their sustained hooks are integrated.
The generic `iceoryx2_publisher` role binary also hosts sustained native API roles
for configured iceoryx2/Zenoh endpoints through its existing `--transport` and
`--role` selections. `--zenoh-shm-segment-size` optionally forwards a deployment
pool budget to the Zenoh binding builder; the worker does not implement pool behavior.
`--zenoh-wait-for-subscriber` separately requests actual matching-subscriber
presence with a five-second deadline. Sustained native Zenoh roles wait before publishing;
ordinary roles retain the binding's publish-without-waiting default.
For the generic iceoryx2 role, `--iceoryx2-receive-capacity` delegates buffer/borrow
limits using its existing configured namespace, and `--iceoryx2-min-subscribers`
independently requests actual peer discovery before publishing. The sustained
fixture supplies capacity 32 and one peer to accommodate a bounded response-drain
burst. The application inbox remains 4; none of these settings lengthens quiescence.
Ordinary finite-message behavior remains the default. The high-level concurrent
workload entry point is still being integrated into the matrix orchestrator.

The controller supplies `replay` identity/bounds, a plan-bound `control_file`,
`offered_per_second`, `consumer_delay_ms`, `maximum_payload_bytes` and
`deadline_seconds`. The control locator may be relative to the worker plan.
`UPROTOCOL_FLOW_EVIDENCE_RUN` and the source/sink actor must match that plan.
Input files are bounded to 64 KiB and unknown fields fail.

Waiting/warm-up/normal/slow/drain/stop commands use the shared phase tracker. New
messages are offered only in traffic phases. The slow consumer retains its original
message while delayed; listener admission uses the shared bounded inbox. Exact full-
queue drops are journalled separately from closed-listener errors and abandoned work.
RPC processing echoes the received payload with the original request correlation.
The sustained Protobuf generator encodes an actual `HelloRequest`; other encodings
reuse the existing application payload generators with a per-message sequence.

Worker completion requires unregister, released application values, acknowledgement
where provided, and backend health after transport release. A failure/interruption is
not successful soak evidence. The application listener uses the SDK's existing
`ListenerAdmission` gate to close entry and wait for its own callbacks within one
five-second deadline; classic unregister alone is not assumed to drain callbacks.
That gate neither changes a binding's shutdown contract nor revokes retained values.
An application quiesce timeout remains a failed workload, with process supervision
responsible for bounded failure cleanup.

Queue/value counts are application measurements; classic
messages do not become native loans merely because their payload representation is
native. OS memory/process supervision belongs to the shared orchestrator. Native
backend setup and platform support remain documented by each transport binding.

## Finite conservation and diagnostics

The generic native worker frontend initializes optional tracing from `RUST_LOG`,
so backend readiness, transport rejection and native storage diagnostics appear
alongside the worker's structured accounting. Logging does not constitute readiness
or alter message admission/count criteria.

Finite conservation worker plans set `finite_messages: 5`, offer at 20/s (50 ms)
only during the normal window and use zero processing delay. Every offer constructs
a fresh message identity; source generation stops at five. Receivers remain admitted
until the controller's complete window and 1000 ms drain end, including all RPC
responses. This is distinct from first-delivery roles that repeat a prepared message
or exit after the first receipt. Wrong finite count/rate/delay combinations reject.
The finite source waits 50 ms after each completed send rather than catching up an
interval delayed by discovery. Its generated Zenoh deployment explicitly selects
reliable publications with blocking congestion control through Zenoh configuration.
The sustained workload controller uses the same explicit publication policy and
binds all generated Zenoh configuration bytes into its deployment evidence.
The finite deadline remains bounded and a blocked or lost message fails the run.
The finite inbox has capacity 5 so a complete five-message backend burst can be
admitted even before the application consumer is scheduled. Sustained workers use
their separately declared queue limits; no finite application drop is permitted.

Worker metrics report admitted RX loans, transient callback-held RX loans and TX
loans independently. Admission transitions occur after reserving queue capacity
and before exposing the lease to the consumer. Rejected full-queue samples remain
transient until their drop record is emitted; they cannot inflate admitted counts.

## Large native sensor fixtures

Build with `payload-contract-large-fixtures` to enable the existing SDK's full
LiDAR (`lidar_hesai_at128`, 4,915,328 bytes) and camera
(`camera_bayer_rggb12p8mp`, 12,441,784 bytes) representations. An explicit
`native_fixture` in the worker plan selects a typed immutable context before
deployment admission; omitted means the existing small workload. The generic
`iceoryx2_publisher` entrypoint supports the configured native iceoryx2/Zenoh
pub/sub roles and keeps backend construction visible.

Sensor plans require native pub/sub, the exact fixture size, 2 offers/s, 1000 ms slow
consumer delay and queue 4. Their actual descriptors must be linked by the exporter
and admitted in the deployment artifact. Disabled features, wrong representation,
truncated or invalid fields fail closed. The source initializes all bytes through
the SDK fixture implementation; hashes cover full transmitted bytes. Receive
validation checks identity/field bits and fixture semantics before consumption,
with retained leases rather than copied receive payloads. Publisher fixture
preparation currently uses heap-owned initialization before copying into its TX
loan; it is not claimed to be allocation-free source construction.

The representative source additionally selects
`--iceoryx2-publisher-prewarm` with its exact/static capacities. This delegates
construction-time backing setup to the binding, preserving the same TX/receiver
limits and worker behavior. See [native capacity configuration](../../configurable-streamer/docs/native-capacity.md).
