
# CloudEvents matrix

`--cloud-events` runs the existing ordered 2,160-row matrix as an additional
interoperability pass: 1,980 relevant bridge combinations and the same 180
same-profile structural exclusions. It covers the six physical transports,
twelve endpoint profiles, three communication styles and native/Protobuf/XCDRv2/
Arrow/OMG-IDL payloads. The ordinary matrix remains a separate required run.
No existing queue, rate, loss, resource or zero-allocation criterion is relaxed.

Build role binaries with `flow-evidence`, then use the ordinary frozen matrix
invocation with `--cloud-events --skip-build` and a fresh artifacts root. The
controller explicitly passes the conversion mode into each real source and sink.
This mode is separate from sustained workloads, direct peers and SingleType
controls, which keep their own denominators and acceptance rules.

The shared endpoint helpers serialize actual uProtocol messages as protobuf
CloudEvents and decode them back. Opaque payload bytes and their original encoding
stay unchanged across real Streamer forwarding. This exercises the converter;
it does not select a new backend envelope. Native metadata requires the already
admitted deployment agreement and must compare equal after conversion.

Each row retains full serialized event bytes/hash, before/after observations,
actual operation correspondence and `cloudevents-verification.json`. The controller
independently decodes the recorded protobuf events again, compares their metadata
and payload hashes with actual TX/RX evidence, and requires the local legacy-data,
missing-format and opaque-presence controls. A missing record, mismatched event,
unsupported legacy representation or lost native identity fails the row.
CloudEvents serialization is an allocating application control; actual whole-process
costs are reported separately from the native zero-allocation acceptance lanes.

CloudEvents uses classic-message semantics: absent priority becomes CS1,
success status may become absent, and zero TTL becomes absent. The native
application adapter checks these explicit default equivalences plus every other
classic field and the admitted native token, then retains the original immutable
frame metadata witness. Native optional-field presence is preserved, with no
identity inferred from payload bytes. Raw event records include before/after
semantic attributes and native frame-witness records; payload presence, encoding
and bytes must remain exactly equal. A positive sub-millisecond native TTL or
other unrepresentable field still fails the existing checked projection.

## Mixed-backend connection readiness

Generated Zenoh clients use the existing ten-second role readiness window as
their bounded native connection-setup timeout. A client may therefore start
while a slower peer backend is still opening its configured listener. Successful
session establishment and listener registration remain required before release
of the source barrier. The client still fails when that window expires.

This is native setup within a single row attempt. It adds no stabilization sleep,
warm-up publication or row retry, and changes no traffic timeout, pacing, queue,
pool, loss or allocation criterion. The same setup applies with or without the
CloudEvents application interoperability control.

The opt-in CloudEvents frame adapter installs the already admitted worker
agreement at flow setup, before registering either an owned or prepared
receiver. Receive-only and RPC-server roles therefore validate their first
incoming native frame before any outbound message is built. Encoding lookup
does not initialize this context. This application control does not infer an
agreement from incoming bytes or change native transport admission.

CloudEvents operation coverage and raw-log hashes are checked after row process
teardown. Classic receivers can append middleware shutdown output after their
first observed message; the recorded hashes therefore bind the complete final
logs, including shutdown, rather than an earlier live prefix. Every conversion
and frame witness in those final logs remains subject to the same checks.
