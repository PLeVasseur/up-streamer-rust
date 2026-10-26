/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

//! Deployment-owned native profiles for the existing Streamer matrix.
//!
//! Configuration carries complete canonical representation bytes. Names alone
//! cannot establish layout agreement. Loading is bounded and explicit; there is
//! no process-global table, payload sniffing or fallback from table mode to ID 0.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use up_native_profile::{
    AgreementArtifact, AllocationBand, DeploymentManifest, IdentityDeclarations, InventoryArtifact,
    LoadedDeployment, PrivatePayloadDeclaration, SeedAllocation, TypeKey, WireDeclaration,
    WireNamespace, WireReference,
};
use up_rust::{
    NativeProfile, NativeProfileAgreement, PayloadEncoding, StablePayload, UCode, UStatus, UWire,
};

pub use streamer_native_types::{NativeFlowPayload, SelectedWireNativePayload, PAYLOAD_CAPACITY};
/// Maximum generated artifact size, shared with the owning loader.
pub const MAX_PROFILE_DOCUMENT_BYTES: usize = up_native_profile::MAX_ARTIFACT_BYTES;
/// Explicit coordinated seed for the role payload, not a type-owned ID.
pub const MATRIX_SELECTED_NATIVE_ID: u32 = 0xF101;
/// Explicit coordinated seed for the flow payload, not a type-owned ID.
pub const MATRIX_FLOW_NATIVE_ID: u32 = 0xF102;
/// Retired locator name, rejected with a migration diagnostic.
pub const LOCAL_PROFILE_ENV: &str = "UPROTOCOL_NATIVE_PROFILE";
/// Retired peer locator name, rejected with a migration diagnostic.
pub const PEER_PROFILE_ENV: &str = "UPROTOCOL_NATIVE_PEER_PROFILE";
/// Deployment artifact root used by a role process.
pub const PROFILE_ROOT_ENV: &str = "UPROTOCOL_NATIVE_PROFILE_ROOT";
/// Independently provisioned deployment manifest locator.
pub const DEPLOYMENT_MANIFEST_ENV: &str = "UPROTOCOL_NATIVE_DEPLOYMENT_MANIFEST";
/// Complete declared route identity.
pub const DEPLOYMENT_ROUTE_ENV: &str = "UPROTOCOL_NATIVE_ROUTE";
/// Local endpoint identity from the manifest.
pub const LOCAL_ENDPOINT_ENV: &str = "UPROTOCOL_NATIVE_ENDPOINT";
/// Peer endpoint identity from the manifest.
pub const PEER_ENDPOINT_ENV: &str = "UPROTOCOL_NATIVE_PEER_ENDPOINT";
/// Startup marker for independently checked deployment identity.
pub const NATIVE_DEPLOYMENT_MARKER: &str = "NATIVE_DEPLOYMENT_LOADED ";
/// Per-message identity verification marker.
pub const NATIVE_VERIFIED_MARKER: &str = "NATIVE_IDENTITY_VERIFIED ";

/// Startup evidence emitted after manifest, whole-route and local type checks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDeploymentVerification {
    /// Complete declared route identity.
    pub route: String,
    /// Local declared endpoint.
    pub endpoint: String,
    /// Distinct declared peer.
    pub peer: String,
    /// Independently supplied manifest content hash.
    pub manifest_sha256: String,
    /// Loaded agreement artifact content hash.
    pub agreement_sha256: String,
    /// Application inventory content hash bound by the agreement.
    pub inventory_sha256: String,
    /// Actual representation target bound by the agreement.
    pub target: up_native_profile::Target,
    /// Deployment domain.
    pub domain: String,
    /// Immutable deployment generation.
    pub generation: u32,
}

/// Structured evidence emitted only after a worker's native receive checks pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeVerification {
    /// Agreed deployment domain.
    pub domain: String,
    /// Immutable profile generation.
    pub version: u32,
    /// Full SDK native-profile content digest.
    pub profile_digest: [u8; 32],
    /// Expected application representation name.
    pub type_name: String,
    /// Carried deployment-private payload ID.
    pub encoding_id: u32,
    /// Independently checked structural token.
    pub native_type_token: u32,
    /// Evidence retained across frame/classic projection.
    pub identity_source: String,
}

