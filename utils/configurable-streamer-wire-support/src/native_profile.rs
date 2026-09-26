/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

//! Deployment-owned native profiles for the existing Streamer matrix.
//!
//! Configuration carries complete canonical representation bytes. Names alone
//! cannot establish layout agreement. Loading is bounded and explicit; there is
//! no process-global table, payload sniffing or fallback from table mode to ID 0.

use std::{fs::File, io::Read, path::Path, sync::Arc};

use serde::{Deserialize, Serialize};
use up_rust::{
    NativeContract, NativeProfile, NativeProfileAgreement, NativeProfileMode, NativeProfileTable,
    NativeRepresentation, PayloadEncoding, StablePayload, UCode, UStatus,
};

pub const PAYLOAD_CAPACITY: usize = 256;
pub const MAX_PROFILE_DOCUMENT_BYTES: usize = 1024 * 1024;
pub const MATRIX_SELECTED_NATIVE_ID: u32 = 0xF101;
pub const MATRIX_FLOW_NATIVE_ID: u32 = 0xF102;
pub const LOCAL_PROFILE_ENV: &str = "UPROTOCOL_NATIVE_PROFILE";
pub const PEER_PROFILE_ENV: &str = "UPROTOCOL_NATIVE_PEER_PROFILE";
pub const NATIVE_VERIFIED_MARKER: &str = "NATIVE_IDENTITY_VERIFIED ";

/// Structured evidence emitted only after a worker's native receive checks pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeVerification {
    pub domain: String,
    pub version: u32,
    pub profile_digest: [u8; 32],
    pub type_name: String,
    pub encoding_id: u32,
    pub native_type_token: u32,
    pub identity_source: String,
}

/// The full representation used by the existing example-role matrix workers.
#[repr(C)]
#[derive(Clone, Copy, Debug, up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(type_name = "org.eclipse.uprotocol.examples.SelectedWireNativePayloadV1")]
pub struct SelectedWireNativePayload {
    pub magic: u32,
    pub sequence: u32,
    pub payload_len: u32,
    pub checksum: u32,
    pub payload: [u8; PAYLOAD_CAPACITY],
}

/// The distinct full representation used by the payload-flow worker.
#[repr(C)]
#[derive(Clone, Copy, Debug, up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(type_name = "org.eclipse.uprotocol.streamer.payload_flow.NativeFlowPayloadV1")]
pub struct NativeFlowPayload {
    pub magic: u32,
    pub schema_version: u32,
    pub payload_len: u32,
    pub payload_checksum: u32,
    pub payload_bytes: [u8; PAYLOAD_CAPACITY],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepresentationDeclaration {
    pub type_name: String,
    pub canonical_representation: Vec<u8>,
}

impl RepresentationDeclaration {
    pub fn of<T: StablePayload>() -> Self {
        Self {
            type_name: T::TYPE_NAME.into(),
            canonical_representation: T::native_representation().canonical_bytes(),
        }
    }

