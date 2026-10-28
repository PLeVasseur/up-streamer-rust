// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Native CloudEvents admission through real application startup loaders.
#![cfg(all(
    feature = "flow-evidence",
    feature = "selected-wire-common",
    feature = "up-wire-xcdrv2"
))]

use configurable_streamer_wire_support::native_profile::{
    generate_matrix_table_profile, DEPLOYMENT_MANIFEST_ENV, DEPLOYMENT_ROUTE_ENV,
    LOCAL_ENDPOINT_ENV, LOCAL_PROFILE_ENV, PEER_ENDPOINT_ENV, PEER_PROFILE_ENV, PROFILE_ROOT_ENV,
};
use example_streamer_uses::common::{cloudevents, payloads};
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
use up_native_profile::{DeploymentEndpoint, DeploymentManifest, DeploymentRoute};
use up_rust::{NativeTypeToken, UFrameMetadata, UOwnedFrame, UUri};

const CHILD: &str = "UPROTOCOL_CLOUDEVENTS_CONTEXT_TEST_CHILD";

#[derive(Clone, Copy)]
enum FixtureKind {
    Absent,
    Selected,
    #[cfg(feature = "payload-contract-large-fixtures")]
    Sensor,
}

struct DeploymentFixture {
    root: PathBuf,
}

impl DeploymentFixture {
    fn new(kind: FixtureKind) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "streamer-cloudevents-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let fixture = Self { root };
        if matches!(kind, FixtureKind::Absent) {
            return fixture;
        }
        let artifact = match kind {
            FixtureKind::Selected => generate_matrix_table_profile().unwrap().0,
            #[cfg(feature = "payload-contract-large-fixtures")]
            FixtureKind::Sensor => sensor_artifact(),
            FixtureKind::Absent => unreachable!(),
        };
        let endpoints = ["sender", "receiver"].map(|id| {
            let agreement_path = format!("{id}.json");
            up_native_profile::write_artifact(&fixture.root.join(&agreement_path), &artifact)
                .unwrap();
            DeploymentEndpoint {
                id: id.into(),
                agreement_path,
                content_sha256: artifact.content_sha256.clone(),
                target: artifact.target.clone(),
                domain: artifact.domain.clone(),
                generation: artifact.generation,
            }
        });
        let manifest = DeploymentManifest::new(
            endpoints.into(),
            vec![DeploymentRoute {
                id: "route".into(),
                endpoints: vec!["sender".into(), "receiver".into()],
            }],
        )
        .unwrap();
        up_native_profile::write_artifact(&fixture.root.join("manifest.json"), &manifest).unwrap();
        fixture
    }
}

impl Drop for DeploymentFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[cfg(feature = "payload-contract-large-fixtures")]
fn sensor_artifact() -> up_native_profile::AgreementArtifact {
    use configurable_streamer_wire_support::sensor::{
        CameraBayerRggb12pFrame8mpV1, LidarPointCloudHesaiAt128V1,
    };
    use up_native_profile::{AllocationBand, InventoryArtifact, TypeKey};
    use up_rust::StablePayload;
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
    up_native_profile::AgreementArtifact::generate_table(
        &inventory,
        "cloudevents.application.context".into(),
        1,
        AllocationBand::default(),
        None,
        &[],
        configurable_streamer_wire_support::native_profile::matrix_identity_declarations(),
    )
    .unwrap()
}

fn isolated(name: &str, kind: FixtureKind, body: fn()) {
    if std::env::var(CHILD).as_deref() == Ok(name) {
        body();
        return;
    }
    let fixture = DeploymentFixture::new(kind);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD, name)
        .env(cloudevents::MODE_ENV, "protobuf-roundtrip-v1")
        .env_remove(streamer_flow_evidence::OUTPUT_PATH_ENV);
    for name in [
        LOCAL_PROFILE_ENV,
        PEER_PROFILE_ENV,
        PROFILE_ROOT_ENV,
        DEPLOYMENT_MANIFEST_ENV,
        DEPLOYMENT_ROUTE_ENV,
        LOCAL_ENDPOINT_ENV,
        PEER_ENDPOINT_ENV,
    ] {
        command.env_remove(name);
    }
    if !matches!(kind, FixtureKind::Absent) {
        command
            .env(PROFILE_ROOT_ENV, &fixture.root)
            .env(DEPLOYMENT_MANIFEST_ENV, "manifest.json")
            .env(DEPLOYMENT_ROUTE_ENV, "route")
            .env(LOCAL_ENDPOINT_ENV, "sender")
            .env(PEER_ENDPOINT_ENV, "receiver");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "isolated {name} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "isolated {name} did not execute exactly one successful test"
    );
    let admitted = !matches!(kind, FixtureKind::Absent);
    let child_stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        child_stdout.contains(&format!(
            "CLOUDEVENT_CONTEXT_PUBLIC_LOADER_RETURNED admitted={admitted}"
        )),
        "isolated {name} did not reach its actual public loader"
    );
    if admitted {
        assert!(
            child_stdout.contains("CLOUDEVENT_CONTEXT_NATIVE_WITNESS_VERIFIED"),
            "isolated {name} did not verify its native witness"
        );
    }
    println!("CLOUDEVENT_CONTEXT_CHILD_RETURNED case={name} admitted={admitted} selected=1");
}

fn topic() -> UUri {
    UUri::try_from("//cloudevents-setup/1234/1/8001").unwrap()
}

