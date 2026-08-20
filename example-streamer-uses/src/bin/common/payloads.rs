#![allow(dead_code)]

use std::mem::{self, MaybeUninit};

use up_rust::{EncodePayload, StablePayloadInit, UCode, UStatus};
use up_wire_xcdrv2::{XcdrV2Payload, XcdrV2Type};

pub(crate) const EXAMPLE_PAYLOAD_CAPACITY: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(type_name = "org.eclipse.uprotocol.examples.SelectedWireNativePayloadV1")]
pub(crate) struct SelectedWireNativePayload {
    magic: u32,
    sequence: u32,
    payload_len: u32,
    checksum: u32,
    payload: [u8; EXAMPLE_PAYLOAD_CAPACITY],
}

#[derive(Clone, Debug, PartialEq, XcdrV2Type)]
#[xcdr_v2(type_name = "org.eclipse.uprotocol.examples.SelectedWireXcdrV2PayloadV1")]
pub(crate) struct SelectedWireXcdrV2Payload {
    pub(crate) sequence: u32,
    pub(crate) source_hash: u32,
    pub(crate) values: [i32; 4],
    pub(crate) checksum: u32,
}

pub(crate) fn native_payload_bytes(
    magic: u32,
    sequence: u32,
    payload: &str,
) -> Result<Vec<u8>, UStatus> {
    let payload = payload.as_bytes();
    if payload.len() > EXAMPLE_PAYLOAD_CAPACITY {
        return Err(invalid_config(format!(
            "native payload is {} bytes, maximum is {} bytes",
            payload.len(),
            EXAMPLE_PAYLOAD_CAPACITY
        )));
    }

    let mut payload_storage = [0_u8; EXAMPLE_PAYLOAD_CAPACITY];
    payload_storage[..payload.len()].copy_from_slice(payload);
    let mut storage = MaybeUninit::<SelectedWireNativePayload>::uninit();
    let init = SelectedWireNativePayload::init(uninit_bytes(&mut storage))
        .map_err(stable_payload_error)?;
    let initialized = init
        .magic(magic)
        .sequence(sequence)
        .payload_len(payload.len() as u32)
        .checksum(payload_checksum(payload))
        .payload_from_array(&payload_storage)
        .finish();
    Ok(initialized.as_bytes().to_vec())
}

pub(crate) fn xcdrv2_payload_bytes(
    sequence: u32,
    source: String,
    payload: &str,
) -> Result<Vec<u8>, UStatus> {
    let checksum = payload_checksum(payload.as_bytes());
    XcdrV2Payload::encode(&SelectedWireXcdrV2Payload {
        sequence,
        source_hash: payload_checksum(source.as_bytes()),
        values: [1, 2, 3, payload.len() as i32],
        checksum,
    })
    .map(|payload| payload.into_bytes())
    .map_err(|error| invalid_config(format!("failed to encode XCDRv2 payload: {error}")))
}

pub(crate) fn arrow_payload_bytes(sequence: u32, payload: &str) -> Result<Vec<u8>, UStatus> {
    let seed = u64::from(sequence) << 32 | u64::from(payload_checksum(payload.as_bytes()));
    <up_wire_arrow::ArrowWire as EncodePayload<up_wire_arrow::TelemetryTableV1>>::encode_payload_owned(
        &up_wire_arrow::TelemetryTableV1::fixture(payload.len().max(1), seed),
    )
    .map(|payload| payload.to_vec())
    .map_err(|error| invalid_config(format!("failed to encode Arrow payload: {error}")))
}

pub(crate) fn omgidl_payload_bytes(sequence: u32, payload: &str) -> Result<Vec<u8>, UStatus> {
    let seed = u64::from(sequence) << 32 | u64::from(payload_checksum(payload.as_bytes()));
    <up_wire_omgidl::OmgIdlWire as EncodePayload<
        up_wire_omgidl::VehicleStatusV1,
    >>::encode_payload_owned(&up_wire_omgidl::VehicleStatusV1::fixture(seed))
    .map(|payload| payload.to_vec())
    .map_err(|error| invalid_config(format!("failed to encode OMG IDL payload: {error}")))
}

pub(crate) fn native_payload_alignment() -> usize {
    mem::align_of::<SelectedWireNativePayload>()
}

pub(crate) fn payload_checksum(payload: &[u8]) -> u32 {
    payload.iter().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

fn uninit_bytes<T>(storage: &mut MaybeUninit<T>) -> &mut [MaybeUninit<u8>] {
    // SAFETY: the returned byte slice covers exactly the uninitialized storage
    // for `T` and inherits its alignment.
    unsafe {
        std::slice::from_raw_parts_mut(
            std::ptr::from_mut(storage).cast::<MaybeUninit<u8>>(),
            mem::size_of::<T>(),
        )
    }
}

fn stable_payload_error(error: up_rust::UWireError) -> UStatus {
    invalid_config(format!("failed to initialize native payload: {error}"))
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
