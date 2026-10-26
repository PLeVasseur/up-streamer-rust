# Streamer flow observations

This integration-test utility captures metadata identity, payload presence, length
and a SHA-256 digest at send, receive and actual Streamer handoff boundaries.
Records are observational diagnostics, not a wire format or deployment agreement.

See [the observation contract](docs/observations.md) for environment controls,
capture behavior and the limits of the finite-run comparison. Public handwritten
APIs use `missing_docs = "deny"` and are checked by strict Rustdoc/doctests.