fn selected_frame(context: &payloads::NativeContext) -> UOwnedFrame {
    let metadata = UFrameMetadata::publish(topic())
        .with_native_payload_identity(context.identity().unwrap())
        .build()
        .unwrap();
    let bytes =
        payloads::native_payload_bytes(u32::from_le_bytes(*b"UPNF"), 1, "first receive").unwrap();
    context.verify_owned(&metadata, &bytes).unwrap();
    println!("CLOUDEVENT_CONTEXT_NATIVE_WITNESS_VERIFIED");
    UOwnedFrame::with_payload(metadata, bytes).unwrap()
}

#[test]
fn native_context_is_ready_before_first_received_frame() {
    isolated(
        "native_context_is_ready_before_first_received_frame",
        FixtureKind::Selected,
        || {
            let context = payloads::load_native_context().unwrap();
            println!(
                "CLOUDEVENT_CONTEXT_PUBLIC_LOADER_RETURNED admitted={}",
                context.configured_agreement().is_some()
            );
            // No outbound operation or encoding lookup initialized the proof context.
            let frame = selected_frame(&context);
            cloudevents::received_frame(&frame).unwrap();
        },
    );
}

#[test]
fn absent_native_deployment_preserves_opaque_receive_without_admitting_native() {
    isolated(
        "absent_native_deployment_preserves_opaque_receive_without_admitting_native",
        FixtureKind::Absent,
        || {
            let context = payloads::load_native_context().unwrap();
            println!(
                "CLOUDEVENT_CONTEXT_PUBLIC_LOADER_RETURNED admitted={}",
                context.configured_agreement().is_some()
            );
            assert!(context.configured_agreement().is_none());
            let metadata = UFrameMetadata::publish(topic())
                .with_payload_encoding(up_rust::PayloadEncoding::RAW)
                .build()
                .unwrap();
            let opaque = UOwnedFrame::with_payload(metadata, vec![0xff, 0, 0xfe]).unwrap();
            cloudevents::received_frame(&opaque).unwrap();

            // Construct a negative witness; it is never passed to the startup loader.
            let profile = generate_matrix_table_profile().unwrap().resolve().unwrap();
            let agreement = up_rust::NativeProfileAgreement::new(
                std::sync::Arc::new(profile.clone()),
                &profile,
            )
            .unwrap();
            let uninstalled = payloads::NativeContext::from_agreement(agreement).unwrap();
            let frame = selected_frame(&uninstalled);
            let error = cloudevents::received_frame(&frame).unwrap_err();
            assert!(error.to_string().contains("needs admitted agreement"));
        },
    );
}

#[test]
fn admitted_context_still_rejects_a_wrong_native_token() {
    isolated(
        "admitted_context_still_rejects_a_wrong_native_token",
        FixtureKind::Selected,
        || {
            let context = payloads::load_native_context().unwrap();
            println!(
                "CLOUDEVENT_CONTEXT_PUBLIC_LOADER_RETURNED admitted={}",
                context.configured_agreement().is_some()
            );
            let valid = selected_frame(&context);
            cloudevents::received_frame(&valid).unwrap();
            let wrong_token =
                if valid.metadata().native_type_token() == Some(NativeTypeToken::from_u32(0)) {
                    NativeTypeToken::from_u32(1)
                } else {
                    NativeTypeToken::from_u32(0)
                };
            let metadata = UFrameMetadata::publish(topic())
                .with_payload_encoding(*valid.metadata().payload_encoding().unwrap())
                .with_native_type_token(wrong_token)
                .build()
                .unwrap();
            let wrong =
                UOwnedFrame::with_payload(metadata, valid.payload_bytes().to_vec()).unwrap();
            assert!(cloudevents::received_frame(&wrong).is_err());
        },
    );
}

#[cfg(feature = "payload-contract-large-fixtures")]
fn sensor_receive(fixture: configurable_streamer_wire_support::sensor::SensorFixture) {
    let context = payloads::load_native_context_for_fixture(fixture).unwrap();
    println!(
        "CLOUDEVENT_CONTEXT_PUBLIC_LOADER_RETURNED admitted={}",
        context.configured_agreement().is_some()
    );
    let bytes = fixture.bytes(1, context.agreement().unwrap()).unwrap();
    let metadata = UFrameMetadata::publish(topic())
        .with_native_payload_identity(context.identity().unwrap())
        .build()
        .unwrap();
    context.verify_owned(&metadata, &bytes).unwrap();
    println!("CLOUDEVENT_CONTEXT_NATIVE_WITNESS_VERIFIED");
    let frame = UOwnedFrame::with_payload(metadata, bytes).unwrap();
    cloudevents::received_frame(&frame).unwrap();
}

#[cfg(feature = "payload-contract-large-fixtures")]
#[test]
fn lidar_context_is_ready_before_first_received_frame() {
    isolated(
        "lidar_context_is_ready_before_first_received_frame",
        FixtureKind::Sensor,
        || {
            sensor_receive(
                configurable_streamer_wire_support::sensor::SensorFixture::LidarHesaiAt128,
            );
        },
    );
}

#[cfg(feature = "payload-contract-large-fixtures")]
#[test]
fn camera_context_is_ready_before_first_received_frame() {
    isolated(
        "camera_context_is_ready_before_first_received_frame",
        FixtureKind::Sensor,
        || {
            sensor_receive(
                configurable_streamer_wire_support::sensor::SensorFixture::CameraBayerRggb12p8mp,
            );
        },
    );
}
