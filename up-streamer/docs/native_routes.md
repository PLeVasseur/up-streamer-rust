# Complete-route native identity policy

Native route agreement includes the complete profile, not only matching type
sizes or short tokens. `NativeRoutePolicy` describes every declared member with
its loaded profile and transport/API class: classic `UTransport`, owned-frame,
or native-loan. A native payload on a classic API remains a **classic hop**.
An encoded-owned core remains an owned-frame API even when carrying native bytes.

Use Table mode by default. It carries a deployment-assigned encoding as well as
the native frame's structural token. SingleType explicitly binds ID 0 to one
deployment-scoped representation, without URI/operation binding. Any classic hop
in the complete declaration makes SingleType fail unless `allow_single_type_over_classic_hops` is enabled
for that route. This includes remote classic members outside the immediate local
boundary. The override acknowledges lost identity evidence; it does not restore
that evidence or bypass agreement, type, size, alignment, field validity or filters.

Create a `NativeRoutePolicy` from the entire provisioned route, then call
`validate_boundary` against actual ingress/egress names, classes and profiles.
Local names must match the Streamer endpoint names. Native-aware configuration
frontends must compare declared membership with their loaded deployment manifest
and validate **before** opening transports. This is deployment agreement, not a
network handshake or remote attestation.

The five classic route APIs have `_with_native_policy` variants. Supply a complete
policy there for an explicit override; ordinary classic APIs reject native
SingleType profiles by default. Successful policy-aware activation emits
`NATIVE_ROUTE_ACTIVATED` with route, all member classes and the explicit flag.
Nonclassic segments use the same complete-policy validation before their ordinary
route API and record activation after it succeeds. There is no process-global flag
and the override is not installed on reusable endpoints.

See `examples/native_route_identity.rs` for the DoorState/SeatState scenario.
Equal-sized layouts remain different application representations. Their distinct
Table allocations prevent interpreting one type as the other, including across
classic projection. Explicit classic-hop opt-in does not make those types equal.

Export the example's actual target descriptors with:

```sh
cargo run -p up-streamer --example native_route_identity -- export --output inventory.json
```

Use the `up-native-profile` tool to generate a Table (or an explicit DoorState
single-type agreement), provision separate endpoint expectations, and create the
complete deployment manifest. Its deployment guide owns the artifact commands
and schemas. Check the distributed result with:

```sh
cargo run -p up-streamer --example native_route_identity -- check \
  --profile-root profiles --manifest profiles/deployment.json \
  --route vehicle-route --local sender --peer receiver
```

The example declares owned-frame APIs by default. Repeat `--classic-hop MEMBER`
for declared classic members. A SingleType profile then requires the
explicit `--allow-single-type-over-classic-hops` flag. The output distinguishes
policy checking from actual Streamer route activation; it does not claim traffic
was forwarded. These example types are separate from the matrix's role inventory.
# Payload movement and native ownership

Native ingress leases borrow their binding's receive storage. Native subscriber
fan-out and checked typed borrowing do not imply cross-backend storage adoption.
Current copy-minimized forwarding loans an egress buffer and copies the full payload
once into that owner's pool. It does not provide end-to-end zero-copy across
iceoryx2 and Zenoh. Source sensor workers also initialize a heap fixture before
copying into their TX loan; ownership bookkeeping and setup allocation are distinct.

With `flow-evidence` and an explicit run/actor environment, each actual bridge
payload copy emits `FLOW_PAYLOAD_COPY` containing message/route identity, copied
byte count and slice count. It is emitted after the copy, even if the subsequent
egress send fails. No record is emitted for a failed pre-copy loan; those failures
remain visible in route diagnostics. The observation never hashes or copies payload
data itself and is not a wire format or an allocation-free logging claim.

Successful checked classic/frame projection emits `NATIVE_PROJECTION_VERIFIED` at INFO level with the route, full agreement identity, encoding and actual token addition or removal. Rejected projections emit no verified record. Opaque carriage outside the agreed native table remains opaque and does not gain a native token. This diagnostic does not claim typed payload validation or allocation-free logging.
