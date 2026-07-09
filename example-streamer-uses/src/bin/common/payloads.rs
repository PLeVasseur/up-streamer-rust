#![allow(dead_code)]

use std::mem::{self, MaybeUninit};

use up_rust::{StablePayloadInit, UCode, UStatus};
use up_wire_xcdrv2::{XcdrV2Payload, XcdrV2Type};

pub(crate) const EXAMPLE_PAYLOAD_CAPACITY: usize = 256;

#[repr(C)]
#[derive(
    Clone,
    Copy,
    up_rust::StablePayload,
    up_rust::ByteBackedStablePayload,
    up_rust::StablePayloadInit,
)]
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
    pub(crate) source: String,
    pub(crate) values: Vec<i32>,
    pub(crate) maybe_checksum: Option<u32>,
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
    let init = SelectedWireNativePayload::init_from_uninit_bytes(uninit_bytes(&mut storage))
        .map_err(stable_payload_error)?;
    let _initialized = init
        .magic(magic)
        .sequence(sequence)
        .payload_len(payload.len() as u32)
        .checksum(payload_checksum(payload))
        .payload_from_array(&payload_storage)
        .finish()
        .map_err(stable_payload_error)?;

    // SAFETY: the generated stable-payload initializer returned a completion
    // proof for this exact storage after initializing all fields and padding.
    let value = unsafe { storage.assume_init() };
    Ok(stable_payload_bytes(&value))
}

pub(crate) fn xcdrv2_payload_bytes(
    sequence: u32,
    source: String,
    payload: &str,
) -> Result<Vec<u8>, UStatus> {
    let checksum = payload_checksum(payload.as_bytes());
    XcdrV2Payload::encode(&SelectedWireXcdrV2Payload {
        sequence,
        source,
        values: vec![1, 2, 3, payload.len() as i32],
        maybe_checksum: Some(checksum),
    })
    .map(|payload| payload.into_bytes())
    .map_err(|error| invalid_config(format!("failed to encode XCDRv2 payload: {error}")))
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

fn stable_payload_bytes<T: up_rust::ByteBackedStablePayload>(payload: &T) -> Vec<u8> {
    // SAFETY: byte-backed stable payloads have a fully initialized byte
    // representation with no uninitialized padding to expose.
    unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(payload).cast::<u8>(),
            mem::size_of::<T>(),
        )
        .to_vec()
    }
}

fn stable_payload_error(error: up_rust::UWireError) -> UStatus {
    invalid_config(format!("failed to initialize native payload: {error}"))
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
