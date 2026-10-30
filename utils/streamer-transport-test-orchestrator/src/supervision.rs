// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Shared Unix process-group supervision, extracted from matrix execution.
//!
//! This module uses POSIX signals and Rust child handles on Linux and macOS. It
//! does not inspect `/proc`, create namespaces, or manage middleware storage.
//!
//! Process-group cleanup and the leader's application exit status are separate
//! facts. [`RunningProcess::terminate`] only establishes cleanup; it also records
//! which supervisor signals preceded the leader's reaped status, so callers can
//! check [`RunningProcess::exit_evidence`] against an explicit [`ExitContract`].
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
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

/// Which leader exit statuses an owned process may end with.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitContract {
    /// Exit code zero, on its own or after supervisor signalling, or the
    /// documented response to a supervisor signal: termination by a signal the
    /// supervisor delivered before the exit, or exit code 128 plus that signal.
    OwnedRole,
    /// Negative-control peer that reports its own outcome: it must exit before
    /// any supervisor signal with an exit code (zero or not), never by a signal.
    SelfExitWithCode,
}

/// Leader exit-status outcome, independent of process-group disappearance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitClassification {
    /// Exit code zero.
    Success,
    /// Followed supervisor signalling with the documented status for a signal
    /// delivered before the exit: termination by it, or exit code 128 plus it.
    SupervisorTerminated,
    /// Any other nonzero exit code, before or after supervisor signalling.
    UnexpectedExit,
    /// Termination by a signal not delivered by the supervisor before the exit.
    UnexpectedSignal,
    /// The supervisor has not reaped the leader.
    NotObserved,
}

impl ExitClassification {
    /// Evidence name, identical to the serialized form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::SupervisorTerminated => "supervisor_terminated",
            Self::UnexpectedExit => "unexpected_exit",
            Self::UnexpectedSignal => "unexpected_signal",
            Self::NotObserved => "not_observed",
        }
    }
}

impl ExitContract {
    /// Evidence name, identical to the serialized form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnedRole => "owned_role",
            Self::SelfExitWithCode => "self_exit_with_code",
        }
    }
}

/// Classifies a reaped leader status against the supervisor signals that preceded it.
///
/// `delivered_before_exit` is empty when the leader exited on its own, before the
/// supervisor signalled its group; such an exit is expected only with code zero.
#[must_use]
pub fn classify_exit(status: ExitStatus, delivered_before_exit: &[i32]) -> ExitClassification {
    if status.success() {
        return ExitClassification::Success;
    }
    let delivered = |signal: i32| delivered_before_exit.contains(&signal);
    match (status.code(), status.signal()) {
        (_, Some(signal)) if delivered(signal) => ExitClassification::SupervisorTerminated,
        (_, Some(_)) => ExitClassification::UnexpectedSignal,
        (Some(code), None) if code > 128 && delivered(code - 128) => {
            ExitClassification::SupervisorTerminated
        }
        _ => ExitClassification::UnexpectedExit,
    }
}

/// Serializable leader exit evidence for one owned process.
#[derive(Clone, Debug, Serialize)]
pub struct ExitEvidence {
    /// Evidence schema version.
    pub schema: u8,
    /// Human-readable role name used in failures.
    pub role: String,
    /// Leader process identifier.
    pub pid: u32,
    /// Dedicated process group created for the leader.
    pub process_group: i32,
    /// Contract the classification was checked against.
    pub contract: ExitContract,
    /// Whether bounded cleanup observed the whole group gone (a separate fact).
    pub group_cleanup_complete: bool,
    /// Every signal the supervisor delivered to the group, in order.
    pub supervisor_signals: Vec<&'static str>,
    /// Supervisor signals delivered before the leader's status was reaped.
    pub signals_before_exit: Vec<&'static str>,
    /// `before_supervisor_signal`, `after_supervisor_signal` or `not_observed`.
    pub observed: &'static str,
    /// Platform rendering of the reaped status.
    pub status: Option<String>,
    /// Exit code for a normal exit.
    pub exit_code: Option<i32>,
    /// Terminating signal number.
    pub signal: Option<i32>,
    /// Whether the terminating signal produced a core dump.
    pub core_dumped: bool,
    /// Outcome independent of the contract.
    pub classification: ExitClassification,
    /// Whether the contract accepts this outcome.
    pub expected: bool,
}

