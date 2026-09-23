// SPDX-License-Identifier: Apache-2.0
#![allow(dead_code)]

use up_rust::{
    NativePayloadIdentity, NativeProfileAgreement, PayloadEncoding, UCode, UFrameMetadata,
    UMessage, UStatus,
};

/// Immutable worker-scoped configuration loaded once at startup.
#[derive(Clone, Debug, Default)]
pub(crate) struct NativeContext {
    profile: Option<NativeProfileAgreement>,
}

impl NativeContext {
    pub(crate) fn load() -> Result<Self, UStatus> {
        #[cfg(feature = "selected-wire-common")]
        let profile =
            configurable_streamer_wire_support::native_profile::load_native_agreement_from_env()?;
        #[cfg(not(feature = "selected-wire-common"))]
        let profile = None;
        let context = Self { profile };
        if context.profile.is_some() {
            context.identity()?;
        }
        Ok(context)
    }

    pub(crate) fn agreement(&self) -> Result<&NativeProfileAgreement, UStatus> {
        self.profile.as_ref().ok_or_else(|| {
            UStatus::fail_with_code(
                UCode::InvalidArgument,
                "native worker requires explicit local and peer profile configuration",
            )
        })
    }

    pub(crate) fn identity(&self) -> Result<NativePayloadIdentity, UStatus> {
        #[cfg(feature = "selected-wire-common")]
        {
            type Payload =
                configurable_streamer_wire_support::native_profile::SelectedWireNativePayload;
            up_rust::StableContainerPayload::<Payload>::identity(self.agreement()?)
                .map_err(UStatus::from)
        }
        #[cfg(not(feature = "selected-wire-common"))]
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "native workers require selected-wire-common",
        ))
    }

    pub(crate) fn encoding(&self) -> Result<PayloadEncoding, UStatus> {
        self.identity().map(NativePayloadIdentity::encoding)
    }

    pub(crate) fn stamp(&self, metadata: UFrameMetadata) -> Result<UFrameMetadata, UStatus> {
        metadata
            .with_native_payload_identity(self.identity()?)
            .map_err(|error| UStatus::fail_with_code(UCode::InvalidArgument, error.to_string()))
    }

    pub(crate) fn verify_metadata(&self, metadata: &UFrameMetadata) -> Result<(), UStatus> {
        up_rust::payload::codec::PayloadIdentity::Native(self.identity()?)
            .verify(metadata.payload_encoding(), metadata.native_type_token())
            .map_err(UStatus::from)
    }

    pub(crate) fn verify_classic(&self, message: &UMessage) -> Result<(), UStatus> {
        #[cfg(feature = "selected-wire-common")]
        {
            super::payloads::NativeContext::from_agreement(self.agreement()?.clone())?
                .verify_classic(message)
        }
        #[cfg(not(feature = "selected-wire-common"))]
        {
            let _ = message;
            Err(UStatus::fail_with_code(
                UCode::Unimplemented,
                "native workers require selected-wire-common",
            ))
        }
    }
}

pub(crate) fn listener(
    context: &NativeContext,
    inner: std::sync::Arc<dyn up_rust::UListener>,
) -> std::sync::Arc<dyn up_rust::UListener> {
    if context.profile.is_none() {
        return inner;
    }
    std::sync::Arc::new(NativeListener {
        context: context.clone(),
        inner,
    })
}

#[cfg(feature = "selected-wire-common")]
pub(crate) fn payload_listener(
    context: &super::payloads::NativeContext,
    inner: std::sync::Arc<dyn up_rust::UListener>,
) -> std::sync::Arc<dyn up_rust::UListener> {
    listener(
        &NativeContext {
            profile: context.configured_agreement().cloned(),
        },
        inner,
    )
}

struct NativeListener {
    context: NativeContext,
    inner: std::sync::Arc<dyn up_rust::UListener>,
}

#[async_trait::async_trait]
impl up_rust::UListener for NativeListener {
    async fn on_receive(&self, message: UMessage) {
        if let Err(error) = self.context.verify_classic(&message) {
            tracing::error!(event = "native_sink_identity_rejected", error = %error);
            return;
        }
        tracing::info!(event = "native_sink_identity_verified", encoding = ?message.payload_encoding(),
            profile_version = self.context.profile.as_ref().map(|profile| profile.profile().version()));
        self.inner.on_receive(message).await;
    }
}

#[cfg(feature = "selected-wire-common")]
pub(crate) trait BindNative: Sized {
    fn with_native_context(
        self,
        context: &NativeContext,
    ) -> Result<up_rust::StableContainerWireTransport<Self>, UStatus> {
        use up_rust::UWithNativePrefixWire;
        Ok(self.into_stable_container_transport(context.agreement()?.clone()))
    }
}

#[cfg(feature = "selected-wire-common")]
impl<T> BindNative for T {}
