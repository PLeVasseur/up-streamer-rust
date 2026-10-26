# Generated native deployments and selected-wire routing

Native senders and receivers load a deployment agreement before activation.
Table mode is the recommended default. Its private payload allocations are
deployment policy; a type's structural token is not its payload encoding ID.
Payload IDs and wire/family/layout declarations are separate namespaces within
one artifact. Private selected-wire identities require literal declarations.

## Export and provision

The `streamer-native-types` package defines the actual role and payload-flow
types and registers their concrete descriptors. Build and run
`streamer-native-inventory` for the deployment target, with `--expect-target`
matching that target. It rejects missing registrations or a different compiled
target rather than relabeling a host layout. Its optional `--agreement-output`
requires explicit `--domain`, `--seeds` and `--identities` files.

The owning generator's `up_native_profile::guide` documents artifact schemas,
successor ledgers, tombstones and its `manifest` command. The formats are
provisional and versioned. This deployment seeds F101/F102 for its two native
types and reserves F001/F002/F003 for its serialized codecs. These are coordinated
inputs, not constants assigned by the type definitions.

Provision an independent `DeploymentManifest` with expected full agreement hash,
target, domain and generation for **every endpoint** and complete route membership.
Distribute the named artifacts beneath an explicit profile root. Direct application
pairs also need two independently declared members; substituting the local member
for its peer is rejected. Files are loaded once and the checked generation is
retained for in-flight operations.

## Streamer endpoint configuration

Each native endpoint supplies:

```json
{
  "native_deployment": {
    "profile_root": "profiles",
    "manifest": "deployment.json",
    "route": "vehicle-route",
    "local_endpoint": "streamer-ingress",
    "peer_endpoint": "publisher"
  }
}
```

The root is relative to the application configuration directory or absolute.
The manifest is relative to that root or absolute; its artifact locators must
stay beneath the root, including after symlink resolution. Startup validates
all supplied endpoint expectations and complete route agreement before opening
endpoints. Unknown members, incompatible generations, changed hashes, invalid
descriptors and undeclared private wire identities fail admission.

Role processes use all five environment variables:

- `UPROTOCOL_NATIVE_PROFILE_ROOT`
- `UPROTOCOL_NATIVE_DEPLOYMENT_MANIFEST`
- `UPROTOCOL_NATIVE_ROUTE`
- `UPROTOCOL_NATIVE_ENDPOINT`
- `UPROTOCOL_NATIVE_PEER_ENDPOINT`

Their typed constructors also check the actual linked representation against the
active allocation. An absent set permits non-native opaque operation; a partial
set is an error. `UPROTOCOL_NATIVE_PROFILE`, `UPROTOCOL_NATIVE_PEER_PROFILE`,
`native_profile_file` and `native_peer_profile_file` are retired and rejected.
There is no native table-to-ID0 fallback. SingleType artifacts must be generated
explicitly for one deployment-scoped representation, without an operation label.
The tool uses `single-type`; artifact mode is `single_type` and canonical tag is3.
Old `contract_defined` artifacts are rejected by name and must be regenerated.

## Complete-route policy

Each native `forwarding_routes` entry also lists `native_hops`, with exactly one
entry per member in the provisioned manifest route:

```json
{
  "endpoint": "egress",
  "wire_format": "up_native",
  "native_hops": [
    { "endpoint": "publisher", "api": "classic" },
    { "endpoint": "streamer-ingress", "api": "owned_frame" },
    { "endpoint": "streamer-egress", "api": "native_loan" },
    { "endpoint": "subscriber", "api": "native_loan" }
  ],
   "allow_single_type_over_classic_hops": false
}
```

Hop names are deployment member IDs, while the forwarding `endpoint` is the
configured local target. Classes describe transport APIs, not native payloads:
`classic`, `owned_frame`, `native_loan`. Loaded local bindings must match their
declared class and full artifact identity. Missing, duplicate or unknown members
fail before transports open; a remote classic hop cannot be omitted just because
the local Streamer boundary has two frame APIs.

