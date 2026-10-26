// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Concrete native application types linked by examples and the target exporter.
//!
//! Type names and layouts are shared directly, rather than reconstructed by a
//! host generator or a transport configuration parser. Registration supplies
//! target-qualified descriptors; allocation belongs to the deployment agreement.

/// Maximum application payload bytes in the two fixed native example layouts.
pub const PAYLOAD_CAPACITY: usize = 256;

#[cfg(feature = "payload-contract-large-fixtures")]
pub mod large;

/// Native representation used by the selected-wire role examples.
#[repr(C)]
#[derive(Clone, Copy, Debug, up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(
    type_name = "org.eclipse.uprotocol.examples.SelectedWireNativePayloadV1",
    inventory
)]
pub struct SelectedWireNativePayload {
    /// Application fixture discriminator.
    pub magic: u32,
    /// Per-stream message sequence.
    pub sequence: u32,
    /// Number of initialized application bytes in the fixed payload region.
    pub payload_len: u32,
    /// Application checksum of the meaningful payload bytes.
    pub checksum: u32,
    /// Fixed application payload region.
    pub payload: [u8; PAYLOAD_CAPACITY],
}

/// Distinct native representation used by payload-flow workloads.
#[repr(C)]
#[derive(Clone, Copy, Debug, up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(
    type_name = "org.eclipse.uprotocol.streamer.payload_flow.NativeFlowPayloadV1",
    inventory
)]
pub struct NativeFlowPayload {
    /// Application fixture discriminator.
    pub magic: u32,
    /// Application payload schema version.
    pub schema_version: u32,
    /// Number of meaningful application bytes.
    pub payload_len: u32,
    /// Application checksum of those bytes.
    pub payload_checksum: u32,
    /// Fixed application payload region.
    pub payload_bytes: [u8; PAYLOAD_CAPACITY],
}
