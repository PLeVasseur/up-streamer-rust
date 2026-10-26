# Observation and finite-leg comparison contract

Set `UPROTOCOL_FLOW_EVIDENCE_RUN` and `UPROTOCOL_FLOW_EVIDENCE_ACTOR` to label a
run and the emitting process/role. Capture is opt-in: with no run identity, helpers
return `None` without hashing payloads. An enabled capture requires an actor.
`Observation::emit` writes `FLOW_MESSAGE_EVIDENCE ` followed by one JSON record.

The record contains schema version 1, run/actor/stage/optional route, message and
request UUIDs, kind, source/sink, encoding and payload presence. Its length and
SHA-256 cover the concatenation of all visible payload slices. Capture checks
overflow and exact slice-length agreement. Absent and present-empty payloads stay
distinct through the presence and encoding fields even though both hash zero bytes.
Classic messages are projected through SDK metadata validation; frame and send-loan
helpers observe their existing views without taking ownership of native storage.

`equivalent` compares the recorded run, kind, endpoints, encoding, presence, length
and digest. Its explicit `preserve_ids` argument also requires matching UUIDs when
true. Turning it off is reserved for a binding's declared UUID-reconstruction
boundary; bridge-internal comparisons always preserve IDs.

`validate_leg` requires matching actual ingress and egress handoffs, verifies route
association inside the bridge, and rejects forwarding or delivery multiplicities
that exceed their source observations. It does not claim that a finite receiver
received every send, and its record does not cover every SDK metadata field.
Whole-message and operational accounting use their additional owning checks.


## Prepared native handoffs

`Observation::header` hashes an admitted borrowed header's opaque payload without
forming a typed payload reference. It preserves absent versus present-empty
payloads and rejects contradictory header/carrier presence when enabled.
Prepared Streamer routes emit ingress before forwarding and egress only after a
successful prepared API handoff. The egress observation describes the borrowed
view submitted to that handoff; actual copied-loan inspection and copied-then-
failed byte accounting remain the separate prepared observer/statistics APIs.
Formatting and hashing allocations are observer costs, included in process
measurements. With evidence disabled, no observation is constructed.

Owned/classic-to-native Streamer adapters capture their submitted owned frame
before forwarding and emit egress only on success, using this same schema.

## Actual copied payloads

`PayloadCopyObservation` captures the synchronously borrowed initialized native
egress payload before publication. Its run, message ID and configured egress
endpoint correlate the record with handoff evidence; it does not name an incoming
physical route. It records payload presence, actual extent, one copy for a present
extent, and SHA-256. Absent payloads record no copy; present-empty payloads record
one zero-byte copy. Failed publication cannot remove a prior copy record. Streamer
emits a separate completion result, including failure. Hashing and formatting
costs remain in the observed process. This does not assert metadata-byte hashes
or create typed payload references.