/// Generated deployment agreement consumed by matrix and worker configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NativeProfileDocument(
    /// Complete generated and versioned agreement artifact.
    pub AgreementArtifact,
);

impl NativeProfileDocument {
    /// Validates the complete generated artifact and constructs its SDK profile.
    pub fn resolve(&self) -> Result<NativeProfile, UStatus> {
        self.0.profile().map_err(|error| invalid(error.to_string()))
    }
}

/// Collects the actual linked matrix types with explicit required identities.
pub fn matrix_native_inventory() -> Result<InventoryArtifact, UStatus> {
    InventoryArtifact::from_linked(&[
        TypeKey {
            name: SelectedWireNativePayload::TYPE_NAME.into(),
            revision: SelectedWireNativePayload::VARIANT,
        },
        TypeKey {
            name: NativeFlowPayload::TYPE_NAME.into(),
            revision: NativeFlowPayload::VARIANT,
        },
    ])
    .map_err(|error| invalid(error.to_string()))
}

/// Coordinated identity declarations for this validation deployment.
pub fn matrix_identity_declarations() -> IdentityDeclarations {
    let wires = [
        (
            WireNamespace::Wire,
            up_rust::StableContainerWireFormat::WIRE_ID,
        ),
        (
            WireNamespace::PayloadFamily,
            up_rust::StableContainerWireFormat::PAYLOAD_FAMILY_ID,
        ),
    ]
    .into_iter()
    .map(|(namespace, identity)| WireDeclaration {
        namespace,
        literal: identity.literal_id().into(),
        compact_id: identity.compact_id(),
        owner: "uprotocol.native-example".into(),
        version: 1,
        reference: WireReference::Literal,
    })
    .collect();
    let private_payloads = [
        (0xF001, "fixed PLAIN_CDR2", "up-wire-xcdrv2"),
        (0xF002, "Arrow IPC stream", "up-wire-arrow"),
        (0xF003, "strict XCDR1 little-endian", "up-wire-omgidl"),
    ]
    .into_iter()
    .map(|(encoding_id, meaning, owner)| PrivatePayloadDeclaration {
        encoding_id,
        meaning: meaning.into(),
        owner: owner.into(),
        version: 1,
    })
    .collect();
    IdentityDeclarations {
        private_payloads,
        wires,
    }
}

/// Runs the owning generator over the actual matrix inventory with explicit seed reservations.
pub fn generate_matrix_table_profile() -> Result<NativeProfileDocument, UStatus> {
    let inventory = matrix_native_inventory()?;
    let seeds = matrix_seed_allocations();
    AgreementArtifact::generate_table(
        &inventory,
        "uprotocol.examples.native".into(),
        1,
        AllocationBand::default(),
        None,
        &seeds,
        matrix_identity_declarations(),
    )
    .map(NativeProfileDocument)
    .map_err(|error| invalid(error.to_string()))
}

/// Explicit pre-coordinated seed inputs for the matrix generator.
pub fn matrix_seed_allocations() -> Vec<SeedAllocation> {
    vec![
        SeedAllocation {
            key: TypeKey {
                name: SelectedWireNativePayload::TYPE_NAME.into(),
                revision: SelectedWireNativePayload::VARIANT,
            },
            encoding_id: MATRIX_SELECTED_NATIVE_ID as u16,
        },
        SeedAllocation {
            key: TypeKey {
                name: NativeFlowPayload::TYPE_NAME.into(),
                revision: NativeFlowPayload::VARIANT,
            },
            encoding_id: MATRIX_FLOW_NATIVE_ID as u16,
        },
    ]
}

/// Generated fixture agreement for deterministic unit scenarios.
///
/// Runtime matrix admission loads an independently generated artifact instead.
/// # Panics
/// Panics if the fixed defining crate is not linked or its checked inventory is invalid.
pub fn matrix_table_profile_document() -> NativeProfileDocument {
    generate_matrix_table_profile().expect("fixed matrix type inventory and seed policy")
}

