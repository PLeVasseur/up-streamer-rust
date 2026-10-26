# Role entrypoints and shared flow

The six `iceoryx2_*` entrypoints each contain their command-line fields and
defaults, Tokio runtime, physical backend construction and concrete selected-wire
constructors. The local `run_endpoint!` macro shows the five wire choices for
both owned-frame and copy-minimized APIs. Native selection requires the supplied
agreement; serialized selections construct their codecs directly.

`src/bin/common/flow.rs` implements publication, notification and RPC over these
configured transports. Payload generation, native identity checks, flow evidence,
passive registration/readiness and sustained worker execution use the existing
common modules. `endpoint_config.rs` validates reusable backend options; actual
backend constructors remain in the entrypoint.

## DDS entrypoints

Each of the six `dds_*` binaries declares its own parser/defaults, Tokio runtime,
domain/origin/QoS configuration, carriage-family constructor and five encoded-owned
wire choices. `common/dds.rs` receives those configured endpoints and shares the
DDS message flow, payload/native validation, observations and workload handling.
It does not parse arguments or construct the backend.

The existing `classic`, `owned-frame` and `encoded-owned` families, defaults and
feature-gated workload option are unchanged. DDS encoded-owned is owned byte
carriage, not a native-loan API. Native encoding requires an explicit native
agreement; the serialized codecs remain independently selectable. Common flow
keeps the existing role-specific matching-reader discovery waits and terminal
reliable-reader acknowledgement waits within `--timeout-ms`. For physical storage,
discovery and QoS behavior, consult the
[DDS binding documentation](https://github.com/PLeVasseur/up-transport-dds-rust#readme).

## iceoryx2 compatibility and addressing

Binary names, CLI defaults and feature requirements are retained. The hidden
transport and role selectors remain available for controller-provisioned uses.
Backend-specific optional arguments keep their existing scope: for example,
`iceoryx2_publisher` exposes the receive-capacity/readiness controls and the
feature-gated `--workload-plan`; the other five entrypoints retain their own CLI.

The selected-wire fixtures retain entity `0x5BA0`, version `1` in message addresses
and receive filters. Authorities and topic/method resource flags select the
fixture endpoints. The `--ue-id` and `--ue-version-major` flags configure the
backend's local registration URI; they do not rewrite these fixture addresses.

Pub/sub receive filters are sinkless. Notification and RPC receivers use explicit
sink filters. Passive roles announce `READY listener_registered` only after
registration completes, and unregister after receipt or timeout. RPC responses
preserve request correlation, priority, TTL and payload bytes. Native received
representations are validated before inspecting their payload.

## Connectivity and sustained workloads

The finite iceoryx2 connectivity role repeats one message identity for `--send-count`
(with zero retaining the existing minimum of one send). Passive connectivity
roles finish after the first delivery. RPC clients finish after the first matching
payload response and cancel/reap their repeated-send task. These controls are
connectivity probes, not counts of distinct delivered messages.

The optional sustained workload follows its controller-provisioned plan through
the same configured native endpoint. See [sustained workloads](sustained-workloads.md)
for supported roles, accounting, queue bounds and lifecycle requirements. Physical
storage, discovery and readiness guarantees remain documented by each binding.
