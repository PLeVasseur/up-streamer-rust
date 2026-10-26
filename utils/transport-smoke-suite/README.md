# Deterministic transport smoke suite

This crate runs finite classic and native-loan transport smoke scenarios with
file-backed claims, readiness markers, owned process groups and durable reports.
It is separate from the full Cartesian transport-profile matrix orchestrator.

See [the execution and evidence guide](docs/execution.md) for configuration,
claim evaluation and cleanup semantics. Public handwritten APIs and command-line
targets use `missing_docs = "deny"` with strict Rustdoc/doctest checks.