Table mode is the default and works across supported classes with normal checks.
SingleType across **any** classic member requires explicitly setting
`allow_single_type_over_classic_hops: true` on that forwarding route. The
reverse route has its own flag. Opt-in acknowledges lost identity evidence; it
does not restore that evidence or bypass profiles, types, layout or filters.
Successful activation emits `NATIVE_ROUTE_ACTIVATED` with the complete classes and
flag, once per activated route, separately from per-message verification.

The [core route policy guide](../../up-streamer/docs/native_routes.md) covers the
library APIs and equal-sized DoorState/SeatState example. The legacy `forwarding`
string list cannot express a complete native policy; use `forwarding_routes` for
native endpoints. Non-native opaque routes need no native deployment arguments.
For a one-way classic-to-frame route, the classic ingress's `wire_format` also
configures the frame egress adapter, even if that endpoint has no outgoing routes.
Any route touching a frame API must declare the wire explicitly; pure classic
routes may keep their existing format-free configuration.

## Bundle and startup evidence

The matrix orchestrator builds and stages the actual application exporter with
its other executables, then invokes it in the new run bundle. Bundle schema5
records the command, exporter hash, target-qualified inventory, seed/declaration
inputs, genesis predecessor state, generated agreement and raw file hashes.
`native/` retains all four JSON inputs/outputs plus stdout/stderr; they become
read-only with the bundle. Admission never generates a replacement peer table.

Each native row provisions source, Streamer ingress, Streamer egress and sink
membership. `NATIVE_DEPLOYMENT_LOADED` records the checked manifest, agreement,
inventory, target, domain, generation and local/peer IDs before traffic. The
orchestrator requires the correct records from both workers and both Streamer
endpoints. `NATIVE_IDENTITY_VERIFIED` remains the separate per-message check;
`NATIVE_PROJECTION_VERIFIED` records classic/frame identity transitions.

## DDS encoded-owned routing

DDS has three distinct configured families: `owned` (classic), `owned_frame`
(validated owned), and `encoded_owned` (selected-wire encoded-owned). Enable
`dds-encoded-owned` for the latter, supply `wire_format` on its routes, and use
`--route-family encoded-owned` in DDS role binaries. The selected-wire adapter
uses the owned routing API, including bridges to classic and native-loan endpoints.
Native payload encoding on this family does not confer a native loan capability.

The retired `dds-zero-copy` feature and DDS `copy_minimized` mode are unsupported;
the role CLI also rejects `--payload-alignment`. Non-DDS endpoints cannot select
the DDS-specific `encoded_owned` mode. Backend storage, topic layout and lifecycle
contracts belong to the [DDS binding](https://github.com/PLeVasseur/up-transport-dds-rust#readme).
# Native pool configuration

For a Zenoh copy-minimized endpoint, optional `zenoh_shm_segment_size` passes a byte
budget to `ZenohZeroCopyCoreBuilder::with_shm_segment_size`. Omission preserves the
binding default and zero is rejected by that builder. Streamer composes the configured
endpoint; the binding owns pool allocation, readiness and loan lifetime behavior.
The sustained-workload fixture supplies an explicit pool budget appropriate for its
declared bounded messages. Normal matrix endpoint generation retains binding defaults.
## Native receive capacity

The optional `iceoryx2_namespace.subscriber_buffer_size` and
`iceoryx2_namespace.max_borrowed_samples` values delegate receive buffering and
retained-sample limits to the binding's runtime configuration. Positive values are
required. These limits are distinct from the application's bounded inbox and from
the separately configured publisher-readiness and unregister-quiescence contracts.
## Native Zenoh publisher readiness

Optional `zenoh_wait_for_subscriber` delegates actual matching-subscriber readiness
to the native Zenoh binding's public builder with a five-second deadline. Omission
or false retains publication without waiting. The sustained fixture requires an
actual match. Local listener registration alone is not treated as proof that a
remote publisher has observed discovery. Pool capacity and quiescence are separate.
