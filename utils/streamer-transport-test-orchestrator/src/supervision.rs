// SPDX-License-Identifier: Apache-2.0
//! Shared Unix process-group supervision, extracted from matrix execution.
//!
//! This module uses POSIX signals and Rust child handles on Linux and macOS. It
//! does not inspect `/proc`, create namespaces, or manage middleware storage.
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{io, thread};

static STARTED: AtomicU64 = AtomicU64::new(0);
static LEAK_CHECKS: AtomicU64 = AtomicU64::new(0);
static LEAKS: AtomicU64 = AtomicU64::new(0);

/// Process-supervision counters for one executable's validation run.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CleanupSummary {
    /// Successfully spawned process-group leaders.
    pub process_groups_started: u64,
    /// Explicit or drop-time group cleanup attempts.
    pub process_group_leak_checks: u64,
    /// Attempts that still observed a group after bounded signal escalation.
    pub process_group_leaks: u64,
}

/// Reads cumulative supervision counters, including drop-time cleanup failures.
#[must_use]
pub fn cleanup_summary() -> CleanupSummary {
    CleanupSummary {
        process_groups_started: STARTED.load(Ordering::SeqCst),
        process_group_leak_checks: LEAK_CHECKS.load(Ordering::SeqCst),
        process_group_leaks: LEAKS.load(Ordering::SeqCst),
    }
}

/// Resets counters at run entry, before starting any concurrent validation workers.
pub fn reset_cleanup_counters() {
    STARTED.store(0, Ordering::SeqCst);
    LEAK_CHECKS.store(0, Ordering::SeqCst);
    LEAKS.store(0, Ordering::SeqCst);
}

/// An execution-owned child and its dedicated process group.
///
/// Explicitly call [`Self::terminate`] to propagate cleanup failures. Drop performs
/// the same bounded cleanup and retains failures in [`cleanup_summary`].
#[derive(Debug)]
pub struct RunningProcess {
    /// Human-readable role name used in failures.
    pub name: String,
    /// Path to the caller-configured process output log.
    pub log_path: PathBuf,
    /// Owned child for polling, waiting and capturing its configured output.
    ///
    /// Do not replace this handle: its process group is bound at creation.
    pub child: Child,
    process_group: i32,
    terminated: bool,
}

impl RunningProcess {
    /// Spawns the supplied command in a fresh group, preserving its I/O and environment.
    ///
    /// The caller owns executable selection, deployment configuration and any
    /// requested isolation wrapper. This method starts no shell or namespace itself.
    pub fn spawn(
        command: &mut Command,
        name: impl Into<String>,
        log_path: PathBuf,
    ) -> Result<Self> {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        let name = name.into();
        let child = command
            .spawn()
            .with_context(|| format!("unable to spawn {name}"))?;
        let process_group =
            i32::try_from(child.id()).context("child PID exceeds process-group range")?;
        STARTED.fetch_add(1, Ordering::SeqCst);
        Ok(Self {
            name,
            log_path,
            child,
            process_group,
            terminated: false,
        })
    }

    /// Returns the dedicated group identifier created for this child.
    #[must_use]
    pub fn process_group(&self) -> i32 {
        self.process_group
    }

    /// Terminates all members of the owned group and reaps its leader.
    ///
    /// Preserves the matrix's SIGINT/2s, SIGTERM/1s, SIGKILL/2s policy. A leader
    /// exiting early does not count as successful cleanup while descendants remain.
    pub fn terminate(&mut self) -> Result<()> {
        if self.terminated {
            return Ok(());
        }
        LEAK_CHECKS.fetch_add(1, Ordering::SeqCst);
        for (signal, grace) in [
            (libc::SIGINT, Duration::from_secs(2)),
            (libc::SIGTERM, Duration::from_secs(1)),
            (libc::SIGKILL, Duration::from_secs(2)),
        ] {
            if !process_group_exists(self.process_group)? {
                self.child.try_wait()?;
                self.terminated = true;
                return Ok(());
            }
            signal_process_group(self.process_group, signal)?;
            if self.wait_for_group_exit(grace)? {
                self.terminated = true;
                return Ok(());
            }
        }
        LEAKS.fetch_add(1, Ordering::SeqCst);
        Err(anyhow!(
            "process group {} for {} still has descendants after SIGKILL; log={}",
            self.process_group,
            self.name,
            self.log_path.display()
        ))
    }

    fn wait_for_group_exit(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            self.child.try_wait()?;
            if !process_group_exists(self.process_group)? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for RunningProcess {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

fn signal_process_group(process_group: i32, signal: i32) -> Result<()> {
    // SAFETY: this strictly positive group was created for our owned child. The
    // negative ID addresses only that group; kill retains no Rust memory.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
            .with_context(|| format!("unable to signal process group {process_group}"))
    }
}

/// Queries whether a positive process-group identifier still has members.
///
/// Permission denial means the group exists. Other unexpected OS errors propagate.
/// This does not assert that a caller owns the group or authorize signalling it.
pub fn process_group_exists(process_group: i32) -> Result<bool> {
    anyhow::ensure!(process_group > 0, "process group must be positive");
    // SAFETY: signal zero only queries the validated scalar group ID.
    let result = unsafe { libc::kill(-process_group, 0) };
    if result == 0 {
        return Ok(true);
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(io::Error::last_os_error())
            .with_context(|| format!("unable to inspect process group {process_group}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    fn spawn(script: &str) -> RunningProcess {
        RunningProcess::spawn(
            Command::new("sh")
                .args(["-c", script])
                .stdout(Stdio::piped()),
            "supervision-test",
            PathBuf::from("captured-stdout"),
        )
        .unwrap()
    }

    fn ready(process: &mut RunningProcess) {
        let mut line = String::new();
        BufReader::new(process.child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "ready\n");
    }

    #[test]
    fn reaps_group_with_descendants_without_proc_filesystem() {
        let mut process = spawn(
            "sleep 30 & child=$!; trap 'wait \"$child\"; exit 0' INT TERM; printf 'ready\\n'; wait",
        );
        ready(&mut process);
        assert!(process_group_exists(process.process_group()).unwrap());
        process.terminate().unwrap();
        assert!(process.child.try_wait().unwrap().is_some());
        assert!(!process_group_exists(process.process_group()).unwrap());
        process.terminate().unwrap();
    }

    #[test]
    fn escalates_for_process_ignoring_interrupt_and_termination() {
        let mut process = spawn("trap '' INT TERM; printf 'ready\\n'; while :; do :; done");
        ready(&mut process);
        let started = Instant::now();
        process.terminate().unwrap();
        assert!(started.elapsed() < Duration::from_secs(6));
        assert!(!process_group_exists(process.process_group()).unwrap());
    }

    #[test]
    fn rejects_nonpositive_group_identifiers() {
        assert!(process_group_exists(0).is_err());
        assert!(process_group_exists(-1).is_err());
    }
}
