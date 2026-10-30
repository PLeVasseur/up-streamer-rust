# Shared execution and platform boundaries

## Shared orchestration

The matrix and sustained workloads should share row preparation, configured
Streamer/role commands, native deployment generation, immutable bundles, readiness,
process supervision and evidence. Transport configuration belongs at that existing
deployment boundary; each binding owns discovery, physical storage and transport
lifecycle. Workload control and journal replay do not manage middleware files.

`prepared` factors the existing row artifact/environment preparation into a shared
internal deployment edge. Its current profile uses the established Linux bundle
and binding inputs. Finite execution consumes that same prepared result; sustained
execution can reuse it without reconstructing native manifests or backend setup.

The crate's library currently exposes shared Unix process supervision and explicit
OS memory/isolation operations. The matrix executable uses that supervisor for
normal workers and namespace holders. It preserves bounded SIGINT, SIGTERM and
SIGKILL escalation and reports surviving groups, including descendants whose leader
has already exited. Cleanup reaps an exited leader before each signal and records
which supervisor signals preceded its status; `RunningProcess::exit_evidence`
reports that, separately from group cleanup, against an explicit `ExitContract`.
This uses POSIX APIs available on both Linux and macOS, without depending on
`/proc` or a virtual machine.

## Present execution support

Full matrix execution still requires the Linux namespace profile described in the
README. The OS check precedes native builds and process execution. Offline `--list`
and criteria generation do not require namespaces or private-mount path restrictions.
The shared library can be tested separately:

```sh
cargo test --locked -p streamer-transport-test-orchestrator --lib
```

Linux and macOS are the portability targets for the shared execution code. Native
macOS end-to-end matrix/soak qualification is pending. In particular, a direct Unix
child launch is not enough to qualify an ordinary-process deployment: its fixture
must preserve physical topology and avoid conflicting endpoint resources through
the existing binding configuration. The runner never silently substitutes that
deployment for requested Linux namespace isolation.

Backend availability is separate from runner portability. Consult each binding's
platform/setup documentation and qualify the actual backend build. Unsupported host
capabilities or unavailable backends must not become new structural unsupported
matrix rows or reduced full-matrix acceptance. Retain the complete canonical inventory.
Native inventories and deployment artifacts must come from actual target-built
exporters; a Linux inventory must not be relabelled as a macOS build.

## Measurement semantics

Filesystem capacity uses the shared Unix `statvfs` adapter, which normalizes the
different Linux/macOS counter widths and checks multiplication into byte extents.
Filesystems without inode quotas report that absence explicitly.

`platform::process_memory` samples a process through an OS-specific adapter:

- Linux: current RSS and proportional set size from `smaps_rollup`, converted to bytes.
- macOS: current resident size from `proc_pidinfo(PROC_PIDTASKINFO)`; an equivalent
  Linux PSS metric is explicitly unsupported by this adapter.

Each value records its native source. Unsupported values are distinct from measured
zero; permission failures, partial reads and process-exit races return errors.
Acceptance that requires a metric must reject an unavailable value. A platform-specific
profile must declare its metrics and limits before execution; RSS must not be silently
substituted for PSS. These process metrics do not claim to measure all middleware
backing-file allocation or to authorize deleting transport storage.

The legacy full-matrix host/cgroup monitor remains Linux-specific. Further shared
workload extraction will consume the platform adapter rather than extend those
Linux assumptions into generic accounting or replay. Native macOS execution must
test the macOS adapter; Linux tests alone do not establish macOS acceptance.
