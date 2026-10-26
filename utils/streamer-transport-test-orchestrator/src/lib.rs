// SPDX-License-Identifier: Apache-2.0
//! Shared validation execution primitives for the matrix and sustained workloads.
//!
//! Workload and evidence policy is independent of the host OS. This library keeps
//! OS process/resource operations at explicit boundaries; transport construction
//! and physical resource ownership remain with deployment fixtures and bindings.
//! See the owning [portability guide](../docs/portability.md) for support and evidence.

pub mod platform;
#[cfg(unix)]
pub mod supervision;
