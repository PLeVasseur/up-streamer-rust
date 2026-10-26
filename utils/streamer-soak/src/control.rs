// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Generation-bound workload phases shared by controller and role workers.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

/// Runtime phase of a bounded sustained workload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Endpoints are becoming ready; producers have not been released.
    #[default]
    Waiting,
    /// Initial offered traffic before measured normal load.
    Warmup,
    /// Normal processing at the declared offered rate.
    Normal,
    /// Deliberately delayed consumer processing with bounded admission.
    Slow,
    /// Producers have stopped; accepted messages and responses are draining.
    Drain,
    /// Terminal stop, also permitted for cancellation before normal completion.
    Stop,
}

impl Phase {
    /// Whether source workers may offer a new message in this phase.
    #[must_use]
    pub const fn offers_messages(self) -> bool {
        matches!(self, Self::Warmup | Self::Normal | Self::Slow)
    }

    fn next(self) -> Option<Self> {
        match self {
            Self::Waiting => Some(Self::Warmup),
            Self::Warmup => Some(Self::Normal),
            Self::Normal => Some(Self::Slow),
            Self::Slow => Some(Self::Drain),
            Self::Drain => Some(Self::Stop),
            Self::Stop => None,
        }
    }
}

/// Controller command bound to the exact frozen workload plan.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseCommand {
    /// Expected plan digest shared by this cohort's workers.
    pub plan_sha256: String,
    /// Desired runtime phase; transport configuration is not part of this command.
    pub phase: Phase,
}

/// Worker-side phase transition validation, independent of the command's I/O mechanism.
pub struct PhaseTracker {
    plan_sha256: String,
    phase: Phase,
}

impl PhaseTracker {
    /// Creates a waiting tracker bound to one frozen plan digest.
    pub fn new(plan_sha256: &str) -> Result<Self> {
        ensure!(
            crate::artifact::is_digest(plan_sha256),
            "invalid control plan digest"
        );
        Ok(Self {
            plan_sha256: plan_sha256.into(),
            phase: Phase::Waiting,
        })
    }

    /// Returns the last successfully applied phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Applies the next phase, returning false when polling sees the same command again.
    ///
    /// Foreign plans, regressions and skipped workload windows fail without changing
    /// state. Stop is always permitted for cancellation, but it does not establish
    /// successful workload duration/accounting; the orchestrator verifies those separately.
    pub fn apply(&mut self, command: &PhaseCommand) -> Result<bool> {
        ensure!(
            command.plan_sha256 == self.plan_sha256,
            "control plan generation mismatch"
        );
        if command.phase == self.phase {
            return Ok(false);
        }
        ensure!(
            command.phase == Phase::Stop || self.phase.next() == Some(command.phase),
            "workload phase skipped or regressed"
        );
        self.phase = command.phase;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(phase: Phase) -> PhaseCommand {
        PhaseCommand {
            plan_sha256: "a".repeat(64),
            phase,
        }
    }

    #[test]
    fn phases_gate_offers_and_stop_is_terminal() {
        let mut tracker = PhaseTracker::new(&"a".repeat(64)).unwrap();
        assert!(!tracker.phase().offers_messages());
        for phase in [
            Phase::Warmup,
            Phase::Normal,
            Phase::Slow,
            Phase::Drain,
            Phase::Stop,
        ] {
            assert!(tracker.apply(&command(phase)).unwrap());
            assert!(!tracker.apply(&command(phase)).unwrap());
        }
        assert!(!tracker.phase().offers_messages());
        assert!(tracker.apply(&command(Phase::Warmup)).is_err());
    }

    #[test]
    fn foreign_stale_or_skipped_commands_preserve_current_state() {
        let mut tracker = PhaseTracker::new(&"a".repeat(64)).unwrap();
        tracker.apply(&command(Phase::Warmup)).unwrap();
        for invalid in [
            command(Phase::Waiting),
            command(Phase::Slow),
            PhaseCommand {
                plan_sha256: "b".repeat(64),
                phase: Phase::Stop,
            },
        ] {
            assert!(tracker.apply(&invalid).is_err());
            assert_eq!(tracker.phase(), Phase::Warmup);
        }
    }

    #[test]
    fn cancellation_can_stop_before_readiness_without_admitting_traffic() {
        let mut tracker = PhaseTracker::new(&"a".repeat(64)).unwrap();
        tracker.apply(&command(Phase::Stop)).unwrap();
        assert!(!tracker.phase().offers_messages());
        assert!(tracker.apply(&command(Phase::Normal)).is_err());
    }
}