    fn resolve(&self) -> Result<NativeRepresentation, UStatus> {
        let representation = match self.type_name.as_str() {
            SelectedWireNativePayload::TYPE_NAME => {
                SelectedWireNativePayload::native_representation()
            }
            NativeFlowPayload::TYPE_NAME => NativeFlowPayload::native_representation(),
            _ => return Err(invalid("native profile names an unknown representation")),
        };
        if representation.canonical_bytes() != self.canonical_representation {
            return Err(invalid(
                "native profile representation differs from the complete local type/layout/target",
            ));
        }
        Ok(representation)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateAllocation {
    pub encoding_id: u32,
    pub representation: RepresentationDeclaration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeModeDefinition {
    Table {
        allocations: Vec<PrivateAllocation>,
    },
    ContractDefined {
        operation: String,
        representation: RepresentationDeclaration,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProfileDocument {
    pub document_version: u32,
    pub domain: String,
    pub version: u32,
    pub definition: NativeModeDefinition,
}

impl NativeProfileDocument {
    /// Resolves a loaded configuration against complete local representations.
    pub fn resolve(&self) -> Result<NativeProfile, UStatus> {
        if self.document_version != 1 {
            return Err(invalid("unsupported native profile document version"));
        }
        let mode = match &self.definition {
            NativeModeDefinition::Table { allocations } => {
                let mut entries = Vec::with_capacity(allocations.len());
                for allocation in allocations {
                    if matches!(allocation.encoding_id, 0xF001..=0xF003) {
                        return Err(invalid(
                            "native allocation conflicts with an agreed serialized wire profile",
                        ));
                    }
                    let encoding = PayloadEncoding::from_id(allocation.encoding_id)
                        .map_err(|error| invalid(error.to_string()))?;
                    entries.push((encoding, allocation.representation.resolve()?));
                }
                NativeProfileMode::Table(
                    NativeProfileTable::new(entries).map_err(|error| invalid(error.to_string()))?,
                )
            }
            NativeModeDefinition::ContractDefined {
                operation,
                representation,
            } => NativeProfileMode::ContractDefined(
                NativeContract::new(operation.clone(), representation.resolve()?)
                    .map_err(|error| invalid(error.to_string()))?,
            ),
        };
        NativeProfile::new(self.domain.clone(), self.version, mode)
            .map_err(|error| invalid(error.to_string()))
    }
}

/// Explicit table-mode configuration for the existing matrix's two native types.
/// These allocations belong to this deployment definition, not the payload types.
pub fn matrix_table_profile_document() -> NativeProfileDocument {
    NativeProfileDocument {
        document_version: 1,
        domain: "uprotocol.pr336.matrix".into(),
        version: 1,
        definition: NativeModeDefinition::Table {
            allocations: vec![
                PrivateAllocation {
                    encoding_id: MATRIX_SELECTED_NATIVE_ID,
                    representation: RepresentationDeclaration::of::<SelectedWireNativePayload>(),
                },
                PrivateAllocation {
                    encoding_id: MATRIX_FLOW_NATIVE_ID,
                    representation: RepresentationDeclaration::of::<NativeFlowPayload>(),
                },
            ],
        },
    }
}

pub fn parse_profile_document(bytes: &[u8]) -> Result<NativeProfile, UStatus> {
    if bytes.len() > MAX_PROFILE_DOCUMENT_BYTES {
        return Err(invalid(
            "native profile document exceeds the configured size limit",
        ));
    }
    let document: NativeProfileDocument = serde_json::from_slice(bytes)
        .map_err(|error| invalid(format!("invalid native profile document: {error}")))?;
    document.resolve()
}

pub fn load_native_profile(path: &Path) -> Result<NativeProfile, UStatus> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| {
            invalid(format!(
                "cannot open native profile {}: {error}",
                path.display()
            ))
        })?
        .take((MAX_PROFILE_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            invalid(format!(
                "cannot read native profile {}: {error}",
                path.display()
            ))
        })?;
    parse_profile_document(&bytes)
}

/// Independently loads both declared peer configurations before constructing the
/// immutable agreement. Configuration ownership supplies actual peer paths.
pub fn load_native_agreement(local: &Path, peer: &Path) -> Result<NativeProfileAgreement, UStatus> {
    let local = load_native_profile(local)?;
    let peer = load_native_profile(peer)?;
    NativeProfileAgreement::new(Arc::new(local), &peer).map_err(|error| invalid(error.to_string()))
}

/// Loads explicit local and peer paths once during process configuration.
/// Callers retain the returned immutable agreement in their endpoint state.
/// A missing pair supports non-native/opaque operation; native constructors
/// still require an agreement, and a half-configured pair is rejected.
pub fn load_native_agreement_from_env() -> Result<Option<NativeProfileAgreement>, UStatus> {
    match (
        std::env::var_os(LOCAL_PROFILE_ENV),
        std::env::var_os(PEER_PROFILE_ENV),
    ) {
        (None, None) => Ok(None),
        (Some(local), Some(peer)) => {
            load_native_agreement(Path::new(&local), Path::new(&peer)).map(Some)
        }
        _ => Err(invalid(
            "both local and peer native profile paths must be configured",
        )),
    }
}

/// Immutable native configuration retained by one worker across operations.
pub struct NativePayloadContext<T: StablePayload> {
    agreement: Option<NativeProfileAgreement>,
    payload: std::marker::PhantomData<T>,
}

impl<T: StablePayload> Default for NativePayloadContext<T> {
    fn default() -> Self {
        Self {
            agreement: None,
            payload: std::marker::PhantomData,
        }
    }
}

impl<T: StablePayload> Clone for NativePayloadContext<T> {
    fn clone(&self) -> Self {
        Self {
            agreement: self.agreement.clone(),
            payload: std::marker::PhantomData,
        }
    }
}

impl<T: StablePayload> std::fmt::Debug for NativePayloadContext<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePayloadContext")
            .field("type_name", &T::TYPE_NAME)
            .field("agreement", &self.agreement)
            .finish()
    }
}

impl<T: StablePayload> NativePayloadContext<T> {
    /// Loads configuration once; missing context remains unusable for native operations.
    pub fn from_env() -> Result<Self, UStatus> {
        match load_native_agreement_from_env()? {
            Some(agreement) => Self::from_agreement(agreement),
            None => Ok(Self::default()),
        }
    }

