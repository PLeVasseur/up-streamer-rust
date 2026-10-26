// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Explicit OS capability and measurement boundaries; no transport-specific policy.
use serde::{Deserialize, Serialize};
use std::io;

/// Host OS relevant to validation execution capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostOs {
    /// Linux, where namespace execution may be available after a permission probe.
    Linux,
    /// macOS, with ordinary Unix process supervision and native process measurements.
    MacOs,
    /// An OS whose execution support has not been implemented here.
    Other,
}

impl HostOs {
    /// Identifies the OS this executable was built for, without probing permissions.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }

    /// Checks that the OS provides Linux namespace APIs, before executable probes.
    ///
    /// Success does not establish permissions, required commands or backend support.
    pub fn require_linux_namespaces(self) -> io::Result<()> {
        if self == Self::Linux {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::Unsupported,
            "this execution profile requires Linux namespaces; offline inventory and shared Unix supervision do not"))
        }
    }
}

/// Native API supplying a process-memory value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySource {
    /// Linux kernel `/proc/<pid>/smaps_rollup`, converted from KiB to bytes.
    LinuxSmapsRollup,
    /// macOS `proc_pidinfo(PROC_PIDTASKINFO)` resident-size field in bytes.
    MacOsProcTaskInfo,
}

/// A byte measurement or an explicit lack of an equivalent OS metric.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ByteMeasurement {
    /// A successfully sampled value with its native source.
    Measured {
        /// Sampled bytes; zero is an actual observation, never a missing-value sentinel.
        bytes: u64,
        /// OS API defining the measurement's semantics.
        source: MemorySource,
    },
    /// This adapter has no equivalent metric; it must not satisfy a required-value gate.
    Unsupported {
        /// Human-readable explanation of the unavailable metric.
        reason: String,
    },
}

impl ByteMeasurement {
    /// Returns a measured value or an unsupported error for a required acceptance metric.
    pub fn require_bytes(&self) -> io::Result<u64> {
        match self {
            Self::Measured { bytes, .. } => Ok(*bytes),
            Self::Unsupported { reason } => {
                Err(io::Error::new(io::ErrorKind::Unsupported, reason.clone()))
            }
        }
    }
}

/// Process-memory observations; RSS and proportional memory are distinct metrics.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessMemory {
    /// Current resident memory, including the native measurement source.
    pub resident: ByteMeasurement,
    /// Proportional memory, or explicitly unsupported where no equivalent is provided.
    pub proportional: ByteMeasurement,
}

/// Filesystem capacity normalized from the host's native `statvfs` field widths.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FilesystemCapacity {
    /// Allocation unit in bytes used to convert block counts.
    pub block_size: u64,
    /// Total filesystem byte capacity.
    pub total_bytes: u64,
    /// Byte capacity currently available to an unprivileged caller.
    pub available_bytes: u64,
    /// Total inode count, absent when the filesystem does not report inode quotas.
    pub total_inodes: Option<u64>,
    /// Available inode count, absent when inode quotas are not reported.
    pub available_inodes: Option<u64>,
}

/// Reads filesystem capacity using Unix `statvfs`, without parsing platform shell output.
///
/// Counter widths differ between Linux and macOS. Conversion to bytes is checked;
/// invalid paths, OS failures and overflowing extents remain errors, not capacities.
#[cfg(unix)]
pub fn filesystem_capacity(path: &std::path::Path) -> io::Result<FilesystemCapacity> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and the output buffer has the exact native
    // type/alignment. statvfs retains neither pointer; initialization requires success.
    if unsafe { libc::statvfs(path.as_ptr(), status.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful statvfs initialized the native output structure above.
    let status = unsafe { status.assume_init() };
    #[allow(
        clippy::useless_conversion,
        reason = "statvfs counters are u32 on macOS and u64 on Linux"
    )]
    let (block_size, blocks, available, files, available_files) = (
        u64::from(if status.f_frsize == 0 {
            status.f_bsize
        } else {
            status.f_frsize
        }),
        u64::from(status.f_blocks),
        u64::from(status.f_bavail),
        u64::from(status.f_files),
        u64::from(status.f_favail),
    );
    let bytes = |count: u64| {
        count.checked_mul(block_size).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "filesystem byte extent overflow",
            )
        })
    };
    Ok(FilesystemCapacity {
        block_size,
        total_bytes: bytes(blocks)?,
        available_bytes: bytes(available)?,
        total_inodes: (files > 0).then_some(files),
        available_inodes: (files > 0).then_some(available_files),
    })
}

