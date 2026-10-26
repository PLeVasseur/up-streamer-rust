# Native deployment API migration

Use SDK `NativeProfileMode::SingleType(NativeSingleType)` for one exact native
representation selected by ID zero. This is deployment-scoped and has no URI or
operation binding. The old unused operation label is removed. Table remains the
recommended default for multiple representations.

Regenerate `contract_defined` artifacts with the SDK-owned `single-type` command;
the mode is now `single_type` with canonical tag3. Unsupported old artifacts fail
by name. Table canonical bytes and descriptor/token derivation version2 remain
unchanged. No native table lookup falls back to SingleType.

Rename route configuration `allow_contract_defined_over_classic_hops` to
`allow_single_type_over_classic_hops`, and the corresponding Rust method and CLI
flag to `with_allow_single_type_over_classic_hops` and
`--allow-single-type-over-classic-hops`. The default staysfalse. Unknown old keys
are rejected; there is no alias. Complete-route validation still sees remote
classic hops, and opt-in never bypasses type/profile/layout/filter checks.

Use `StablePayload::REVISION` for application type revision and the checked SDK
projection APIs. Metadata layout identity belongs to the configured metadata
codec: construct context from both wire and codec. Fixed serialized wires need
no native deployment context.
