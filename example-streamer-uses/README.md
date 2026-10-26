
# Running entities

Finite worker plans can exercise full-window five-message conservation through
the same endpoints; see [workload workers](docs/sustained-workloads.md) for unique
identity, pacing and receiver drain behavior.
The generic native worker frontend also honors `RUST_LOG` for backend diagnostics.
Finite controllers can hold the active role's first send with
`UPROTOCOL_SOURCE_START_FILE`; see [role startup](docs/role-frontends.md) for the
readiness marker, exact release bytes and timeout. Normal examples omit it.
The same guide documents `UPROTOCOL_SOURCE_FINISH_FILE`, an optional controller
barrier retaining a finite generic publisher until receiver completion instead of
destroying its native endpoint immediately after a successful send.

The optional `payload-contract-large-fixtures` feature enables full SDK sensor
representations in explicitly configured native pub/sub worker plans. See
[sustained workloads](docs/sustained-workloads.md) for type, size and workload limits.

The six iceoryx2 role frontends accept optional publisher slice/allocation,
subscriber-ceiling and TX-loan settings. See the
[native capacity guide](../configurable-streamer/docs/native-capacity.md) for exact
flags and namespace requirements; ordinary defaults and multi-subscriber support
are retained.
The `iceoryx2_publisher` readiness and receive-capacity arguments apply to both
owned-frame and copy-minimized modes, including its controller-selected RPC role.

