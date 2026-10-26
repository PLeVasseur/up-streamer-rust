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
fixture supplies capacity32 and one peer to accommodate a bounded response-drain
burst. The application inbox remains4; none of these settings lengthens quiescence.
Ordinary finite-message behavior remains the default. The high-level concurrent
workload entry point is still being integrated into the matrix orchestrator.

The controller supplies `replay` identity/bounds, a plan-bound `control_file`,
`offered_per_second`, `consumer_delay_ms`, `maximum_payload_bytes` and
`deadline_seconds`. The control locator may be relative to the worker plan.
`UPROTOCOL_FLOW_EVIDENCE_RUN` and the source/sink actor must match that plan.
Input files are bounded to64KiB and unknown fields fail.

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