impl ExitEvidence {
    /// Rejects an outcome the contract does not accept, naming status and timing.
    pub fn ensure_expected(&self, log_path: &Path) -> Result<()> {
        if self.expected {
            return Ok(());
        }
        let Some(status) = &self.status else {
            return Err(anyhow!(
                "process {} exit status was not observed by its supervisor; log={}",
                self.role,
                log_path.display()
            ));
        };
        let timing = if self.signals_before_exit.is_empty() {
            "before any supervisor signal".to_string()
        } else {
            format!("after supervisor {}", self.signals_before_exit.join("+"))
        };
        Err(anyhow!(
            "process {} exited with {status} {timing} ({} under {} exit contract); log={}",
            self.role,
            self.classification.as_str(),
            self.contract.as_str(),
            log_path.display()
        ))
    }
}

fn supervisor_signal_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGINT => "SIGINT",
        libc::SIGTERM => "SIGTERM",
        libc::SIGKILL => "SIGKILL",
        _ => "UNEXPECTED",
    }
}

/// An execution-owned child and its dedicated process group.
///
/// Explicitly call [`Self::terminate`] to propagate cleanup failures. Drop performs
/// the same bounded cleanup and retains failures in [`cleanup_summary`]. Signals are
/// sent only by cleanup, so a status reaped earlier through `child` preceded them.
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
    signals: Vec<i32>,
    exit: Option<(ExitStatus, usize)>,
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
            signals: Vec::new(),
            exit: None,
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
    /// Success means only that the group is gone; the leader's status is recorded,
    /// with the signals delivered before it, for [`Self::exit_evidence`].
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
            // Reap a pending leader first: an exit that preceded supervisor action
            // must never be attributed to the signal sent below.
            self.observe_exit()?;
            if !process_group_exists(self.process_group)? {
                self.terminated = true;
                return Ok(());
            }
            if signal_process_group(self.process_group, signal)? {
                self.signals.push(signal);
            }
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

    /// Reports the leader status and its timing relative to supervisor signals.
    ///
    /// Polls once without blocking. Call after [`Self::terminate`] so a status is
    /// available; group cleanup success is reported separately by that method.
    pub fn exit_evidence(&mut self, contract: ExitContract) -> Result<ExitEvidence> {
        self.observe_exit()?;
        let names = |signals: &[i32]| -> Vec<&'static str> {
            signals
                .iter()
                .copied()
                .map(supervisor_signal_name)
                .collect()
        };
        let (status, before, observed, classification) = match self.exit {
            Some((status, count)) => {
                let before = &self.signals[..count];
                let observed = if before.is_empty() {
                    "before_supervisor_signal"
                } else {
                    "after_supervisor_signal"
                };
                (
                    Some(status),
                    before,
                    observed,
                    classify_exit(status, before),
                )
            }
            None => (
                None,
                &[][..],
                "not_observed",
                ExitClassification::NotObserved,
            ),
        };
        let expected = match contract {
            ExitContract::OwnedRole => matches!(
                classification,
                ExitClassification::Success | ExitClassification::SupervisorTerminated
            ),
            ExitContract::SelfExitWithCode => {
                before.is_empty() && status.is_some_and(|status| status.code().is_some())
            }
        };
        Ok(ExitEvidence {
            schema: 1,
            role: self.name.clone(),
            pid: self.child.id(),
            process_group: self.process_group,
            contract,
            group_cleanup_complete: self.terminated,
            supervisor_signals: names(&self.signals),
            signals_before_exit: names(before),
            observed,
            status: status.map(|status| status.to_string()),
            exit_code: status.and_then(|status| status.code()),
            signal: status.and_then(|status| status.signal()),
            core_dumped: status.is_some_and(|status| status.core_dumped()),
            classification,
            expected,
        })
    }

    /// Polls the leader once, keeping the first status with its signal timing.
    fn observe_exit(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some((status, _)) = self.exit {
            return Ok(Some(status));
        }
        let status = self.child.try_wait()?;
        if let Some(status) = status {
            self.exit = Some((status, self.signals.len()));
        }
        Ok(status)
    }

    fn wait_for_group_exit(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            self.observe_exit()?;
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

/// Signals the owned group; `false` means the group had already disappeared.
fn signal_process_group(process_group: i32, signal: i32) -> Result<bool> {
    // SAFETY: this strictly positive group was created for our owned child. The
    // negative ID addresses only that group; kill retains no Rust memory.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error).with_context(|| format!("unable to signal process group {process_group}"))
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

    /// Waits for the leader to exit but leaves it unreaped, like a classic passive
    /// that the row never polls before teardown.
    fn exited_unreaped(process: &RunningProcess) {
        // SAFETY: zeroed siginfo is plain data filled by the kernel; WNOWAIT waits
        // only for our own child and leaves it reapable by `Child`.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                process.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        assert_eq!(result, 0, "{}", io::Error::last_os_error());
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
        // Escalation stays explicit: the leader died by the SIGKILL we delivered.
        let exit = process.exit_evidence(ExitContract::OwnedRole).unwrap();
        assert_eq!(exit.supervisor_signals, ["SIGINT", "SIGTERM", "SIGKILL"]);
        assert_eq!(exit.signals_before_exit, ["SIGINT", "SIGTERM", "SIGKILL"]);
        assert_eq!(exit.signal, Some(libc::SIGKILL));
        assert_eq!(
            exit.classification,
            ExitClassification::SupervisorTerminated
        );
        assert!(exit.expected);
    }

    #[test]
    fn rejects_nonpositive_group_identifiers() {
        assert!(process_group_exists(0).is_err());
        assert!(process_group_exists(-1).is_err());
    }

    #[test]
    fn exit_before_supervisor_signal_is_kept_separate_from_successful_cleanup() {
        let mut process = spawn("printf 'ready\\n'; exit 42");
        ready(&mut process);
        // An unreaped leader still holds its group, so cleanup must reap it before
        // signalling or the earlier exit would be attributed to supervisor action.
        exited_unreaped(&process);
        process.terminate().unwrap();
        let exit = process.exit_evidence(ExitContract::OwnedRole).unwrap();
        assert!(exit.group_cleanup_complete);
        assert!(exit.supervisor_signals.is_empty());
        assert_eq!(exit.observed, "before_supervisor_signal");
        assert_eq!(exit.exit_code, Some(42));
        assert_eq!(exit.classification, ExitClassification::UnexpectedExit);
        assert!(!exit.expected);
        let error = exit
            .ensure_expected(Path::new("role.log"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("exit status: 42 before any supervisor signal (unexpected_exit"),
            "{error}"
        );
        // Repeated polling cannot lose the cached reaped status.
        for _ in 0..2 {
            let status = process.child.try_wait().unwrap();
            assert_eq!(status.and_then(|status| status.code()), Some(42));
        }
        // A negative-control peer may report its failure with its own exit code.
        process
            .exit_evidence(ExitContract::SelfExitWithCode)
            .unwrap()
            .ensure_expected(Path::new("role.log"))
            .unwrap();
    }

    #[test]
    fn documented_responses_to_supervisor_signals_are_supervisor_terminated() {
        for (script, signals, code, signal) in [
            (
                "printf 'ready\\n'; exec sleep 30",
                vec!["SIGINT"],
                None,
                Some(2),
            ),
            (
                "trap 'exit 0' INT; printf 'ready\\n'; while :; do sleep 1; done",
                vec!["SIGINT"],
                Some(0),
                None,
            ),
            (
                "trap 'exit 130' INT; printf 'ready\\n'; while :; do sleep 1; done",
                vec!["SIGINT"],
                Some(130),
                None,
            ),
            (
                "trap '' INT; trap 'exit 143' TERM; printf 'ready\\n'; while :; do sleep 1; done",
                vec!["SIGINT", "SIGTERM"],
                Some(143),
                None,
            ),
        ] {
            let mut process = spawn(script);
            ready(&mut process);
            process.terminate().unwrap();
            let exit = process.exit_evidence(ExitContract::OwnedRole).unwrap();
            assert_eq!(exit.observed, "after_supervisor_signal", "{script}");
            assert_eq!(exit.signals_before_exit, signals, "{script}");
            assert_eq!((exit.exit_code, exit.signal), (code, signal), "{script}");
            assert!(exit.expected, "{script}: {exit:?}");
            exit.ensure_expected(Path::new("role.log")).unwrap();
            // A negative-control peer must report its own outcome instead.
            assert!(
                !process
                    .exit_evidence(ExitContract::SelfExitWithCode)
                    .unwrap()
                    .expected
            );
        }
    }

    #[test]
    fn failures_after_supervisor_signal_are_not_hidden_by_cleanup() {
        for (script, classification) in [
            (
                "trap 'exit 3' INT; printf 'ready\\n'; while :; do sleep 1; done",
                ExitClassification::UnexpectedExit,
            ),
            // A crash while handling the supervisor's SIGINT (SIGUSR1 avoids cores).
            (
                "trap 'kill -USR1 $$' INT; printf 'ready\\n'; while :; do sleep 1; done",
                ExitClassification::UnexpectedSignal,
            ),
        ] {
            let mut process = spawn(script);
            ready(&mut process);
            process.terminate().unwrap();
            let exit = process.exit_evidence(ExitContract::OwnedRole).unwrap();
            assert!(exit.group_cleanup_complete, "{script}");
            assert_eq!(exit.signals_before_exit, ["SIGINT"], "{script}");
            assert_eq!(exit.classification, classification, "{script}");
            let error = exit
                .ensure_expected(Path::new("role.log"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("after supervisor SIGINT"), "{error}");
        }
    }

    #[test]
    fn classification_requires_the_supervisor_to_have_sent_the_signal_first() {
        let signalled = |signal| ExitStatus::from_raw(signal);
        let exited = |code: i32| ExitStatus::from_raw(code << 8);
        let int = [libc::SIGINT];
        let escalated = [libc::SIGINT, libc::SIGTERM];
        use ExitClassification::*;
        for (status, delivered, expected) in [
            (exited(0), &[][..], Success),
            (exited(0), &int[..], Success),
            (exited(42), &[][..], UnexpectedExit),
            (exited(42), &int[..], UnexpectedExit),
            (exited(130), &[][..], UnexpectedExit),
            (exited(130), &int[..], SupervisorTerminated),
            (exited(143), &int[..], UnexpectedExit),
            (exited(143), &escalated[..], SupervisorTerminated),
            (signalled(libc::SIGINT), &[][..], UnexpectedSignal),
            (signalled(libc::SIGINT), &int[..], SupervisorTerminated),
            (signalled(libc::SIGTERM), &int[..], UnexpectedSignal),
            (signalled(libc::SIGKILL), &int[..], UnexpectedSignal),
            (signalled(libc::SIGSEGV), &escalated[..], UnexpectedSignal),
        ] {
            assert_eq!(
                classify_exit(status, delivered),
                expected,
                "{status} {delivered:?}"
            );
        }
    }

    #[test]
    fn failure_messages_use_the_serialized_evidence_names() {
        use ExitClassification::*;
        for value in [
            Success,
            SupervisorTerminated,
            UnexpectedExit,
            UnexpectedSignal,
            NotObserved,
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), value.as_str());
        }
        for value in [ExitContract::OwnedRole, ExitContract::SelfExitWithCode] {
            assert_eq!(serde_json::to_value(value).unwrap(), value.as_str());
        }
    }
}
