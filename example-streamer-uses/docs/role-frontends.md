# Role entrypoints and shared flow

Finite connectivity controllers may set `UPROTOCOL_SOURCE_START_FILE` on the
active role. After setup it announces `READY source_waiting` and waits for the
controller to atomically write exactly `START` followed by a newline. Missing
release times out after 30 seconds; malformed or unreadable markers fail without
publication. The wait completes once per process, before first traffic; it sends
no warm-up samples or first-client RPC requests. Ordinary examples omit the
variable. Sustained workers retain their existing phase-control barrier.
Controllers must use a fresh file and release only after the passive receiver and
bridge report readiness. Request factories run after this barrier so setup does
not consume the new request's TTL.

Controllers can also set `UPROTOCOL_SOURCE_FINISH_FILE` for finite generic owned
or native publication/notification sources. After the configured sends, the
source reports `READY source_complete_waiting` and retains its endpoint until the
controller atomically writes exactly `STOP` followed by a newline. The controller
releases this only after observing receiver completion. The wait is bounded by
30 seconds and rejects malformed/unreadable markers. It sends no extra samples
and is not a substitute for subscriber readiness: successful native publication
alone does not prove consumption before a publisher is destroyed. Ordinary
examples omit this variable and retain their normal exit behavior.

Prepared native construction uses the shared
[`PreparedAddress` catalog and `PreparedSession`](../../utils/configurable-streamer-wire-support/docs/prepared.md).
Exact TX destinations, including the client's response address, are reserved
before traffic. Finite receive owns one bounded native inbox and its physical
subscription; it does not insert a second binding mailbox or spawn a task for
each sample. RPC response registration precedes publication. Header identity and
decoded source/sink filters are checked before compatibility metadata is exposed.
The diagnostic compatibility view owns decoded metadata but retains the actual
native frame/payload, so it is distinct from the typed in-place allocation path.

Owned receives in `common/receive.rs` reserve one first-result slot and a setup-owned registration
lifecycle task. Readiness follows completed registration, including when the
backend delivers a frame during registration itself. Explicit stop awaits native
unregister; dropping a caller closes admission immediately and lets that existing
task finish retirement. Cleanup therefore survives cancellation during setup,
receive or stop without a per-message task or an unbounded receive channel.

For optional iceoryx2 slice/allocation/subscriber/TX controls, see
[native capacity configuration](../../configurable-streamer/docs/native-capacity.md).
Those options are visible in every iceoryx2 frontend and applied to the real
binding configuration with typed validation.

The six `iceoryx2_*` and six `lola_*` entrypoints retain their command-line fields,
defaults, Tokio runtime and role-specific inputs. `common/endpoint.rs` owns the
backend construction match and the five concrete wire choices for owned-frame
and copy-minimized APIs. `common/backend.rs` shares classic binding constructors
and the owned Zenoh wire selector. Native selection requires the supplied
agreement; serialized selections construct their codecs directly.

`src/bin/common/flow.rs` implements publication, notification and RPC over these
configured transports. Payload generation, native identity checks, flow evidence,
passive registration/readiness and sustained worker execution use the existing
common modules. `endpoint_config.rs` validates reusable backend options and
delegates prepared-core setup to the binding-owned constructors. These helpers
compile once in the example-support library rather than once in every role.

LoLa roles retain their explicit primary/optional RPC-response channel settings.
Reusable option validation requires a canonical complete MW COM manifest shared
by both channels; the shared LoLa factory constructs either a prepared native
core or an owned `UTransportLola` endpoint from those inputs. Hidden compatibility transport/role selectors and existing
defaults remain available. See the
[LoLa binding documentation](https://github.com/PLeVasseur/up-transport-lola-rust#readme)
for physical sample configuration, setup and storage lifecycle requirements.

## DDS entrypoints

Each of the six `dds_*` binaries declares its own parser/defaults, Tokio runtime
and domain/origin/QoS inputs. It passes an explicit `common/dds.rs::DdsArgs` value
to the shared DDS constructor and role runner. That module owns the carriage-family
and five encoded-owned wire choices together with DDS message flow, payload/native
validation, observations and workload handling. It does not depend on a binary's
parser type.

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
For `iceoryx2_publisher`, `--iceoryx2-min-subscribers` and
`--iceoryx2-receive-capacity` apply to both owned-frame and copy-minimized modes,
including controller-selected RPC roles. A requested subscriber minimum reaches
the binding before the first publication; it is not replaced by a fixed delay.

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
application config, URI validation and runtime; the common backend factory applies
the explicit `TransportConfig::new(payload_encoding)` setup. Its `--encoding` convention must
agree at both endpoints and supports native/protobuf/xcdrv2/arrow/omgidl, defaulting
to protobuf. Serialized modes need no native agreement. Existing configuration
defaults remain relative to the owning Cargo manifest directory. Physical mapping,
native setup and the request/session/client-ID reconstruction boundary belong to
the [SOME/IP binding](https://github.com/eclipse-uprotocol/up-transport-vsomeip-rust#readme).

All six Zenoh entrypoints retain their CLI/defaults, runtime, configuration and
role-specific URI/payload policies. Shared backend factories and wire selectors
construct their endpoints, while the existing flows handle messages:

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
  correlation, payload comparison and cleanup when sending fails. Prepared native
  clients attach one response receiver before the first request and retain it
  across exchanges. Owned clients now do the same, arming exact request-ID and
  response-kind admission before each send. The first matching result completes
  the exchange; unrelated IDs and duplicate responses cannot displace it. Failed
  sends close admission and await retirement of that registration. The configured
  100ms receive-start settle delay, timeout, repeated request identity, finite
  send count and zero-count unbounded exchange policy are retained. This settle
  delay is existing connectivity behavior, not a registration-readiness proof.
  It remains a per-exchange pause for owned clients and is included in their fixed
  response deadline; waiting does not renew that deadline.

Publisher/subscriber/client custom URI fields and payload factories remain
distinct from the generic fixture addresses. Owned selected publisher repeats
retain their immediate-send policy; native repeats retain the configured interval.
Classic counter exhaustion and target-size count overflow fail with typed errors.
The three custom Zenoh roles also use the shared prepared constructor/session.
Publishers reserve their actual encoded payload extent; clients reserve request
TX plus response RX before admission. Receive-only entries reserve no TX payload
loan, and received frames retain the publisher's native owner through cleanup.
The ordinary `zenoh-transport` feature includes this shared setup, so selected
wire examples do not rely on another workspace member enabling it.

The finite iceoryx2/LoLa connectivity role repeats one message identity for `--send-count`
(with zero retaining the existing minimum of one send). Passive connectivity
roles finish after the first delivery. RPC clients finish after the first matching
request-ID/payload response and retire their registered receiver. These controls are
connectivity probes, not counts of distinct delivered messages.
Generic owned and native RPC clients drive repeated-send and correlated-receive
futures together in their caller's task. The first matching reply drops the
remaining send future; send failure or caller cancellation drops its counterpart.
No detached repeated-send task can outlive the exchange. Owned response admission
is installed before first-send readiness, including the single-request case.

The optional sustained workload follows its controller-provisioned plan through
the same configured native endpoint. See [sustained workloads](sustained-workloads.md)
for supported roles, accounting, queue bounds and lifecycle requirements. Physical
storage, discovery and readiness guarantees remain documented by each binding.
