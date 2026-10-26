// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! OS-independent bounded workload journals and complete message reconciliation.
//!
//! The owning matrix orchestrator supplies deployment, workers and process/resource
//! supervision. This library handles evidence only: no transport construction,
//! middleware files, namespaces, process control or OS-specific memory assumptions.
#![doc = include_str!("../docs/workload.md")]
mod artifact;
mod config;
pub mod control;
pub mod inbox;
mod journal;
mod oracle;

use anyhow::{ensure, Result};
pub use config::Role;
use std::path::Path;

/// Expected execution identity and explicitly bounded reconciliation settings.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayOptions {
    /// Exact execution identity attached to every message observation.
    pub run: String,
    /// SHA-256 of the caller's source/build qualification artifact.
    pub source_identity: String,
    /// SHA-256 of the frozen workload plan.
    pub plan_sha256: String,
    /// Role-specific publication or request/response accounting contract.
    pub role: Role,
    /// Maximum application receive queue occupancy.
    pub queue_capacity: u64,
    /// Maximum simultaneously unreconciled message identities.
    pub pending_limit: usize,
    /// Require FIFO receipt order when the deployment guarantees it; otherwise count bounded reordering.
    #[serde(default = "default_fifo")]
    pub require_fifo: bool,
}

fn default_fifo() -> bool {
    true
}

impl ReplayOptions {
    fn context(&self) -> Result<journal::Context> {
        ensure!(
            !self.run.is_empty()
                && self.run.len() <= 128
                && artifact::is_digest(&self.source_identity)
                && artifact::is_digest(&self.plan_sha256),
            "invalid execution identity"
        );
        ensure!(
            self.queue_capacity > 0 && (1..=65_536).contains(&self.pending_limit),
            "invalid reconciliation bounds"
        );
        Ok(journal::Context {
            run: self.run.clone(),
            source_identity: self.source_identity.clone(),
            plan_sha256: self.plan_sha256.clone(),
        })
    }
}

/// A generation-bound record emitted by a [`JournalWriter`].
pub struct JournalRecord(journal::Record);

/// Full hash-chained compressed journals with bounded records and chunks.
pub struct JournalWriter(journal::Journal);

impl JournalWriter {
    /// Creates a writer in an existing artifact directory without overwriting chunks.
    ///
    /// The chunk bound must be between 64 KiB and 4 MiB. Every record is bounded to 64 KiB.
    pub fn new(directory: &Path, options: &ReplayOptions, chunk_bytes: usize) -> Result<Self> {
        ensure!(
            (artifact::MAX_LINE..=4 * 1024 * 1024).contains(&chunk_bytes),
            "invalid chunk bound"
        );
        Ok(Self(journal::Journal::new(
            directory,
            options.context()?,
            chunk_bytes,
        )?))
    }

    /// Appends one complete worker observation and returns its generation-bound record.
    ///
    /// Feed the returned record into [`Reconciler::observe`] for online checking.
    pub fn append(
        &mut self,
        actor: &str,
        kind: &str,
        value: serde_json::Value,
    ) -> Result<JournalRecord> {
        Ok(JournalRecord(self.0.append(
            actor.into(),
            kind.into(),
            value,
        )?))
    }

    /// Flushes/fsyncs the final chunk and writes the manifest exactly once.
    ///
    /// A failed or interrupted run must use `complete = false`; replay rejects it.
    pub fn finish(self, complete: bool) -> Result<()> {
        self.0.close(complete)?;
        Ok(())
    }
}

/// Constant-bound live message reconciliation, also used during independent replay.
pub struct Reconciler {
    context: journal::Context,
    oracle: oracle::Oracle,
}

impl Reconciler {
    /// Creates an empty oracle bound to the expected run/source/plan and role.
    pub fn new(options: &ReplayOptions) -> Result<Self> {
        Ok(Self {
            context: options.context()?,
            oracle: oracle::Oracle::with_order(
                options.role,
                options.run.clone(),
                options.queue_capacity,
                options.pending_limit,
                options.require_fifo,
            ),
        })
    }

    /// Consumes one record, rejecting foreign generations, corruption or invalid accounting.
    pub fn observe(&mut self, record: JournalRecord) -> Result<()> {
        ensure!(
            record.0.context == self.context,
            "record generation mismatch"
        );
        self.oracle.observe(record.0)
    }

    /// Returns the number of currently unreconciled message identities.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.oracle.pending()
    }

    /// Requires complete lifecycles, drained queues/loans and exact message disposition.
    ///
    /// Returns per-phase counts and measured accounting high-water marks. The owning
    /// workload additionally checks its duration, offered rate and loss budget.
    pub fn finish(self) -> Result<serde_json::Value> {
        Ok(serde_json::to_value(self.oracle.finish()?)?)
    }
}

/// Replays every retained chunk independently of live transports and host OS metrics.
///
/// The caller supplies expected identity/bounds from the frozen workload inputs;
/// replay does not trust a journal to choose its own generation or acceptance limits.
pub fn replay(directory: &Path, options: &ReplayOptions) -> Result<serde_json::Value> {
    let mut reconciler = Reconciler::new(options)?;
    journal::replay(directory, &options.context()?, |record| {
        reconciler.observe(JournalRecord(record))
    })?;
    reconciler.finish()
}
