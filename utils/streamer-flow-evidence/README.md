# Streamer flow observations

This integration-test utility captures metadata identity, payload presence, length
and a SHA-256 digest at send, receive and actual Streamer handoff boundaries.
Records are observational diagnostics, not a wire format or deployment agreement.

See [the observation contract](docs/observations.md) for environment controls,
capture behavior and the limits of the finite-run comparison. Public handwritten
APIs use `missing_docs = "deny"` and are checked by strict Rustdoc/doctests.


Prepared native routes use the same observation schema through checked borrowed
headers. See [Prepared native handoffs](docs/observations.md#prepared-native-handoffs)
for the successful API boundary, payload-presence checks and observer cost scope.

Owned/classic-to-native adapters report the same successful-handoff schema.

`PayloadCopyObservation` records actual initialized egress-loan payload bytes
before put, including copies followed by failed publication. See
[Actual copied payloads](docs/observations.md#actual-copied-payloads) for precise
presence, endpoint correlation and measurement scope.