    pub fn from_agreement(agreement: NativeProfileAgreement) -> Result<Self, UStatus> {
        up_rust::StableContainerPayload::<T>::identity(&agreement).map_err(UStatus::from)?;
        Ok(Self {
            agreement: Some(agreement),
            payload: std::marker::PhantomData,
        })
    }

    pub fn configured_agreement(&self) -> Option<&NativeProfileAgreement> {
        self.agreement.as_ref()
    }

    pub fn agreement(&self) -> Result<&NativeProfileAgreement, UStatus> {
        self.agreement.as_ref().ok_or_else(|| {
            invalid("native operation requires explicit local and peer profile configuration")
        })
    }

    pub fn identity(&self) -> Result<up_rust::NativePayloadIdentity, UStatus> {
        up_rust::StableContainerPayload::<T>::identity(self.agreement()?).map_err(UStatus::from)
    }

    pub fn encoding(&self) -> Result<PayloadEncoding, UStatus> {
        self.identity()
            .map(up_rust::NativePayloadIdentity::encoding)
    }

    pub fn stamp(
        &self,
        metadata: up_rust::UFrameMetadata,
    ) -> Result<up_rust::UFrameMetadata, UStatus> {
        metadata
            .with_native_payload_identity(self.identity()?)
            .map_err(|error| invalid(error.to_string()))
    }

    /// Checks identity and field bits for owned bytes without casting an address.
    /// Typed reference borrowing separately uses the core loan helper's alignment,
    /// provenance and lifetime checks.
    pub fn verify_owned(
        &self,
        metadata: &up_rust::UFrameMetadata,
        bytes: &[u8],
    ) -> Result<(), UStatus> {
        self.validate_bytes(metadata, bytes)?;
        self.record_verification(metadata, "carried_frame")
    }

    fn validate_bytes(
        &self,
        metadata: &up_rust::UFrameMetadata,
        bytes: &[u8],
    ) -> Result<(), UStatus> {
        use up_rust::payload::codec::PayloadCodec;
        up_rust::StableContainerPayload::<T>::verify_metadata(metadata, Some(self.agreement()?))
            .map_err(UStatus::from)?;
        if !T::validate_field_bytes(bytes) {
            return Err(invalid("native payload size or field bits are invalid"));
        }
        Ok(())
    }

    pub fn project_classic(
        &self,
        message: &up_rust::UMessage,
    ) -> Result<up_rust::UFrameMetadata, UStatus> {
        up_rust::frame::metadata::try_project_umessage_to_frame_metadata_with_native_profile(
            message,
            self.agreement()?,
        )
        .map_err(|error| invalid(error.to_string()))
    }

    pub fn verify_classic(&self, message: &up_rust::UMessage) -> Result<(), UStatus> {
        let metadata = self.project_classic(message)?;
        let bytes = message
            .payload()
            .ok_or_else(|| invalid("native message has no payload"))?;
        self.validate_bytes(&metadata, &bytes)?;
        self.record_verification(&metadata, "agreed_classic_encoding")
    }

    /// Checks a view before copying opaque bytes, without constructing a typed
    /// reference. Typed access additionally uses the loan API below.
    pub fn verify_view(&self, frame: &impl up_rust::UFrameView) -> Result<(), UStatus> {
        if let Some(retained) = frame.native_profile() {
            if retained.profile() != self.agreement()?.profile() {
                return Err(invalid(
                    "native receive profile generation differs from worker context",
                ));
            }
        }
        let bytes = frame
            .try_contiguous_payload()
            .ok_or_else(|| invalid("native payload is not contiguous"))?;
        self.verify_owned(frame.metadata(), bytes)
    }

