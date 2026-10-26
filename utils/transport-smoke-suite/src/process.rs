/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

#[derive(Debug)]
/// Captured output and exit status of a completed shell command.
pub struct CommandOutcome {
    /// Actual shell command, including optional environment bootstrap.
    pub command: String,
    /// Numeric exit code, or `None` if termination did not produce one.
    pub status_code: Option<i32>,
    /// Standard output decoded with replacement for invalid UTF-8.
    pub stdout: String,
    /// Standard error decoded with replacement for invalid UTF-8.
    pub stderr: String,
}

#[derive(Debug, Clone)]
/// Launch parameters for one execution-owned scenario process.
pub struct ProcessSpec {
    /// Human-readable process/role label.
    pub name: String,
    /// Child working directory.
    pub workdir: PathBuf,
    /// Executable to invoke after optional bootstrap.
    pub executable: PathBuf,
    /// Individual executable arguments, escaped when rendering the shell command.
    pub args: Vec<String>,
    /// Environment names and values exported for the child.
    pub env: Vec<(String, String)>,
    /// Combined output log name under the scenario artifact directory.
    pub log_file_name: String,
}

#[derive(Debug)]
/// Owned child handle and process-group metadata used for explicit shutdown and reporting.
pub struct ManagedProcess {
    /// Human-readable process/role label.
    pub name: String,
    /// PID returned when the child was spawned.
    pub pid: i32,
    /// Dedicated process-group identifier established for this child.
    pub process_group_id: i32,
    /// Rendered shell invocation used to launch the child.
    pub command_line: String,
    /// Working directory used by the child.
    pub workdir: PathBuf,
    /// Combined standard-output/error log path.
    pub log_path: PathBuf,
    /// UTC timestamp recorded after spawning.
    pub started_at: DateTime<Utc>,
    /// UTC timestamp when this handle observed process completion.
    pub ended_at: Option<DateTime<Utc>>,
    /// Observed numeric exit code, absent before observation or on signal termination.
    pub exit_status_code: Option<i32>,
    child: Child,
}

