# Direct peer connectivity

`--direct-pairs --artifacts-root <directory>` executes same-profile peers without
starting a configurable Streamer. Its 180 candidates (twelve profiles, three roles,
five encodings) use separate `direct-*` identities and a `direct-summary.json`.
The ordinary 2160-row bridged universe and its identical-profile exclusions remain
unchanged. `--only <direct-id>` restricts the direct cohort.

Direct execution reuses native bundle/exporter generation, role entrypoints,
namespace/broker setup, readiness and owned process supervision. Peer deployments
share an actual physical bus: one iceoryx2 namespace, DDS domain, Zenoh session
connection, MQTT broker, LoLa primary/response service or SOME/IP routing network.
Native manifests name only the two peers. Each listener still owns its native
subscriber and receive lease; no payload copy or ownership shortcut is introduced.

Both peer processes start before the controller waits for listener registration.
Prepared receivers can therefore discover the service offered by their peer,
including the RPC response provider. First traffic remains blocked until the
passive listener and active first-send barrier are both ready; no warm-up message
is sent.

Raw Tx/Rx evidence must match payload bytes, encoding, presence, addressing and
bounded multiplicity; RPC responses must correlate with the requests at both peers.
Only the SOME/IP binding's explicit UUID reconstruction changes ID matching. The
output records actual failures and zero retries; absent bridge observations are
not synthesized. This lane checks finite first-delivery connectivity, not whole-
window losslessness or endurance. Backend capabilities/setup remain binding-owned.

Cleanup accounts for metadata inspection tools as well as namespaces, brokers and
role processes. `metadata_tool_processes` reports the inspection groups separately;
success still requires every started group to be checked and zero leaked groups.