    /// Uses the core typed loan gate, including retained generation, size,
    /// alignment, field bits and loan provenance, before a worker consumes bytes.
    pub fn verify_loan<Rx: up_rust::ULoanedContiguousZeroCopyRxFrame>(
        &self,
        frame: &Rx,
    ) -> Result<(), UStatus> {
        frame
            .borrow_stable_payload::<T>(self.agreement()?)
            .map_err(UStatus::from)?;
        self.record_verification(frame.metadata(), "carried_native_loan")
    }

    fn record_verification(
        &self,
        metadata: &up_rust::UFrameMetadata,
        identity_source: &str,
    ) -> Result<(), UStatus> {
        let profile = self.agreement()?.profile();
        let evidence = NativeVerification {
            domain: profile.domain().to_owned(),
            version: profile.version(),
            profile_digest: profile.content_digest(),
            type_name: T::TYPE_NAME.to_owned(),
            encoding_id: metadata
                .payload_encoding()
                .ok_or_else(|| invalid("verified native encoding is missing"))?
                .id(),
            native_type_token: metadata
                .native_type_token()
                .ok_or_else(|| invalid("verified native token is missing"))?
                .as_u32(),
            identity_source: identity_source.to_owned(),
        };
        let json = serde_json::to_string(&evidence).map_err(|error| invalid(error.to_string()))?;
        println!("{NATIVE_VERIFIED_MARKER}{json}");
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;
    use up_rust::{
        NativeTypeToken, UFrameMetadata, UFrameView, UMessageBuilder, UUri, UVecRxLease,
    };

    fn agreement(document: &NativeProfileDocument) -> NativeProfileAgreement {
        let local = document.resolve().unwrap();
        let peer = parse_profile_document(&serde_json::to_vec(document).unwrap()).unwrap();
        NativeProfileAgreement::new(Arc::new(local), &peer).unwrap()
    }

    fn topic() -> UUri {
        UUri::try_from_parts("native-test", 0x5BA0, 1, 0x8001).unwrap()
    }

    fn selected_context() -> NativePayloadContext<SelectedWireNativePayload> {
        NativePayloadContext::from_agreement(agreement(&matrix_table_profile_document())).unwrap()
    }

    fn metadata(encoding: Option<u32>, token: Option<NativeTypeToken>) -> UFrameMetadata {
        let mut builder = UFrameMetadata::publish(topic());
        if let Some(encoding) = encoding {
            builder = builder.with_payload_encoding(PayloadEncoding::from_id(encoding).unwrap());
        }
        if let Some(token) = token {
            builder = builder.with_native_type_token(token);
        }
        builder.build().unwrap()
    }

    #[test_case(Some(0xF101), true, false, 272, true; "matching pair and complete bytes")]
    #[test_case(None, false, false, 272, false; "missing encoding")]
    #[test_case(Some(0xF101), false, false, 272, false; "missing native token")]
    #[test_case(Some(0xF101), true, true, 272, false; "foreign representation of equal size")]
    #[test_case(Some(0xF102), true, false, 272, false; "foreign allocation")]
    #[test_case(Some(0), true, false, 272, false; "table cannot silently fall back to zero")]
    #[test_case(Some(0xF101), true, false, 0, false; "present empty native payload")]
    #[test_case(Some(0xF101), true, false, 271, false; "truncated native layout")]
    #[test_case(Some(0xF101), true, false, 273, false; "oversized native layout")]
    fn worker_checks_carried_pair_before_native_bytes(
        encoding: Option<u32>,
        has_token: bool,
        foreign: bool,
        length: usize,
        accepted: bool,
    ) {
        let token = has_token.then(|| {
            if foreign {
                NativeFlowPayload::native_type_token()
            } else {
                SelectedWireNativePayload::native_type_token()
            }
        });
        let result = selected_context().verify_owned(&metadata(encoding, token), &vec![0; length]);
        assert_eq!(result.is_ok(), accepted, "{result:?}");
    }

    #[test_case(0xF101, true; "assigned native representation")]
    #[test_case(0xF102, false; "equal-size foreign representation")]
    #[test_case(0xF201, false; "unallocated private encoding")]
    #[test_case(8, false; "opaque unassigned encoding is not native membership")]
    #[test_case(0, false; "contract mode must be explicit")]
    fn classic_sink_uses_carried_encoding(encoding: u32, accepted: bool) {
        let message = UMessageBuilder::publish(topic())
            .build_with_payload(
                vec![0; std::mem::size_of::<SelectedWireNativePayload>()],
                PayloadEncoding::from_id(encoding).unwrap(),
            )
            .unwrap();
        let result = selected_context().verify_classic(&message);
        assert_eq!(result.is_ok(), accepted, "{result:?}");
        if encoding == MATRIX_FLOW_NATIVE_ID {
            assert_eq!(
                selected_context()
                    .project_classic(&message)
                    .unwrap()
                    .native_type_token(),
                Some(NativeFlowPayload::native_type_token())
            );
        }
    }

    #[test]
    fn missing_context_and_absent_payload_never_gain_native_identity() {
        let context = NativePayloadContext::<SelectedWireNativePayload>::default();
        assert!(context.identity().is_err());
        assert!(context
            .verify_owned(
                &metadata(
                    Some(0xF101),
                    Some(SelectedWireNativePayload::native_type_token())
                ),
                &[0; 272]
            )
            .is_err());
        let absent = UMessageBuilder::publish(topic()).build().unwrap();
        assert!(selected_context().verify_classic(&absent).is_err());
    }

    #[repr(C)]
    #[derive(up_rust::StablePayload)]
    #[stable_payload(type_name = "streamer.tests.NativeBoolean")]
    struct NativeBoolean {
        value: bool,
    }

    #[test_case(0, true; "false is a valid representation")]
    #[test_case(1, true; "true is a valid representation")]
    #[test_case(2, false; "invalid boolean bit pattern")]
    #[test_case(255, false; "invalid all-ones boolean")]
    fn recursive_field_validation_is_not_skipped(value: u8, accepted: bool) {
        let profile = Arc::new(
            NativeProfile::new(
                "bits-test",
                1,
                NativeProfileMode::Table(
                    NativeProfileTable::new([(
                        PayloadEncoding::from_id(0xF211).unwrap(),
                        NativeBoolean::native_representation(),
                    )])
                    .unwrap(),
                ),
            )
            .unwrap(),
        );
        let context = NativePayloadContext::<NativeBoolean>::from_agreement(
            NativeProfileAgreement::new(profile.clone(), &profile).unwrap(),
        )
        .unwrap();
        let metadata = UFrameMetadata::publish(topic())
            .with_native_payload_identity(context.identity().unwrap())
            .build()
            .unwrap();
        assert_eq!(context.verify_owned(&metadata, &[value]).is_ok(), accepted);
    }

    struct ScopedView {
        frame: UVecRxLease,
        profile: NativeProfileAgreement,
    }

    impl UFrameView for ScopedView {
        type PayloadReader<'a> = std::io::Cursor<&'a [u8]>;
        type PayloadSlices<'a> = std::option::IntoIter<&'a [u8]>;
        fn metadata(&self) -> &UFrameMetadata {
            self.frame.metadata()
        }
        fn native_profile(&self) -> Option<&NativeProfileAgreement> {
            Some(&self.profile)
        }
        fn payload_len(&self) -> usize {
            self.frame.payload_len()
        }
        fn has_payload(&self) -> bool {
            self.frame.has_payload()
        }
        fn payload_reader(&self) -> Self::PayloadReader<'_> {
            self.frame.payload_reader()
        }
        fn payload_slices(&self) -> Self::PayloadSlices<'_> {
            self.frame.payload_slices()
        }
        fn try_contiguous_payload(&self) -> Option<&[u8]> {
            self.frame.try_contiguous_payload()
        }
    }