/// Samples one positive process ID using the OS-specific implementation.
///
/// Process exit races and permission errors propagate. An owning supervisor may
/// subsequently confirm that its child exited; this function never fabricates zeros.
pub fn process_memory(pid: u32) -> io::Result<ProcessMemory> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "positive process ID required",
        ));
    }
    sample_memory(pid)
}

/// One live member of an execution-owned process tree, measured by the OS adapter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessSample {
    /// OS process identifier observed during this sample.
    pub pid: u32,
    /// Actual process group used for endpoint/launcher ownership accounting.
    #[serde(default)]
    pub process_group: i32,
    /// Linux start-time ticks distinguishing reused PID identities; zero in old records.
    #[serde(default)]
    pub start_time_ticks: u64,
    /// Current resident bytes.
    pub rss: u64,
    /// Proportional resident bytes.
    pub pss: u64,
    /// Proportional shared-memory resident bytes; not backing-file allocation.
    pub shared_pss: u64,
    /// Open file descriptors.
    pub fds: u64,
    /// Live OS threads.
    pub threads: u64,
}

/// Samples the given root and all of its currently visible descendants.
///
/// Currently provided by Linux `/proc`. Other OS adapters return unsupported,
/// never a partial tree or zeroed measurements. Callers retain their own child
/// handles and separately verify process-group cleanup after execution.
#[cfg(target_os = "linux")]
pub fn process_tree_samples(root: u32) -> io::Result<Vec<ProcessSample>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        let mut fields = fields.split_whitespace();
        if fields.next() == Some("Z") {
            continue;
        }
        if let Some(parent) = fields.next().and_then(|p| p.parse::<u32>().ok()) {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    let mut samples = Vec::new();
    while let Some(pid) = pending.pop() {
        if !seen.insert(pid) {
            continue;
        }
        pending.extend(children.get(&pid).into_iter().flatten().copied());
        let sample = (|| -> io::Result<ProcessSample> {
            let before_stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let identity = |stat: &str| -> io::Result<(i32, u64)> {
                let fields: Vec<_> = stat
                    .rsplit_once(')')
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing proc comm"))?
                    .1
                    .split_whitespace()
                    .collect();
                let group = fields.get(2).and_then(|s| s.parse().ok());
                let start = fields.get(19).and_then(|s| s.parse().ok());
                group.zip(start).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid process identity")
                })
            };
            let (process_group, start_time_ticks) = identity(&before_stat)?;
            let smaps = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))?;
            let memory = parse_smaps(&smaps)?;
            let shared = smaps
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Pss_Shmem:")
                        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                })
                .and_then(|n| n.checked_mul(1024))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "missing shared-memory PSS")
                })?;
            let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
            let threads = status
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Threads:")
                        .and_then(|v| v.trim().parse().ok())
                })
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "missing thread count")
                })?;
            let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))?
                .collect::<io::Result<Vec<_>>>()?
                .len() as u64;
            if identity(&std::fs::read_to_string(format!("/proc/{pid}/stat"))?)?
                != (process_group, start_time_ticks)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "process identity changed during sample",
                ));
            }
            Ok(ProcessSample {
                pid,
                process_group,
                start_time_ticks,
                rss: memory.resident.require_bytes()?,
                pss: memory.proportional.require_bytes()?,
                shared_pss: shared,
                fds,
                threads,
            })
        })();
        match sample {
            Ok(sample) => samples.push(sample),
            Err(error) => {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                let gone = matches!(&stat, Err(e) if e.kind() == io::ErrorKind::NotFound)
                    || stat
                        .as_ref()
                        .ok()
                        .and_then(|s| s.rsplit_once(')'))
                        .and_then(|(_, s)| s.split_whitespace().next())
                        == Some("Z");
                if !gone {
                    return Err(error);
                }
            }
        }
    }
    Ok(samples)
}

/// Returns an explicit unsupported result until this OS has a complete process-tree adapter.
#[cfg(not(target_os = "linux"))]
pub fn process_tree_samples(_: u32) -> io::Result<Vec<ProcessSample>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "complete process-tree sampling is not implemented for this OS",
    ))
}

#[cfg(target_os = "linux")]
fn sample_memory(pid: u32) -> io::Result<ProcessMemory> {
    parse_smaps(&std::fs::read_to_string(format!(
        "/proc/{pid}/smaps_rollup"
    ))?)
}

