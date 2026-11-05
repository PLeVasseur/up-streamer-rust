// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Explicit large native workload representation selection and real fixture generation.
use serde::{Deserialize, Serialize};
pub use streamer_native_types::large::{CameraBayerRggb12pFrame8mpV1, LidarPointCloudHesaiAt128V1};
use up_rust::bench_fixtures::payload_contract::{
    self as fixtures, PayloadContractCase, PayloadContractCaseKind,
};
use up_rust::{NativeProfileAgreement, UCode, UStatus};

/// Large sensor representations supported by the native pub/sub workload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorFixture {
    /// Full 1200-by-128 Hesai AT128 point cloud, 4,915,328 native bytes.
    LidarHesaiAt128,
    /// Full 3840-by-2160 packed Bayer frame, 12,441,784 native bytes.
    CameraBayerRggb12p8mp,
}

impl SensorFixture {
    /// The existing SDK fixture definition for this exact representation.
    pub fn case(self) -> &'static PayloadContractCase {
        let kind = match self {
            Self::LidarHesaiAt128 => PayloadContractCaseKind::LidarHesaiAt128PointCloud,
            Self::CameraBayerRggb12p8mp => PayloadContractCaseKind::Camera8mpBayerRggb12p,
        };
        fixtures::large_sensor_cases()
            .iter()
            .find(|case| case.kind() == kind)
            .expect("enabled SDK sensor fixture")
    }

    /// Actual compiled payload size; transport metadata/alignment are additional.
    pub fn payload_len(self) -> usize {
        match self {
            Self::LidarHesaiAt128 => std::mem::size_of::<LidarPointCloudHesaiAt128V1>(),
            Self::CameraBayerRggb12p8mp => std::mem::size_of::<CameraBayerRggb12pFrame8mpV1>(),
        }
    }

    /// Initializes the full existing SDK fixture using its checked heap initializer.
    ///
    /// # Errors
    /// Fails if the exact compiled representation is not admitted by the agreement.
    pub fn bytes(
        self,
        sequence: u32,
        agreement: &NativeProfileAgreement,
    ) -> Result<Vec<u8>, UStatus> {
        fixtures::stable_owned_fixture_for(self.case(), sequence, agreement)
            .map(|fixture| fixture.bytes)
            .map_err(UStatus::from)
    }

    /// Verifies typed identity and semantic fields using the exact fixture sequence.
    /// This does not form a reference from untrusted bytes.
    ///
    /// # Errors
    /// Rejects truncated/invalid data, mismatched identity or inconsistent fields.
    pub fn validate(
        self,
        metadata: &up_rust::UFrameMetadata,
        bytes: &[u8],
        agreement: &NativeProfileAgreement,
    ) -> Result<(), UStatus> {
        // BenchHeaderV1's public repr(C) sequence offset is obtained from the
        // compiled definition, never inferred from a payload marker.
        let offset = std::mem::offset_of!(fixtures::BenchHeaderV1, sequence);
        let sequence = bytes.get(offset..offset + 4).ok_or_else(|| {
            UStatus::fail_with_code(UCode::InvalidArgument, "truncated sensor sequence")
        })?;
        let sequence = u32::from_ne_bytes(sequence.try_into().expect("four-byte bounded slice"));
        fixtures::validate_stable_owned_bytes(self.case(), sequence, metadata, agreement, bytes)
            .map_err(UStatus::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use up_native_profile::{AgreementArtifact, AllocationBand, InventoryArtifact, TypeKey};
    use up_rust::{StablePayload, UFrameMetadata, UUri};

    #[test]
    fn actual_sensor_fixtures_have_full_lengths_and_checked_identity_and_fields() {
        let inventory = InventoryArtifact::from_linked(&[
            TypeKey {
                name: LidarPointCloudHesaiAt128V1::TYPE_NAME.into(),
                revision: LidarPointCloudHesaiAt128V1::REVISION,
            },
            TypeKey {
                name: CameraBayerRggb12pFrame8mpV1::TYPE_NAME.into(),
                revision: CameraBayerRggb12pFrame8mpV1::REVISION,
            },
        ])
        .unwrap();
        let artifact = AgreementArtifact::generate_table(
            &inventory,
            "sensor-test".into(),
            1,
            AllocationBand::default(),
            None,
            &[],
            crate::native_profile::matrix_identity_declarations(),
        )
        .unwrap();
        let profile = artifact.profile().unwrap();
        let agreement = NativeProfileAgreement::new(Arc::new(profile.clone()), &profile).unwrap();
        for (fixture, expected_len) in [
            (SensorFixture::LidarHesaiAt128, 4_915_328),
            (SensorFixture::CameraBayerRggb12p8mp, 12_441_784),
        ] {
            assert_eq!(fixture.payload_len(), expected_len);
            let mut bytes = fixture.bytes(7, &agreement).unwrap();
            assert_eq!(bytes.len(), expected_len);
            let representation = fixtures::stable_payload_representation(fixture.case()).unwrap();
            let identity = agreement.identity_for(&representation).unwrap();
            let metadata = UFrameMetadata::publish(
                UUri::try_from_parts("sensor", 0x0001_4210, 1, 0x9001).unwrap(),
            )
            .with_native_payload_identity(identity)
            .build()
            .unwrap();
            fixture.validate(&metadata, &bytes, &agreement).unwrap();
            assert!(fixture
                .validate(&metadata, &bytes[..bytes.len() - 1], &agreement)
                .is_err());
            bytes[0] ^= 1;
            assert!(fixture.validate(&metadata, &bytes, &agreement).is_err());
        }
    }
}