    impl up_rust::UZeroCopyRxLease for ScopedView {}

    impl up_rust::ULoanedContiguousZeroCopyRxFrame for ScopedView {
        fn loaned_contiguous_payload(
            &self,
        ) -> Result<up_rust::LoanedPayload<'_>, up_rust::UWireError> {
            Err(up_rust::UWireError::MissingPayload)
        }
    }

    #[test]
    fn in_flight_generation_and_loan_capability_remain_authoritative() {
        let old = selected_context();
        let mut next_document = matrix_table_profile_document();
        next_document.version = 2;
        let next = NativePayloadContext::<SelectedWireNativePayload>::from_agreement(agreement(
            &next_document,
        ))
        .unwrap();
        // Generation changes need not change the representation token or allocation.
        assert_eq!(old.identity().unwrap(), next.identity().unwrap());
        let metadata = UFrameMetadata::publish(topic())
            .with_native_payload_identity(old.identity().unwrap())
            .build()
            .unwrap();
        let frame = ScopedView {
            frame: UVecRxLease::new(metadata, Some(vec![0; 272])).unwrap(),
            profile: old.agreement().unwrap().clone(),
        };
        assert!(old.clone().verify_view(&frame).is_ok());
        assert!(next.verify_view(&frame).is_err());
        assert!(
            old.verify_loan(&frame).is_err(),
            "valid bytes do not establish a borrowable loan"
        );
    }

