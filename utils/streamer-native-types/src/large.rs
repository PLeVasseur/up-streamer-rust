// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual SDK sensor fixture types linked into the target application inventory.
pub use up_rust::bench_fixtures::payload_contract::{
    CameraBayerRggb12pFrame8mpV1, LidarPointCloudHesaiAt128V1,
};

up_rust::register_native_type!(LidarPointCloudHesaiAt128V1);
up_rust::register_native_type!(CameraBayerRggb12pFrame8mpV1);