/// Parses a generated artifact; deployment admission additionally requires a manifest.
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

/// Loads a bounded generated artifact for inspection, without claiming peer admission.
pub fn load_native_profile(path: &Path) -> Result<NativeProfile, UStatus> {
    let artifact: AgreementArtifact =
        up_native_profile::read_artifact(path).map_err(|error| invalid(error.to_string()))?;
    artifact
        .profile()
        .map_err(|error| invalid(error.to_string()))
}

/// Explicit independently provisioned native deployment locator for one endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDeploymentConfig {
    /// Artifact root, relative to the application configuration directory or absolute.
    pub profile_root: PathBuf,
    /// Manifest locator, relative to profile_root or absolute.
    pub manifest: PathBuf,
    /// Complete route membership identity.
    pub route: String,
    /// This endpoint's independently declared identity.
    pub local_endpoint: String,
    /// Its independently declared peer.
    pub peer_endpoint: String,
}

/// Retained complete-route admission inputs for the Streamer configuration frontend.
#[derive(Clone, Debug)]
pub struct NativeDeploymentContext {
    /// Immutable SDK agreement established from the provisioned peer documents.
    pub agreement: NativeProfileAgreement,
    /// Complete member names from the same loaded manifest generation.
    pub members: Vec<String>,
    /// Full agreement-artifact hash, including the separate identity namespaces.
    pub agreement_sha256: String,
}

impl NativeDeploymentConfig {
    /// Verifies the whole provisioned route and emits loaded identities before endpoint activation.
    pub fn load(&self, base: &Path) -> Result<NativeProfileAgreement, UStatus> {
        self.load_for_types(base, &[])
    }

    /// Also requires linked application types to match the provisioned active descriptors.
    pub fn load_for_types(
        &self,
        base: &Path,
        required: &[TypeKey],
    ) -> Result<NativeProfileAgreement, UStatus> {
        self.load_context(base, required)
            .map(|context| context.agreement)
    }

    /// Retains membership and artifact identity from the same load as the profile agreement.
    pub fn load_context(
        &self,
        base: &Path,
        required: &[TypeKey],
    ) -> Result<NativeDeploymentContext, UStatus> {
        let root = base.join(&self.profile_root);
        let manifest: DeploymentManifest =
            up_native_profile::read_artifact(&root.join(&self.manifest))
                .map_err(|error| invalid(error.to_string()))?;
        let loaded =
            LoadedDeployment::load(&root, manifest).map_err(|error| invalid(error.to_string()))?;
        loaded
            .require_local_types(&self.local_endpoint, required)
            .map_err(|error| invalid(error.to_string()))?;
        let artifact = loaded
            .artifact(&self.local_endpoint)
            .map_err(|error| invalid(error.to_string()))?;
        artifact
            .require_wire::<up_rust::StableContainerWireFormat>()
            .map_err(|error| invalid(error.to_string()))?;
        let agreement = loaded
            .agreement(&self.route, &self.local_endpoint, &self.peer_endpoint)
            .map_err(|error| invalid(error.to_string()))?;
        let receipt = NativeDeploymentVerification {
            route: self.route.clone(),
            endpoint: self.local_endpoint.clone(),
            peer: self.peer_endpoint.clone(),
            manifest_sha256: loaded.manifest().content_sha256.clone(),
            agreement_sha256: artifact.content_sha256.clone(),
            inventory_sha256: artifact.inventory_sha256.clone(),
            target: artifact.target.clone(),
            domain: artifact.domain.clone(),
            generation: artifact.generation,
        };
        println!(
            "{NATIVE_DEPLOYMENT_MARKER}{}",
            serde_json::to_string(&receipt).map_err(|error| invalid(error.to_string()))?
        );
        let members = loaded
            .manifest()
            .routes
            .iter()
            .find(|route| route.id == self.route)
            .ok_or_else(|| invalid("unknown deployment route"))?
            .endpoints
            .clone();
        Ok(NativeDeploymentContext {
            agreement,
            members,
            agreement_sha256: artifact.content_sha256.clone(),
        })
    }
}