/// Runs a shell command with optional native-environment bootstrap and captures its completed output.
pub async fn run_shell_command(
    repo_root: &Path,
    workdir: &Path,
    command: &str,
    no_bootstrap: bool,
) -> Result<CommandOutcome> {
    let full_command = with_optional_bootstrap(repo_root, command, no_bootstrap);
    let output = Command::new("bash")
        .arg("-lc")
        .arg(&full_command)
        .current_dir(workdir)
        .output()
        .await
        .with_context(|| format!("failed to execute shell command: {full_command}"))?;

    Ok(CommandOutcome {
        command: full_command,
        status_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Quotes one literal shell word, including embedded single quotes.
pub fn shell_escape(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

impl ManagedProcess {
    /// Starts a child in its own process group, appending both output streams to its log.
    pub async fn spawn(
        spec: ProcessSpec,
        repo_root: &Path,
        artifacts_dir: &Path,
        no_bootstrap: bool,
    ) -> Result<Self> {
        let log_path = artifacts_dir.join(&spec.log_file_name);
        let stdout_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("unable to open log file {}", log_path.display()))?;
        let stderr_file = stdout_file
            .try_clone()
            .with_context(|| format!("unable to clone log file {}", log_path.display()))?;

        let exec_command = render_exec_command(&spec.executable, &spec.args);
        let env_prefix = render_env_prefix(&spec.env);
        let shell_body = if env_prefix.is_empty() {
            exec_command
        } else {
            format!("{env_prefix} && {exec_command}")
        };
        let full_command = with_optional_bootstrap(repo_root, &shell_body, no_bootstrap);

        let mut command = Command::new("bash");
        command
            .arg("-lc")
            .arg(&full_command)
            .current_dir(&spec.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file));

        // SAFETY: the post-fork closure calls only async-signal-safe setpgid for
        // the current child; error construction reads errno without user callbacks.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = command.spawn().with_context(|| {
            format!(
                "unable to spawn process '{}' with command '{}': in {}",
                spec.name,
                full_command,
                spec.workdir.display()
            )
        })?;

        let pid = child
            .id()
            .ok_or_else(|| anyhow!("spawned process '{}' has no pid", spec.name))?
            as i32;

        Ok(Self {
            name: spec.name,
            pid,
            process_group_id: pid,
            command_line: full_command,
            workdir: spec.workdir,
            log_path,
            started_at: Utc::now(),
            ended_at: None,
            exit_status_code: None,
            child,
        })
    }

    /// Reports whether this handle has observed completion; it does not poll the child.
    pub fn has_exited(&self) -> bool {
        self.ended_at.is_some()
    }

    /// Polls the child without waiting and records completion when observed.
    pub async fn refresh_exit_status(&mut self) -> Result<()> {
        if self.exit_status_code.is_some() {
            return Ok(());
        }

        if let Some(status) = self.child.try_wait().with_context(|| {
            format!(
                "unable to query process status for '{}'(pid={})",
                self.name, self.pid
            )
        })? {
            self.exit_status_code = status.code();
            self.ended_at = Some(Utc::now());
        }

        Ok(())
    }

    /// Waits up to the supplied interval, returning false on timeout without terminating the child.
    pub async fn wait_with_timeout(&mut self, timeout: Duration) -> Result<bool> {
        self.refresh_exit_status().await?;
        if self.exit_status_code.is_some() {
            return Ok(true);
        }

        let wait_result = tokio::time::timeout(timeout, self.child.wait()).await;
        match wait_result {
            Ok(wait_outcome) => {
                let status = wait_outcome.with_context(|| {
                    format!(
                        "unable to wait on process '{}'(pid={})",
                        self.name, self.pid
                    )
                })?;
                self.exit_status_code = status.code();
                self.ended_at = Some(Utc::now());
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Signals the owned group with SIGINT, then SIGTERM after the first grace period.
    ///
    /// Returns an error if both grace periods expire; the caller must handle any
    /// remaining process rather than treating that result as successful cleanup.
    pub async fn terminate_gracefully(
        &mut self,
        sigint_grace: Duration,
        sigterm_grace: Duration,
    ) -> Result<()> {
        self.refresh_exit_status().await?;
        if self.exit_status_code.is_some() {
            return Ok(());
        }

        self.signal_process_group(libc::SIGINT)?;
        if self.wait_with_timeout(sigint_grace).await? {
            return Ok(());
        }

        self.signal_process_group(libc::SIGTERM)?;
        if self.wait_with_timeout(sigterm_grace).await? {
            return Ok(());
        }

        self.refresh_exit_status().await?;
        if self.exit_status_code.is_none() {
            return Err(anyhow!(
                "process '{}' (pid={}, pgid={}) did not terminate after SIGINT/SIGTERM grace windows",
                self.name,
                self.pid,
                self.process_group_id
            ));
        }

        Ok(())
    }

    fn signal_process_group(&self, signal: i32) -> Result<()> {
        let target_pgid = -self.process_group_id;
        // SAFETY: kill accepts scalar process-group/signal identifiers and retains
        // no Rust memory; this identifier belongs to the group created at spawn.
        let rc = unsafe { libc::kill(target_pgid, signal) };
        if rc == 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }

        Err(anyhow!(
            "unable to send signal {} to process group {}: {}",
            signal,
            self.process_group_id,
            error
        ))
    }
}

fn render_env_prefix(environment: &[(String, String)]) -> String {
    environment
        .iter()
        .map(|(key, value)| format!("export {key}={}", shell_escape(value)))
        .collect::<Vec<String>>()
        .join(" && ")
}

fn render_exec_command(executable: &Path, args: &[String]) -> String {
    let mut tokens = Vec::with_capacity(args.len() + 1);
    tokens.push(shell_escape(executable.to_string_lossy().as_ref()));
    for arg in args {
        tokens.push(shell_escape(arg));
    }
    format!("exec {}", tokens.join(" "))
}

fn with_optional_bootstrap(repo_root: &Path, command: &str, no_bootstrap: bool) -> String {
    if no_bootstrap {
        return command.to_string();
    }

    let bootstrap_script = repo_root.join("build").join("envsetup.sh");
    format!(
        "source {} highest >/dev/null 2>&1 && {command}",
        shell_escape(bootstrap_script.to_string_lossy().as_ref())
    )
}
