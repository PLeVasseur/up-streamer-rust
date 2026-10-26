# Explicit endpoint composition

`RouteWireFormat` parses configured wire names and emits canonical spellings.
Endpoint construction selects that wire explicitly; it does not inspect incoming
payload bytes to guess a decoder. Native construction requires a loaded
`NativeProfileAgreement` and retains the profile on the core endpoint.

`RouteWireEndpoint` retains the concrete wire-specific native-loan family and its
supported transport variants. `RouteOwnedEndpoint` instead carries a validated
owned-frame endpoint with the chosen wire. DDS encoded-owned construction uses the
owned-frame adapter and does not become a native loan merely because the selected
payload representation is native.

Route helpers require the declared wire to match both participating frame endpoints
before delegating to the core `UStreamer` API. Classic/frame helpers validate the
frame-side wire and preserve the default rejection of SingleType across a
classic hop. The `_with_native_policy` variants accept a complete route-scoped policy;
an explicit override still cannot bypass profile, type, layout or final filter checks.

See the [deployment guide](../../../configurable-streamer/docs/native-deployment.md)
for artifact locators, complete membership, startup identity checks and whole-route
configuration. Backend storage, installation and native lifetimes belong to each binding.
