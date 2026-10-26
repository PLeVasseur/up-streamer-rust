// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Immutable, explicitly selected native worker contexts; no payload sniffing.
#![allow(dead_code)]
use configurable_streamer_wire_support::{
    native_profile::{NativePayloadContext, SelectedWireNativePayload},
    sensor::{CameraBayerRggb12pFrame8mpV1, LidarPointCloudHesaiAt128V1, SensorFixture},
};
use up_rust::{
    NativePayloadIdentity, NativeProfileAgreement, PayloadEncoding, UFrameMetadata, UFrameView,
    UMessage, UStatus,
};

#[derive(Clone, Debug)]
pub(crate) enum NativeContext {
    Selected(NativePayloadContext<SelectedWireNativePayload>),
    Lidar(NativePayloadContext<LidarPointCloudHesaiAt128V1>),
    Camera(NativePayloadContext<CameraBayerRggb12pFrame8mpV1>),
}

macro_rules! delegate {
    ($self:expr, $method:ident($($args:expr),*)) => {
        match $self {
            NativeContext::Selected(context) => context.$method($($args),*),
            NativeContext::Lidar(context) => context.$method($($args),*),
            NativeContext::Camera(context) => context.$method($($args),*),
        }
    };
}

impl Default for NativeContext {
    fn default() -> Self {
        Self::Selected(NativePayloadContext::default())
    }
}

impl NativeContext {
    pub fn from_env() -> Result<Self, UStatus> {
        NativePayloadContext::from_env().map(Self::Selected)
    }
    pub fn from_agreement(agreement: NativeProfileAgreement) -> Result<Self, UStatus> {
        NativePayloadContext::from_agreement(agreement).map(Self::Selected)
    }
    pub fn from_env_for_fixture(fixture: SensorFixture) -> Result<Self, UStatus> {
        match fixture {
            SensorFixture::LidarHesaiAt128 => NativePayloadContext::from_env().map(Self::Lidar),
            SensorFixture::CameraBayerRggb12p8mp => {
                NativePayloadContext::from_env().map(Self::Camera)
            }
        }
    }
    pub fn fixture(&self) -> Option<SensorFixture> {
        match self {
            Self::Selected(_) => None,
            Self::Lidar(_) => Some(SensorFixture::LidarHesaiAt128),
            Self::Camera(_) => Some(SensorFixture::CameraBayerRggb12p8mp),
        }
    }
    pub fn agreement(&self) -> Result<&NativeProfileAgreement, UStatus> {
        delegate!(self, agreement())
    }
    pub fn configured_agreement(&self) -> Option<&NativeProfileAgreement> {
        delegate!(self, configured_agreement())
    }
    pub fn identity(&self) -> Result<NativePayloadIdentity, UStatus> {
        delegate!(self, identity())
    }
    pub fn encoding(&self) -> Result<PayloadEncoding, UStatus> {
        delegate!(self, encoding())
    }
    pub fn stamp(&self, metadata: UFrameMetadata) -> Result<UFrameMetadata, UStatus> {
        delegate!(self, stamp(metadata))
    }
    pub fn project_classic(&self, message: &UMessage) -> Result<UFrameMetadata, UStatus> {
        delegate!(self, project_classic(message))
    }
    pub fn verify_owned(&self, metadata: &UFrameMetadata, bytes: &[u8]) -> Result<(), UStatus> {
        delegate!(self, verify_owned(metadata, bytes))?;
        if let Some(fixture) = self.fixture() {
            fixture.validate(metadata, bytes, self.agreement()?)?;
        }
        Ok(())
    }
    pub fn verify_view(&self, frame: &impl UFrameView) -> Result<(), UStatus> {
        delegate!(self, verify_view(frame))?;
        if let Some(fixture) = self.fixture() {
            fixture.validate(
                frame.metadata(),
                frame
                    .try_contiguous_payload()
                    .ok_or_else(|| invalid("noncontiguous sensor payload"))?,
                self.agreement()?,
            )?;
        }
        Ok(())
    }
    pub fn verify_classic(&self, message: &UMessage) -> Result<(), UStatus> {
        delegate!(self, verify_classic(message))?;
        if let Some(fixture) = self.fixture() {
            fixture.validate(
                &self.project_classic(message)?,
                &message
                    .payload()
                    .ok_or_else(|| invalid("sensor payload missing"))?,
                self.agreement()?,
            )?;
        }
        Ok(())
    }
    pub fn verify_loan<R: up_rust::ULoanedContiguousZeroCopyRxFrame>(
        &self,
        frame: &R,
    ) -> Result<(), UStatus> {
        delegate!(self, verify_loan(frame))?;
        if let Some(fixture) = self.fixture() {
            fixture.validate(
                frame.metadata(),
                frame
                    .try_contiguous_payload()
                    .ok_or_else(|| invalid("noncontiguous sensor payload"))?,
                self.agreement()?,
            )?;
        }
        Ok(())
    }
}

fn invalid(message: &str) -> UStatus {
    UStatus::fail_with_code(up_rust::UCode::InvalidArgument, message)
}
