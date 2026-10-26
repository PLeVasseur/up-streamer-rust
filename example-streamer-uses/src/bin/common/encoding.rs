// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Codec-backed payload selection for classic message examples.

use super::{native::NativeContext, payloads};
use clap::ValueEnum;
use up_rust::{PayloadCodecIdentity, PayloadEncoding, UStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Encoding {
    Native,
    Protobuf,
    Xcdrv2,
    Arrow,
    Omgidl,
}

impl Encoding {
    /// Explicit payload identity; native requires the supplied deployment agreement.
    pub fn identity(self, native: &NativeContext) -> Result<PayloadEncoding, UStatus> {
        Ok(match self {
            Self::Native => native.encoding()?,
            Self::Protobuf => PayloadEncoding::PROTOBUF,
            Self::Xcdrv2 => up_wire_xcdrv2::XcdrV2Wire::encoding(),
            Self::Arrow => up_wire_arrow::ArrowWire::encoding(),
            Self::Omgidl => up_wire_omgidl::OmgIdlWire::encoding(),
        })
    }

    /// Build an actual codec representation. Protobuf message types are selected
    /// by the role's Timer/HelloRequest factory, rather than treating text as protobuf.
    pub fn payload_parts(
        self,
        native: &NativeContext,
        magic: u32,
        sequence: u32,
        source: &str,
        text: &str,
    ) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
        let encoding = self.identity(native)?;
        let bytes = match self {
            Self::Native => payloads::native_payload_bytes(magic, sequence, text)?,
            Self::Xcdrv2 => payloads::xcdrv2_payload_bytes(sequence, source.into(), text)?,
            Self::Arrow => payloads::arrow_payload_bytes(sequence, text)?,
            Self::Omgidl => payloads::omgidl_payload_bytes(sequence, text)?,
            Self::Protobuf => {
                return Err(super::flow::invalid(
                    "protobuf requires a role-specific message factory",
                ))
            }
        };
        Ok((bytes, encoding))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use up_rust::{DecodePayload, EncodePayload};

    #[test]
    fn serialized_classic_factories_emit_decodable_representations_without_native_agreement() {
        let native = NativeContext::default();
        let (arrow, arrow_id) = Encoding::Arrow
            .payload_parts(&native, 0, 7, "sender", "codec test")
            .unwrap();
        assert_eq!(arrow_id, up_wire_arrow::ArrowWire::encoding());
        let table = <up_wire_arrow::ArrowWire as DecodePayload<up_wire_arrow::TelemetryTableV1>>::decode_payload(&arrow).unwrap();
        let encoded = <up_wire_arrow::ArrowWire as EncodePayload<
            up_wire_arrow::TelemetryTableV1,
        >>::encode_payload_owned(&table)
        .unwrap();
        assert_eq!(encoded.as_ref(), arrow.as_slice());

        let (omgidl, omgidl_id) = Encoding::Omgidl
            .payload_parts(&native, 0, 7, "sender", "codec test")
            .unwrap();
        assert_eq!(omgidl_id, up_wire_omgidl::OmgIdlWire::encoding());
        let status = <up_wire_omgidl::OmgIdlWire as DecodePayload<
            up_wire_omgidl::VehicleStatusV1,
        >>::decode_payload(&omgidl)
        .unwrap();
        let encoded = <up_wire_omgidl::OmgIdlWire as EncodePayload<
            up_wire_omgidl::VehicleStatusV1,
        >>::encode_payload_owned(&status)
        .unwrap();
        assert_eq!(encoded.as_ref(), omgidl.as_slice());
        assert!(Encoding::Native
            .payload_parts(&native, 0, 7, "sender", "codec test")
            .is_err());
    }
}
