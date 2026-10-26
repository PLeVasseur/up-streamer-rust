// SPDX-License-Identifier: Apache-2.0
//! Transport-independent workload role selection.
use serde::{Deserialize, Serialize};

/// Communication role whose message-accounting contract is checked.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Each accepted publication has a complete bridge handoff and sink disposition.
    PublisherSubscriber,
    /// Each processed request additionally has a correlated, reversed-endpoint response.
    ClientServerRpc,
}
