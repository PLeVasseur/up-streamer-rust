# Representative pool page evidence

New representative plans set `require_pool_page_census: true`. After the complete
warmup and before normal traffic, the existing Rust controller captures owned
source, bridge and sink process mappings from Linux `/proc/<pid>/smaps`. Each
snapshot retains PID/start-time/group identity, its byte hash and full raw kernel
fields. Failure or a missing measurement fails the lane; zeros are not substituted.

`pool-page-census.json` (schema `uprotocol.pool-page-census.v2`) binds these snapshots to the workload plan. Replay reparses
the raw files and checks mapped extents, device/inode/offset identities, configured
capacities and complete physical backing residency of the source arena and bridge
provider. A hash-bound native util-linux `fincore` executable queries actual
mincore page residency through a descriptor held by the controller; raw output,
argv, descriptor identity, file extent and measured native page size are retained.
Missing or inconsistent page/byte counts fail before normal traffic. Replay
rehashes and reparses these observations without rerunning native measurements.
Native backing must correspond across source/bridge/sink. The generator delegates
eager setup only to the existing bridge provider through `zenoh_shm_prewarm`;
receive-only workers do not allocate an additional transmit provider.

Mapped bytes, RSS, proportional locked bytes and a locked VMA are different
measurements. A remote alias can have fewer populated page-table entries even when
the owning mapping proves every page of the shared object is resident. Shared
locked-byte accounting may be proportional; it is not required to equal the VMA
extent. The census retains both raw values and flags, avoiding double-counting
aliases as independent pools. It measures live OS state without manipulating
memory or adding a transport setup path. Physical setup and storage behavior remain
binding-owned; see the [native capacity guide](../../../configurable-streamer/docs/native-capacity.md).

Earlier plans that omit the new requirement retain their declared evidence scope.
Their fixture hashes and working-set measurements do not become a whole-pool
census retroactively. Calibration and acceptance must use matching source,
executables, provider configuration and this new declared requirement.
