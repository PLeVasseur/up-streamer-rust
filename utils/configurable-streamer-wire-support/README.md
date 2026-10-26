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
no payload sniffing or automatic native ID0 fallback is provided.

Policy-aware classic bridge helpers forward a complete `NativeRoutePolicy` to the
core route API. The ordinary helpers retain the rejecting default for native
ContractDefined/classic combinations. The configuration frontend checks complete
manifest membership before constructing these helpers.
