# Direct peer connectivity

`--direct-pairs --artifacts-root <directory>` executes same-profile peers without
starting a configurable Streamer. Its180 candidates (twelve profiles, three roles,
five encodings) use separate `direct-*` identities and a `direct-summary.json`.
The ordinary2160-row bridged universe and its identical-profile exclusions remain
unchanged. `--only <direct-id>` restricts the direct cohort.

Direct execution reuses native bundle/exporter generation, role entrypoints,
namespace/broker setup, readiness and owned process supervision. Peer deployments
share an actual physical bus: one iceoryx2 namespace, DDS domain, Zenoh session
connection, MQTT broker, LoLa primary/response service or SOME/IP routing network.
Native manifests name only the two peers. Each listener still owns its native
subscriber and receive lease; no payload copy or ownership shortcut is introduced.

Raw Tx/Rx evidence must match payload bytes, encoding, presence, addressing and
bounded multiplicity; RPC responses must correlate with the requests at both peers.
Only the SOME/IP binding's explicit UUID reconstruction changes ID matching. The
output records actual failures and zero retries; absent bridge observations are
not synthesized. This lane checks finite first-delivery connectivity, not whole-
window losslessness or endurance. Backend capabilities/setup remain binding-owned.
