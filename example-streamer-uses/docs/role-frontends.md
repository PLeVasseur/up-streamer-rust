# Role entrypoints and shared flow

The six `iceoryx2_*` and six `lola_*` entrypoints each contain their command-line fields and
defaults, Tokio runtime, physical backend construction and concrete selected-wire
constructors. The local `run_endpoint!` macro shows the five wire choices for
both owned-frame and copy-minimized APIs. Native selection requires the supplied
agreement; serialized selections construct their codecs directly.

`src/bin/common/flow.rs` implements publication, notification and RPC over these
configured transports. Payload generation, native identity checks, flow evidence,
passive registration/readiness and sustained worker execution use the existing
common modules. `endpoint_config.rs` validates reusable backend options; actual
backend constructors remain in the entrypoint.

LoLa roles retain their explicit primary/optional RPC-response channel settings.
Reusable option validation requires a canonical complete MW COM manifest shared
by both channels; each role constructs `UTransportLola` and chooses its native or
owned endpoint locally. Hidden compatibility transport/role selectors and existing
defaults remain available. See the
[LoLa binding documentation](https://github.com/PLeVasseur/up-transport-lola-rust#readme)
for physical sample configuration, setup and storage lifecycle requirements.

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

## iceoryx2 and LoLa compatibility and addressing

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

MQTT's six classic roles share send iteration, listener waits and payload factories
while preserving explicit broker/options/connection setup. Active roles accept
`--encoding native|protobuf|xcdrv2|arrow|omgidl`; the default remains protobuf.
The ordinary `mqtt-transport` feature includes the selected codec dependencies,
so this capability does not depend on workspace feature unification. Arrow uses
the actual telemetry-table IPC encoder and OMG IDL the vehicle-status encoder;
each message carries its matching private payload identity. Native still requires
its deployment agreement. Passive MQTT roles preserve per-message encoding;
non-Protobuf publications remain opaque and RPC servers echo their bytes and
identity. Broker storage/mapping/setup remain in the MQTT binding documentation.

SOME/IP's six roles use the same codec-backed payload selection and shared classic
flow with the ordinary `vsomeip-transport` feature. Each entrypoint keeps its native
application config, URI validation, runtime and explicit
`TransportConfig::new(payload_encoding)` setup. Its `--encoding` convention must
agree at both endpoints and supports native/protobuf/xcdrv2/arrow/omgidl, defaulting
to protobuf. Serialized modes need no native agreement. Existing configuration
defaults remain relative to the owning Cargo manifest directory. Physical mapping,
native setup and the request/session/client-ID reconstruction boundary belong to
the [SOME/IP binding](https://github.com/eclipse-uprotocol/up-transport-vsomeip-rust#readme).

All six Zenoh entrypoints retain their CLI/defaults, runtime, configuration and
concrete backend/wire constructors while sharing actual message flow:

- `common/classic.rs` owns delayed classic send iteration, timer payloads and
  admitted listeners awaiting process shutdown. Passive classic roles await
  shutdown without parking a Tokio worker thread. Entry-specific bounded versus
  unbounded counts and final waits stay explicit.
- `common/flow.rs` owns finite owned/native sends and the generic notification/
  RPC-server flows; `common/receive.rs` owns registered receive and cleanup.
  Zenoh-specific readiness markers follow successful registration. Receivers
  unregister after finite receipt or timeout, retain the received frame/lease
  through cleanup and validate native representations before inspection.
- `common/rpc.rs` owns the selected-wire client's exchange loop, response polling,
  payload comparison and cancellation/reaping when sending fails. Its configured
  100ms receive-start settle delay, timeout, repeated request identity, finite
  send count and zero-count unbounded exchange policy are retained. This settle
  delay is existing connectivity behavior, not a conservation readiness proof.

Publisher/subscriber/client custom URI fields and payload factories remain
distinct from the generic fixture addresses. Owned selected publisher repeats
retain their immediate-send policy; native repeats retain the configured interval.
Classic counter exhaustion and target-size count overflow fail with typed errors.

The finite iceoryx2/LoLa connectivity role repeats one message identity for `--send-count`
(with zero retaining the existing minimum of one send). Passive connectivity
roles finish after the first delivery. RPC clients finish after the first matching
payload response and cancel/reap their repeated-send task. These controls are
connectivity probes, not counts of distinct delivered messages.

The optional sustained workload follows its controller-provisioned plan through
the same configured native endpoint. See [sustained workloads](sustained-workloads.md)
for supported roles, accounting, queue bounds and lifecycle requirements. Physical
storage, discovery and readiness guarantees remain documented by each binding.
