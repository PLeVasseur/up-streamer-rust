# Configurable Streamer wire support

Public APIs use `missing_docs = "deny"` with strict Rustdoc/doctests. The
[endpoint composition guide](docs/endpoints.md) describes the typed route families
and the common wire/policy validation boundary.

This package composes the configured selected-wire adapters and owns the
Streamer's generated native-deployment startup integration. The deployment
artifact model and generator live in `up-native-profile`; the actual registered
application types live in `streamer-native-types`.

See the [native deployment and routing guide](../../configurable-streamer/docs/native-deployment.md)
for complete endpoint membership, locators, immutable generations, startup checks
and migration. Native typed operations require a matching compiled descriptor;
opaque routing does not manufacture a local representation or peer agreement.

`dds-encoded-owned` exposes an owned selected-wire adapter. The native-loan
endpoint families remain capability-specific. Every route uses an explicit wire;
no payload sniffing or automatic native ID 0 fallback is provided.

Policy-aware classic bridge helpers forward a complete `NativeRoutePolicy` to the
core route API. The ordinary helpers retain the rejecting default for native
SingleType/classic combinations. The configuration frontend checks complete
manifest membership before constructing these helpers.
# Capacity composition

`capacity::Iceoryx2Capacity` composes optional publisher slice/allocation, subscriber
and TX-loan limits through the owning binding API, retaining the explicit namespace.
Omitted fields preserve defaults; invalid inputs return typed errors. See the
[native capacity guide](../../configurable-streamer/docs/native-capacity.md) for
configuration and ownership boundaries. Physical allocation and service checks
remain binding-owned.

`prepared::PreparedAddress` supplies a shared explicit source/sink and bounded
frame catalog to bridge and role setup. Native constructors consume that catalog,
and wire-specific endpoint helpers reserve exact egress routes before admission.
They use the SDK's prepared core and generic receive workers. See the
[prepared endpoint guide](docs/prepared.md) for logical capacities and configuration.
`PreparedSession` supplies shared setup-owned sending/in-place admission for role
frontends. The iceoryx2 constructor derives a fixed catalog-sized default sample
extent and honors explicit capacity overrides after resolving the namespace. See
[prepared setup](docs/prepared.md) for capacity and readiness boundaries.

The session's explicitly allocating compatibility view retains native payload
storage separately from the typed native allocation path.

Disabled-default `publisher_prewarm` delegates exact/static publisher backing setup
through the same capacity object; it requires explicit static allocation. It does
not alter queue/slot limits. The [native capacity guide](../../configurable-streamer/docs/native-capacity.md)
describes configuration, CLI ownership and binding-owned physical behavior.
# Sensor workload types

With `payload-contract-large-fixtures`, `sensor::SensorFixture` selects the actual
SDK LiDAR/camera representations and their checked initializer/semantic validator.
It exposes compiled payload lengths; metadata/alignment overhead remains separate.
See [sustained workloads](../../example-streamer-uses/docs/sustained-workloads.md).

Prepared setup carries each declared TX/RX direction into native reservation. Receive-only Iceoryx and Zenoh endpoints reserve no unused TX payload pool. See [Receiver-only catalogs](docs/prepared.md#receiver-only-catalogs).

Receiver-only Iceoryx setup retains its native service through core lifetime and supports attachment before a peer publisher starts. See [Receiver-first attachment](docs/prepared.md#receiver-first-attachment).

LoLa prepared attachment completes bounded peer discovery before traffic, including concurrent RPC setup. See [Bounded LoLa peer discovery](docs/prepared.md#bounded-lola-peer-discovery).
