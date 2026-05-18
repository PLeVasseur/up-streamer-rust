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

/// Health snapshot for uSubscription refresh attempts.
///
/// A newly constructed streamer starts with all fields set to `None`. Each call
/// to [`UStreamer::refresh_subscriptions`](crate::UStreamer::refresh_subscriptions)
/// updates `last_attempt_at` and shifts the previous success value before
/// recording the latest outcome.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubscriptionSyncHealth {
    /// Time at which the most recent refresh attempt started.
    pub last_attempt_at: Option<SystemTime>,
    /// Time at which the most recent successful refresh started.
    pub last_success_at: Option<SystemTime>,
    /// Whether the most recent refresh attempt succeeded.
    pub last_attempt_succeeded: Option<bool>,
    /// Success value from the attempt before `last_attempt_succeeded`.
    pub previous_attempt_succeeded: Option<bool>,
}
