# Shared prepared endpoint setup

`PreparedAddress` is shared semantic setup, not a second native backend framework.
It declares exact source/sink URI text, maximum metadata/payload bytes, payload
alignment, token/traceparent capacity and whether this endpoint owns TX. The
binding constructors keep native geometry, mapping and ownership responsibilities.

The helpers open one creation-reserved core and share its `Arc` across configured
wire adapters. `configure` reserves each transmitting address through the core's
finite route table and declares all exact receive addresses, including receiver-only
entries. A wire-specific route helper cannot implicitly discover or grow native
resources after traffic starts. Unknown IDs remain opaque during forwarding;
typed receive still requires an independently loaded native agreement.

Configuration defaults are 3072 metadata bytes, eight-byte payload alignment,
2048 token bytes and traceparent enabled. Payload size and exact source are explicit;
sink omission means publication. `transmit: false` creates an ingress-only logical
entry. Route/registration table capacities are independent of native RX queues,
borrowed credits, publisher loans and physical slots. Invalid or overflowing
logical bounds fail before native setup.
Checked-in application templates and generated deployments supply these catalogs
explicitly. A native-encoding template also needs its independent deployment
agreement; reserving addresses and extents never declares a payload type.

`iceoryx2_core` resolves the supplied namespace or the binding's existing namespace
environment before applying explicit capacity choices. It derives the default
fixed complete sample extent from the largest validated catalog entry. Explicit
slice limits remain limits: too-small extents and dynamic allocation are rejected
by the prepared binding. This setup does not enlarge RX queues, borrowed credits,
subscriber ceilings or live TX loans. Configured native frontends create the
prepared core directly; they do not also open an unused owned transport.

The configurable frontend uses `prepared_routes` per native endpoint and optional
`prepared_route_capacity`/`prepared_registration_capacity` (defaults 128/100).
These values reserve logical tables rather than enlarge the binding's native
acceptance queues. See the library's [prepared routing guide](../../../up-streamer/docs/prepared-routing.md)
for receiver workers, observer/copy accounting and teardown. See binding guides
for their native installation, geometry and lifecycle contracts.

`PreparedSession` shares finite sender reservation and exact header admission
between role frontends. Its synchronous `send` copies already serialized bytes
into the reserved native loan; `admit` retains a native frame without owning
metadata strings. `register` composes one generic SDK header worker with the
physical subscription. Typed native send/receive continues to use SDK templates
and `RxAdmission` over that same configured core.

`decode_compatibility_frame` explicitly allocates owned metadata for compatibility
validation and output. Its `PreparedReceived` view retains the original native
frame and borrows its payload; it does not manufacture native backing or a typed
agreement. This diagnostic conversion is separate from in-place admission and
the typed native allocation path. All receiver registration happens before a
role reports READY or publishes a request needing a correlated response.

## Receiver-only catalogs

Shared setup preserves each declared address's transmit flag when reserving
native resources. Iceoryx receive-only entries create a service without a
publisher/data segment; Zenoh receive-only catalogs borrow the remote SHM pool
without an unused local provider. The bindings still own static physical sample
sizes, pool geometry and all subscriber/borrow/history/loan limits. TX addresses
retain their eager setup reservation; no hot-path lazy publisher is introduced.

## Receiver-first attachment

The Iceoryx binding owns each reserved native service for the core lifetime,
including receive-only addresses. Shared setup can attach a receiver before its
peer publisher exists, without adding a local transmit arena. The eventual
publisher owns its original configured payload pool. Connection readiness still
follows the binding's explicit setup checks before traffic is released.

## Bounded LoLa peer discovery

The LoLa binding completes native proxy discovery during prepared attachment.
A peer may finish offering its reserved transmitting events after attachment
starts, including both directions of an RPC deployment. The binding uses its
bounded setup discovery interval; this does not extend role/scenario deadlines,
retry traffic or change catalog geometry, borrowing credit or native pools.
Cancellation and expiry release the uncommitted registration slot and partial
proxies. Retained payload samples preserve their native owners through detach.