#[cfg(any(target_os = "linux", test))]
fn parse_smaps(text: &str) -> io::Result<ProcessMemory> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete or invalid smaps measurement",
        )
    };
    let mut rss = None;
    let mut pss = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let field = match name {
            "Rss" => &mut rss,
            "Pss" => &mut pss,
            _ => continue,
        };
        let mut parts = value.split_whitespace();
        let bytes = parts
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .and_then(|n| n.checked_mul(1024))
            .ok_or_else(invalid)?;
        if field.is_some() || parts.next() != Some("kB") || parts.next().is_some() {
            return Err(invalid());
        }
        *field = Some(bytes);
    }
    let measured = |bytes| ByteMeasurement::Measured {
        bytes,
        source: MemorySource::LinuxSmapsRollup,
    };
    Ok(ProcessMemory {
        resident: measured(rss.ok_or_else(invalid)?),
        proportional: measured(pss.ok_or_else(invalid)?),
    })
}

#[cfg(target_os = "macos")]
fn sample_memory(pid: u32) -> io::Result<ProcessMemory> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = i32::try_from(std::mem::size_of::<libc::proc_taskinfo>()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "native process info size overflow",
        )
    })?;
    // SAFETY: a correctly sized/aligned writable buffer is provided; proc_pidinfo
    // retains no pointer. Only a full-size successful result permits initialization.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written <= 0 {
        return Err(io::Error::last_os_error());
    }
    if written != size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "partial native process info",
        ));
    }
    // SAFETY: the successful full-size call above initialized every field.
    let info = unsafe { info.assume_init() };
    Ok(ProcessMemory {
        resident: ByteMeasurement::Measured {
            bytes: info.pti_resident_size,
            source: MemorySource::MacOsProcTaskInfo,
        },
        proportional: ByteMeasurement::Unsupported {
            reason: "macOS process task info does not provide Linux proportional set size".into(),
        },
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_memory(_: u32) -> io::Result<ProcessMemory> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process-memory adapter is not implemented for this OS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[cfg(unix)]
    #[test]
    fn reports_native_filesystem_capacity_without_shell_tools() {
        let value = filesystem_capacity(&std::env::temp_dir()).unwrap();
        assert!(value.block_size > 0 && value.total_bytes > 0);
        assert!(value.available_bytes <= value.total_bytes);
        assert_eq!(
            value.total_inodes.is_some(),
            value.available_inodes.is_some()
        );
        assert!(filesystem_capacity(std::path::Path::new("/invalid\0path")).is_err());
    }

    #[test]
    fn unsupported_metrics_cannot_satisfy_required_values() {
        let value = ByteMeasurement::Unsupported {
            reason: "no equivalent metric".into(),
        };
        let round_trip: ByteMeasurement =
            serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
        assert_eq!(
            round_trip.require_bytes().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            ByteMeasurement::Measured {
                bytes: 0,
                source: MemorySource::LinuxSmapsRollup
            }
            .require_bytes()
            .unwrap(),
            0
        );
    }

    #[test]
    fn unavailable_namespaces_are_not_a_host_process_fallback() {
        assert_eq!(
            HostOs::MacOs.require_linux_namespaces().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[test_case("Rss: 1 kB\n"; "missing_proportional")]
    #[test_case("Rss: 1 kB\nRss: 2 kB\nPss: 1 kB\n"; "duplicate_resident")]
    #[test_case("Rss: 18446744073709551615 kB\nPss: 1 kB\n"; "extent_overflow")]
    #[test_case("Rss: 1 bytes\nPss: 1 kB\n"; "wrong_units")]
    fn rejects_incomplete_or_ambiguous_kernel_measurements(value: &str) {
        assert_eq!(
            parse_smaps(value).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn converts_kernel_units_without_conflating_metrics() {
        let sample = parse_smaps("Rss: 12 kB\nPss: 7 kB\n").unwrap();
        assert_eq!(sample.resident.require_bytes().unwrap(), 12 * 1024);
        assert_eq!(sample.proportional.require_bytes().unwrap(), 7 * 1024);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn samples_the_actual_current_process() {
        let sample = process_memory(std::process::id()).unwrap();
        assert!(sample.resident.require_bytes().unwrap() > 0);
        if cfg!(target_os = "macos") {
            assert!(sample.proportional.require_bytes().is_err());
        } else {
            assert!(sample.proportional.require_bytes().unwrap() > 0);
        }
        assert!(process_memory(0).is_err());
    }
}
