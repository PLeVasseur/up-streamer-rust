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

//! Deterministic transport smoke scenarios with file-backed claims and owned process cleanup.
/// File-backed log claims, thresholds and evaluated outcomes.
pub mod claims;
/// Runtime defaults, repository discovery and native environment lookup.
pub mod env;
/// Exact readiness-marker polling and bounded diagnostic log excerpts.
pub mod logs;
/// Child process groups, execution metadata and bounded shutdown escalation.
pub mod process;
/// Serialized scenario/matrix reports and human-readable summaries.
pub mod report;
/// Scenario definitions, command-line controls and execution entry points.
pub mod scenario;