/// Loads an explicit deployment root, manifest, route and endpoint pair once.
/// Callers retain the returned immutable agreement in their endpoint state.
/// With no locators, non-native/opaque operation remains available; native
/// constructors still require an agreement. Partial or legacy locators fail.
pub fn load_native_agreement_from_env() -> Result<Option<NativeProfileAgreement>, UStatus> {
    load_native_agreement_for_type_from_env::<SelectedWireNativePayload>()
}

/// Loads worker locators and checks the actual linked representation of `T` before admission.
pub fn load_native_agreement_for_type_from_env<T: StablePayload>(
) -> Result<Option<NativeProfileAgreement>, UStatus> {
    if std::env::var_os(LOCAL_PROFILE_ENV).is_some() || std::env::var_os(PEER_PROFILE_ENV).is_some()
    {
        return Err(invalid("legacy native profile paths are unsupported; configure the deployment manifest/root/route/endpoints"));
    }
    let names = [
        PROFILE_ROOT_ENV,
        DEPLOYMENT_MANIFEST_ENV,
        DEPLOYMENT_ROUTE_ENV,
        LOCAL_ENDPOINT_ENV,
        PEER_ENDPOINT_ENV,
    ];
    if names.iter().all(|name| std::env::var_os(name).is_none()) {
        return Ok(None);
    }
    let required = |name| {
        std::env::var(name).map_err(|_| invalid(format!("native deployment requires {name}")))
    };
    NativeDeploymentConfig {
        profile_root: required(PROFILE_ROOT_ENV)?.into(),
        manifest: required(DEPLOYMENT_MANIFEST_ENV)?.into(),
        route: required(DEPLOYMENT_ROUTE_ENV)?,
        local_endpoint: required(LOCAL_ENDPOINT_ENV)?,
        peer_endpoint: required(PEER_ENDPOINT_ENV)?,
    }
    .load_for_types(
        Path::new("."),
        &[TypeKey {
            name: T::TYPE_NAME.into(),
            revision: T::VARIANT,
        }],
    )
    .map(Some)
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
        match load_native_agreement_for_type_from_env::<T>()? {
            Some(agreement) => Self::from_agreement(agreement),
            None => Ok(Self::default()),
        }
    }

    /// Checks this compiled payload representation against the supplied agreement.
    pub fn from_agreement(agreement: NativeProfileAgreement) -> Result<Self, UStatus> {
        up_rust::StableContainerPayload::<T>::identity(&agreement).map_err(UStatus::from)?;
        Ok(Self {
            agreement: Some(agreement),
            payload: std::marker::PhantomData,
        })
    }

    /// Returns the retained agreement, if explicitly configured.
    pub fn configured_agreement(&self) -> Option<&NativeProfileAgreement> {
        self.agreement.as_ref()
    }

    /// Requires an admitted deployment before a native operation.
    pub fn agreement(&self) -> Result<&NativeProfileAgreement, UStatus> {
        self.agreement.as_ref().ok_or_else(|| {
            invalid("native operation requires explicit deployment manifest/root/route/endpoints")
        })
    }

    /// Resolves the agreed encoding and structural token for this payload type.
    pub fn identity(&self) -> Result<up_rust::NativePayloadIdentity, UStatus> {
        up_rust::StableContainerPayload::<T>::identity(self.agreement()?).map_err(UStatus::from)
    }

    /// Returns this payload's deployment-assigned encoding.
    pub fn encoding(&self) -> Result<PayloadEncoding, UStatus> {
        self.identity()
            .map(up_rust::NativePayloadIdentity::encoding)
    }

    /// Attaches the agreed encoding/token pair to outgoing metadata.
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

    /// Projects a classic message using the retained deployment agreement.
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

    /// Validates carried classic encoding and payload bits before consumption.
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
    use std::sync::Arc;
    use test_case::test_case;
    use up_rust::{
        NativeProfileMode, NativeProfileTable, NativeTypeToken, UFrameMetadata, UFrameView,
        UMessageBuilder, UUri, UVecRxLease,
    };

    fn agreement(document: &NativeProfileDocument) -> NativeProfileAgreement {
        let local = document.resolve().unwrap();
        let peer = parse_profile_document(&serde_json::to_vec(document).unwrap()).unwrap();
        NativeProfileAgreement::new(Arc::new(local), &peer).unwrap()
    }

    fn topic() -> UUri {
        UUri::try_from_parts("native-test", 0x5BA0, 1, 0x8001).unwrap()
    }

    struct DeploymentFixture {
        root: PathBuf,
        artifact: AgreementArtifact,
        manifest: DeploymentManifest,
        config: NativeDeploymentConfig,
    }

    impl DeploymentFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "streamer-native-admission-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            let artifact = generate_matrix_table_profile().unwrap().0;
            let endpoints = ["sender", "bridge", "receiver"]
                .map(|id| {
                    let agreement_path = format!("{id}.json");
                    up_native_profile::write_artifact(&root.join(&agreement_path), &artifact)
                        .unwrap();
                    up_native_profile::DeploymentEndpoint {
                        id: id.into(),
                        agreement_path,
                        content_sha256: artifact.content_sha256.clone(),
                        target: artifact.target.clone(),
                        domain: artifact.domain.clone(),
                        generation: artifact.generation,
                    }
                })
                .to_vec();
            let manifest = DeploymentManifest::new(
                endpoints,
                vec![up_native_profile::DeploymentRoute {
                    id: "route".into(),
                    endpoints: ["sender", "bridge", "receiver"].map(str::to_owned).to_vec(),
                }],
            )
            .unwrap();
            up_native_profile::write_artifact(&root.join("manifest.json"), &manifest).unwrap();
            let config = NativeDeploymentConfig {
                profile_root: root.clone(),
                manifest: "manifest.json".into(),
                route: "route".into(),
                local_endpoint: "sender".into(),
                peer_endpoint: "bridge".into(),
            };
            Self {
                root,
                artifact,
                manifest,
                config,
            }
        }

        fn load(&self) -> Result<NativeProfileAgreement, UStatus> {
            self.config.load_for_types(
                Path::new("."),
                &[TypeKey {
                    name: SelectedWireNativePayload::TYPE_NAME.into(),
                    revision: SelectedWireNativePayload::VARIANT,
                }],
            )
        }
    }

    impl Drop for DeploymentFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn startup_loads_actual_registered_type_and_retains_immutable_generation() {
        let fixture = DeploymentFixture::new();
        let retained = fixture.load().unwrap();
        assert_eq!(retained.profile(), &fixture.artifact.profile().unwrap());
        std::fs::write(fixture.root.join("receiver.json"), b"{}").unwrap();
        assert!(fixture.load().is_err());
        assert!(
            NativePayloadContext::<SelectedWireNativePayload>::from_agreement(retained).is_ok()
        );
    }

    #[test_case("self"; "self substitution")]
    #[test_case("missing"; "missing remote member")]
    #[test_case("route"; "unknown route")]
    #[test_case("hash"; "stale expected hash")]
    #[test_case("old"; "retired document shape")]
    fn deployment_startup_rejects_incomplete_or_stale_configuration(mutation: &str) {
        let mut fixture = DeploymentFixture::new();
        match mutation {
            "self" => fixture
                .config
                .peer_endpoint
                .clone_from(&fixture.config.local_endpoint),
            "missing" => std::fs::remove_file(fixture.root.join("receiver.json")).unwrap(),
            "route" => fixture.config.route = "undeclared".into(),
            "hash" => {
                fixture.manifest.endpoints[0].content_sha256 = "a".repeat(64);
                let manifest = DeploymentManifest::new(
                    fixture.manifest.endpoints.clone(),
                    fixture.manifest.routes.clone(),
                )
                .unwrap();
                std::fs::write(
                    fixture.root.join("manifest.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            }
            "old" => std::fs::write(
                fixture.root.join("sender.json"),
                br#"{"document_version":1,"domain":"old","version":1}"#,
            )
            .unwrap(),
            _ => unreachable!(),
        }
        assert!(fixture.load().is_err());
    }

    #[test]
    fn remote_route_member_cannot_disagree_even_when_adjacent_pair_agrees() {
        let mut fixture = DeploymentFixture::new();
        let next = AgreementArtifact::generate_table(
            &matrix_native_inventory().unwrap(),
            fixture.artifact.domain.clone(),
            2,
            AllocationBand::default(),
            Some(&fixture.artifact),
            &[],
            matrix_identity_declarations(),
        )
        .unwrap();
        std::fs::write(
            fixture.root.join("receiver.json"),
            serde_json::to_vec(&next).unwrap(),
        )
        .unwrap();
        let receiver = fixture
            .manifest
            .endpoints
            .iter_mut()
            .find(|value| value.id == "receiver")
            .unwrap();
        receiver.content_sha256 = next.content_sha256;
        receiver.generation = next.generation;
        let manifest = DeploymentManifest::new(
            fixture.manifest.endpoints.clone(),
            fixture.manifest.routes.clone(),
        )
        .unwrap();
        std::fs::write(
            fixture.root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let error = fixture.load().unwrap_err();
        assert!(error.to_string().contains("route endpoints disagree"));
    }

    #[test]
    fn startup_requires_actual_linked_types() {
        let fixture = DeploymentFixture::new();
        assert!(fixture
            .config
            .load_for_types(
                Path::new("."),
                &[TypeKey {
                    name: "not.linked".into(),
                    revision: 1
                }]
            )
            .is_err());
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
        let previous = matrix_table_profile_document();
        let next_document = NativeProfileDocument(
            AgreementArtifact::generate_table(
                &matrix_native_inventory().unwrap(),
                previous.0.domain.clone(),
                2,
                AllocationBand::default(),
                Some(&previous.0),
                &[],
                previous.0.identities.clone(),
            )
            .unwrap(),
        );
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
        let seeds = serde_json::json!([{"key": {"name": SelectedWireNativePayload::TYPE_NAME, "revision": SelectedWireNativePayload::VARIANT}, "encoding_id": id}]);
        let seeds = serde_json::from_value::<Vec<SeedAllocation>>(seeds);
        if let Ok(seeds) = seeds {
            assert!(AgreementArtifact::generate_table(
                &matrix_native_inventory().unwrap(),
                "invalid-allocation".into(),
                1,
                AllocationBand {
                    first: 0xF000,
                    last: 0xFFFF
                },
                None,
                &seeds,
                matrix_identity_declarations()
            )
            .is_err());
        } else {
            assert_eq!(id, 65536);
        }
    }

    #[test]
    fn named_type_with_foreign_layout_is_rejected() {
        let mut declaration = up_native_profile::NativeDescriptor::from_representation(
            &NativeFlowPayload::native_representation(),
        );
        declaration.key = TypeKey {
            name: SelectedWireNativePayload::TYPE_NAME.into(),
            revision: SelectedWireNativePayload::VARIANT,
        };
        assert!(declaration.representation().is_err());
    }

    #[test]
    fn equal_domain_version_with_different_allocation_cannot_activate() {
        let local = matrix_table_profile_document().resolve().unwrap();
        let mut seeds = matrix_seed_allocations();
        seeds.first_mut().unwrap().encoding_id = 0xF201;
        let peer = AgreementArtifact::generate_table(
            &matrix_native_inventory().unwrap(),
            local.domain().into(),
            1,
            AllocationBand::default(),
            None,
            &seeds,
            matrix_identity_declarations(),
        )
        .unwrap()
        .profile()
        .unwrap();
        assert!(NativeProfileAgreement::new(Arc::new(local), &peer).is_err());
    }

    #[test]
    fn explicit_operation_contract_uses_zero_with_exact_representation() {
        let document = NativeProfileDocument(
            AgreementArtifact::generate_contract(
                &matrix_native_inventory().unwrap(),
                "operation-test".into(),
                "event.selected.v1".into(),
                &TypeKey {
                    name: SelectedWireNativePayload::TYPE_NAME.into(),
                    revision: SelectedWireNativePayload::VARIANT,
                },
                matrix_identity_declarations(),
            )
            .unwrap(),
        );
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