Role files keep their CLI definitions, defaults, runtime and role-specific configuration.
The example-support library shares backend construction, selected-wire dispatch,
message flow, native validation and evidence. DDS setup receives explicit `DdsArgs`
independent of each parser. See
[role entrypoints and shared flow](docs/role-frontends.md) for addressing,
connectivity semantics and the boundary with sustained workloads.
All six Zenoh roles share classic/selected-wire send, receive and RPC flow and
backend factories. Their local configuration, defaults, URI choices and payload
policies stay visible in the entrypoints. The role guide describes timing, readiness and cleanup semantics.
Prepared native roles share explicit address/capacity catalogs and `PreparedSession`
setup with the configurable frontend. Native receivers attach before READY and
before publishing a request that needs a response. Finite compatibility controls
retain the actual native frame while decoding metadata for diagnostic output;
this allocating conversion is separate from in-place typed allocation evidence.
Owned RPC roles also register their response listener before first-send readiness
and reuse it across exchanges. One first-result slot admits the expected response
ID; unrelated and duplicate responses cannot replace it. One setup-owned lifecycle
task completes unregister after caller cancellation, rather than allocating a
receive task per exchange. Passive owned receives through this shared helper use the same bounded ownership
and readiness mechanism.
Generic owned/native RPC clients also drive repeated sends and correlated receive
in one caller task, so completion or cancellation cannot leave a detached sender.
The custom Zenoh publisher, subscriber and client use that same setup while
retaining their custom URI fields, payloads and send timing. The ordinary
`zenoh-transport` feature includes their prepared native constructors.
MQTT classic roles additionally support Arrow and OMG IDL payloads with the
ordinary `mqtt-transport` feature. Their existing flags and defaults remain;
`--encoding` adds `arrow` and `omgidl`. See the role guide for encoding and opaque
receive/RPC behavior and the [MQTT binding](https://github.com/eclipse-uprotocol/up-transport-mqtt5-rust#readme)
for broker configuration and physical mapping.
SOME/IP roles support those same five payload choices through `vsomeip-transport`;
the explicit payload convention must match across the configured native endpoints.
Their application configuration defaults and native setup remain in each role.

## Running a UStreamer

To run one of the examples below and see two entities communicate, you'll need to run one of the two example implementations of the UStreamer to bridge between the transports in a terminal. Check out the `example-streamer-implementation` README for details.

## Running two UEntities

To launch any of the entities choose the correct bin and give it the appropriate feature flags:

| --bin      | --features        |
| ---------- | -------------     |
| mqtt_*     | mqtt-transport    |
| zenoh_*    | zenoh-transport   |
| someip*    | vsomeip-transport |
| dds_*      | dds-transport     |

Combined:

```bash
cargo run --bin <transport-entity> --features <*-transport>
```

To check which combinations of entities are possible for a test run check the `example-streamer-implementation` README.
Before running any entity that uses MQTT remember to first start up a MQTT broker, for example the mosquitto broker found in this repo:

```bash
cd ../utils/mosquitto
docker compose up
```

# Working setup: a mechatronics client and a high compute service

This example should help with understanding what the streamer does and how it can be used. The first part of the system that we are building here is a mechatronics component, which could be some embedded chip deep down in the belly of a car, connected to some mechanical parts like a windshield wiper. This component can (or wants to) only use SOME/IP as a transport protocol either because thats what its developers like to use or because of some hardware limitation. The second component is a high compute unit running some small Linux system that serves as the vehicles boardcomputer. The software thats running here uses Zenoh as its preferred transport protocol because a solution architect once decided that it should do that. The UStreamer comes into play as the system that allows these two components to communicate with each other while staying true to their respective protocols.

Because the streamer cant run on the mechatronics chip, it runs on the high compute unit. In our setup we want the mechatronics entity to request some information from the high compute unit, so the entity type on the SOME/IP side should be a client. If the chip indeed controls the windshield wipers it might want to periodically ask the board computer to provide information about if and how much its raining. The high compute unit should respond to the chip with the latest data that it has available, so it must be an entity of the type service.

## Run the Zenoh-SOME/IP Streamer

To launch the streamer, go to the example-streamer-implementations folder and run the binary with the correct feature flags and the provided config file in a new terminal:

```bash
cd ../example-streamer-implementations
cargo run --bin zenoh_someip --features="zenoh-transport vsomeip-transport" -- --config='DEFAULT_CONFIG.json5'
```

This will start the streamer. By itself it should not do much, except for logging the vsomeip version every now and then.

## Run the mechatronics client

To launch the mechatronics entity, run the binary `someip_client` example in a new terminal:

```bash
cargo run --bin someip_client --features someip_transport
```

You should see in the log that the client is trying to send messages of type UMESSAGE_TYPE_REQUEST. These messages should now also show up in the logs of your streamer terminal just after the SOME/IP client sends them. Since there is no other component they are not yet being forwarded anywhere though.

## Run the high compute unit

Lastly to launch the high compute unit that has access to whatever data the mechatronics part is requesting, launch a zenoh service in yet another terminal:

```bash
cargo run --bin zenoh_service --features zenoh_transport
```

Immediately after launching you should see that this entity is logging incoming requests of type UMESSAGE_TYPE_REQUEST and right after is trying to answer them with UMESSAGE_TYPE_RESPONSE messages. These should then show up again in the log of the UStreamer, and also in the log of the mechatronics SOME/IP client.

The service and client will run forever. Every second a new request is made from the someip_client via vsomeip. That vsomeip message is caught and routed over Zenoh to the zenoh_service. The response makes the same journey in reverse.

To better track the messages you can also check that the payload is listed with `@i=n` where n is incremented on each send so that we can trace the message back and forth over the streamer.


Further, you will see printed the deserialized `HelloResponse` ProtoBuf object:

> Here we received response: HelloResponse { message: "The response to the request: someip_client@i=n", special_fields: SpecialFields { unknown_fields: UnknownFields { fields: None }, cached_size: CachedSize { size: 0 } } }

## CLI parameter overrides

All 12 example binaries now support a common local-entity override surface:

- `--uauthority <string>`
- `--uentity <u32 decimal|hex>`
- `--uversion <u8 decimal|hex>`
- `--resource <u16 decimal|hex>`

Additional role-specific flags:

- Client binaries: `--target-authority --target-uentity --target-uversion --target-resource`
- Subscriber binaries: `--source-authority --source-uentity --source-uversion --source-resource`

Transport-specific flags:

- MQTT binaries: `--broker-uri` (default `localhost:1883`)
- Zenoh binaries: `--endpoint` (existing behavior, now composed with URI overrides)
- SOME/IP binaries: `--vsomeip-config` and `--remote-authority`

Deterministic sender controls (active client/publisher binaries only):

- `--send-count <n>`
  - `0` (default) preserves existing infinite-send behavior
  - any value `>0` performs a bounded send run and then exits
- `--send-interval-ms <ms>`
  - defaults to `1000`
  - controls delay between sends for bounded and unbounded runs

Running with no extra flags keeps prior behavior (defaults are aligned with previous constants).

## Native payload configuration

Native workers require an independently provisioned manifest and explicit root,
route, local endpoint and peer endpoint. Set all five deployment environment
variables in the [native deployment guide](../configurable-streamer/docs/native-deployment.md).
Startup verifies complete route agreement and the actual linked application type.
Partial, legacy or mismatched configuration fails; there is no automatic ID 0 fallback.

See [native deployment profiles](../configurable-streamer/README.md#native-deployment-profiles)
for generation, private allocations and endpoint configuration. The matrix
orchestrator supplies these paths for native rows and requires real sink identity
verification evidence. Byte-copy receive paths check the full identity and field
representation; typed loan paths additionally use the SDK's alignment, lifetime
and provenance gate.

## DDS roles

`dds_publisher`, `dds_subscriber`, `dds_notifier`, `dds_notifyee`, `dds_client`,
and `dds_server` are Streamer example-surface binaries. Each accepts
`--domain-id`, `--origin-id`, `--reliability`, `--history-depth`,
`--route-family classic|owned-frame|encoded-owned`, and
`--encoding native|protobuf|xcdrv2|arrow|omgidl`. Passive roles print
`READY listener_registered`; successful sends and receives print `FLOW`
evidence. Active DDS roles wait for publication matching rather than relying on
retry sends.

Terminal DDS sends also wait for matched reliable-reader acknowledgements before
dropping their writer. A local write is not a remote-delivery barrier; discovery
and acknowledgement waits use the existing `--timeout-ms` bound. No retry send or
fixed shutdown sleep substitutes for this completion condition.

For a direct Arrow encoded-owned pub/sub smoke, start the subscriber first:

```bash
cargo run -p example-streamer-uses --features dds-transport --bin dds_subscriber -- \
  --domain-id 80 --origin-id subscriber --route-family encoded-owned \
  --encoding arrow --local-authority authority-b --peer-authority authority-a
cargo run -p example-streamer-uses --features dds-transport --bin dds_publisher -- \
  --domain-id 80 --origin-id publisher --route-family encoded-owned \
  --encoding arrow --local-authority authority-a --peer-authority authority-b
```

The route-family flag selects the adapter used by these examples. For its
backend storage and copy guarantees, consult the
[DDS binding documentation](https://github.com/PLeVasseur/up-transport-dds-rust#readme).

### Numeric formats

Numeric URI flags accept decimal and `0x`/`0X` prefixed hex.

Decimal example:

```bash
cargo run -p example-streamer-uses --bin mqtt_publisher --features mqtt-transport -- --uauthority authority-a --uentity 23456 --uversion 1 --resource 32769
```

Hex example:

```bash
cargo run -p example-streamer-uses --bin mqtt_publisher --features mqtt-transport -- --uauthority authority-a --uentity 0x5BA0 --uversion 0x1 --resource 0x8001
```

Invalid formats (for example underscores in numeric values) are rejected with deterministic errors that include the flag name, raw value, and expected range.

### SOME/IP caveat

SOME/IP binaries accept URI overrides, but runtime compatibility can still depend on application/service IDs configured in the selected `--vsomeip-config` file. If `--uentity` is overridden, the binaries emit a startup warning when that override may conflict with vsomeip config expectations.

For protocol mappings and supported communication roles, consult the
[vSomeIP binding documentation](https://github.com/eclipse-uprotocol/up-transport-vsomeip-rust#readme).
# Sustained workload integration

The optional `sustained-workload` feature reuses existing role endpoint construction
with bounded application queues and plan-bound phase control. See the
[sustained role guide](docs/sustained-workloads.md) for current supported entry points,
evidence/lifetime contracts and integration status. Deployment and OS supervision
remain with the matrix orchestrator; physical transport behavior remains with bindings.
The generic native role supports `--zenoh-wait-for-subscriber` as an explicit binding
readiness setting; see the [sustained role guide](docs/sustained-workloads.md).

Fixed sensor roles reserve their exact encoded publication header before native
setup. They do not add the ordinary token/traceparent allowance to the approved
LiDAR/camera sample extent. See [Fixed sensor reservations](docs/role-frontends.md#fixed-sensor-reservations).

Fixed sensor sources also retain a real native fanout witness alongside the bridge reader. It borrows and releases each sample without forwarding or copying the payload; its observations and costs remain visible. See [Fixed sensor reservations](docs/role-frontends.md#fixed-sensor-reservations).

The shared native workload adapter admits and drains synchronous receiver callbacks independently of whether a middleware RX thread has entered an async runtime. See [Synchronous native callbacks](docs/role-frontends.md#synchronous-native-callbacks).

Native workload registrations reserve their application-admission worker at setup. A synchronous handoff uses the existing transient loan and waits for admission to finish before the next callback; decoding and evidence costs remain in whole-process measurements. See [Native application handoff](docs/role-frontends.md#native-application-handoff).

The `flow-evidence` build also supports the explicit `UPROTOCOL_CLOUDEVENTS_PROOF=protobuf-roundtrip-v1` interoperability control. It serializes actual messages as protobuf CloudEvents and converts them back at send/receive boundaries. See [CloudEvents conversion control](docs/role-frontends.md#cloudevents-conversion-control).