    #[test]
    fn serialized_configuration_retains_full_identity_and_content() {
        let document = matrix_table_profile_document();
        let local = document.resolve().unwrap();
        let peer = parse_profile_document(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert_eq!(local.content_digest(), peer.content_digest());
        let agreement = NativeProfileAgreement::new(Arc::new(local), &peer).unwrap();
        let selected = agreement
            .identity_for(&SelectedWireNativePayload::native_representation())
            .unwrap();
        let flow = agreement
            .identity_for(&NativeFlowPayload::native_representation())
            .unwrap();
        assert_eq!(selected.encoding().id(), MATRIX_SELECTED_NATIVE_ID);
        assert_eq!(flow.encoding().id(), MATRIX_FLOW_NATIVE_ID);
        assert_ne!(selected.token(), flow.token());
    }

    #[test_case(0; "no table fallback to contract-defined")]
    #[test_case(8; "unassigned public number")]
    #[test_case(0xF001; "XCDRv2 private reservation")]
    #[test_case(0xF002; "Arrow private reservation")]
    #[test_case(0xF003; "OMG IDL private reservation")]
    #[test_case(65536; "encoding overflow")]
    fn invalid_allocations_are_rejected(id: u32) {
        let mut document = matrix_table_profile_document();
        if let NativeModeDefinition::Table { allocations } = &mut document.definition {
            allocations.first_mut().unwrap().encoding_id = id;
        }
        assert!(document.resolve().is_err());
    }

    #[test]
    fn named_type_with_foreign_layout_is_rejected() {
        let mut declaration = RepresentationDeclaration::of::<SelectedWireNativePayload>();
        declaration.canonical_representation =
            NativeFlowPayload::native_representation().canonical_bytes();
        assert!(declaration.resolve().is_err());
    }

    #[test]
    fn equal_domain_version_with_different_allocation_cannot_activate() {
        let local = matrix_table_profile_document().resolve().unwrap();
        let mut peer = matrix_table_profile_document();
        if let NativeModeDefinition::Table { allocations } = &mut peer.definition {
            allocations.first_mut().unwrap().encoding_id = 0xF201;
        }
        assert!(NativeProfileAgreement::new(Arc::new(local), &peer.resolve().unwrap()).is_err());
    }

    #[test]
    fn explicit_operation_contract_uses_zero_with_exact_representation() {
        let document = NativeProfileDocument {
            document_version: 1,
            domain: "operation-test".into(),
            version: 1,
            definition: NativeModeDefinition::ContractDefined {
                operation: "event.selected.v1".into(),
                representation: RepresentationDeclaration::of::<SelectedWireNativePayload>(),
            },
        };
        let local = document.resolve().unwrap();
        let agreement = NativeProfileAgreement::new(Arc::new(local.clone()), &local).unwrap();
        assert_eq!(
            agreement
                .identity_for(&SelectedWireNativePayload::native_representation())
                .unwrap()
                .encoding(),
            PayloadEncoding::IMPLICIT
        );
        assert!(agreement
            .identity_for(&NativeFlowPayload::native_representation())
            .is_err());
        let context =
            NativePayloadContext::<SelectedWireNativePayload>::from_agreement(agreement).unwrap();
        let message = UMessageBuilder::publish(topic())
            .build_with_payload(vec![0; 272], PayloadEncoding::IMPLICIT)
            .unwrap();
        assert!(context.verify_classic(&message).is_ok());
        let metadata = context.project_classic(&message).unwrap();
        assert_eq!(
            metadata.native_type_token(),
            Some(SelectedWireNativePayload::native_type_token())
        );
        assert!(context.verify_owned(&metadata, &[0; 272]).is_ok());
        assert!(selected_context().verify_classic(&message).is_err());
    }
}
