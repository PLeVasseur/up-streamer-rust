// SPDX-License-Identifier: Apache-2.0
//! Equal-sized, distinct application representations for the deployment example.

#[repr(C)]
#[derive(up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(type_name = "example.route.DoorState", inventory)]
pub struct DoorState {
    pub state: u32,
}

#[repr(C)]
#[derive(up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(type_name = "example.route.SeatState", inventory)]
pub struct SeatState {
    pub state: u32,
}
