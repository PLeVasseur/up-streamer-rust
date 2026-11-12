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

//! Public sync-health metadata for subscription refresh attempts.

use std::time::SystemTime;

/// Outcome history of subscription snapshot refreshes, independent of subscription contents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubscriptionSyncHealth {
    /// Completion time of the most recent refresh attempt, if one has finished.
    pub last_attempt_at: Option<SystemTime>,
    /// Completion time of the most recent successful refresh, if any.
    pub last_success_at: Option<SystemTime>,
    /// Whether the most recent completed refresh succeeded.
    pub last_attempt_succeeded: Option<bool>,
    /// Outcome of the completed refresh immediately preceding the latest one.
    pub previous_attempt_succeeded: Option<bool>,
}
