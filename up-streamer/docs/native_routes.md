# Complete-route native identity policy

Native route agreement includes the complete profile, not only matching type
sizes or short tokens. `NativeRoutePolicy` describes every declared member with
its loaded profile and transport/API class: classic `UTransport`, owned-frame,
or native-loan. A native payload on a classic API remains a **classic hop**.
An encoded-owned core remains an owned-frame API even when carrying native bytes.

Use Table mode by default. It carries a deployment-assigned encoding as well as
the native frame's structural token. ContractDefined explicitly binds ID0 to one
operation and representation. Any classic hop in the complete declaration makes
ContractDefined fail unless `allow_contract_defined_over_classic_hops` is enabled
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
ContractDefined profiles by default. Successful policy-aware activation emits
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
operation contract), provision separate endpoint expectations, and create the
complete deployment manifest. Its deployment guide owns the artifact commands
and schemas. Check the distributed result with:

```sh
cargo run -p up-streamer --example native_route_identity -- check \
  --profile-root profiles --manifest profiles/deployment.json \
  --route vehicle-route --local sender --peer receiver
```

The example declares owned-frame APIs by default. Repeat `--classic-hop MEMBER`
for declared classic members. A ContractDefined profile then requires the
explicit `--allow-contract-defined-over-classic-hops` flag. The output distinguishes
policy checking from actual Streamer route activation; it does not claim traffic
was forwarded. These example types are separate from the matrix's role inventory.
