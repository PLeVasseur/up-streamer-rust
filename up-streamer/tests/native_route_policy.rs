// SPDX-License-Identifier: Apache-2.0
//! Verify native agreement and complete-route classic-hop policy before activation.
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use test_case::test_case;
use up_native_profile::{
    AgreementArtifact, AllocationBand, IdentityDeclarations, InventoryArtifact, TypeKey,
};
use up_rust::{NativeProfile, StablePayload};
use up_streamer::{Endpoint, NativeRouteApi, NativeRouteHop, NativeRoutePolicy, UStreamer};

#[path = "../examples/support/native_route_types.rs"]
mod types;

fn profile(contract: bool, generation: u32) -> Arc<NativeProfile> {
    let inventory = InventoryArtifact::from_linked(&[
        TypeKey {
            name: types::DoorState::TYPE_NAME.into(),
            revision: 0,
        },
        TypeKey {
            name: types::SeatState::TYPE_NAME.into(),
            revision: 0,
        },
    ])
    .unwrap();
    let artifact = if contract {
        AgreementArtifact::generate_single_type(
            &inventory,
            "route-test".into(),
            &TypeKey {
                name: types::DoorState::TYPE_NAME.into(),
                revision: 0,
            },
            IdentityDeclarations::default(),
        )
        .unwrap()
    } else {
        AgreementArtifact::generate_table(
            &inventory,
            "route-test".into(),
            1,
            AllocationBand::default(),
            None,
            &[],
            IdentityDeclarations::default(),
        )
        .unwrap()
    };
    let profile = artifact.profile().unwrap();
    Arc::new(NativeProfile::new(profile.domain(), generation, profile.mode().clone()).unwrap())
}

fn policy(profile: &Arc<NativeProfile>, remote: NativeRouteApi, allow: bool) -> NativeRoutePolicy {
    NativeRoutePolicy::new(
        "complete-route",
        [
            ("remote", remote),
            ("left", NativeRouteApi::OwnedFrame),
            ("right", NativeRouteApi::OwnedFrame),
        ]
        .into_iter()
        .map(|(name, api)| NativeRouteHop {
            name: name.into(),
            api,
            profile: Arc::clone(profile),
        })
        .collect(),
    )
    .unwrap()
    .with_allow_single_type_over_classic_hops(allow)
}

#[test_case(true, NativeRouteApi::OwnedFrame, false, true; "all frame contract")]
#[test_case(true, NativeRouteApi::NativeLoan, false, true; "native loan contract")]
#[test_case(true, NativeRouteApi::Classic, false, false; "remote classic contract rejected")]
#[test_case(true, NativeRouteApi::Classic, true, true; "remote classic contract opted in")]
#[test_case(false, NativeRouteApi::Classic, false, true; "table across classic")]
#[test_case(false, NativeRouteApi::OwnedFrame, false, true; "table across owned")]
#[test_case(false, NativeRouteApi::NativeLoan, false, true; "table across loans")]
fn complete_route_checks_remote_api_classes(
    contract: bool,
    remote: NativeRouteApi,
    allow: bool,
    accepted: bool,
) {
    let profile = profile(contract, 1);
    let policy = policy(&profile, remote, allow);
    assert_eq!(
        policy
            .validate_boundary(
                ("left", NativeRouteApi::OwnedFrame, &profile),
                ("right", NativeRouteApi::OwnedFrame, &profile)
            )
            .is_ok(),
        accepted
    );
}

#[test]
fn override_never_waives_boundary_class_membership_or_generation() {
    let first = profile(false, 1);
    let other = profile(false, 2);
    let policy = policy(&first, NativeRouteApi::Classic, true);
    assert!(policy
        .validate_boundary(
            ("left", NativeRouteApi::Classic, &first),
            ("right", NativeRouteApi::OwnedFrame, &first)
        )
        .is_err());
    assert!(policy
        .validate_boundary(
            ("missing", NativeRouteApi::OwnedFrame, &first),
            ("right", NativeRouteApi::OwnedFrame, &first)
        )
        .is_err());
    assert!(policy
        .validate_boundary(
            ("left", NativeRouteApi::OwnedFrame, &first),
            ("right", NativeRouteApi::OwnedFrame, &other)
        )
        .is_err());
}

#[tokio::test]
async fn direct_classic_activation_rejects_by_default_and_accepts_route_scoped_policy() {
    let registrations = Arc::new(AtomicUsize::new(0));
    let transport = || {
        let mut transport = up_rust::MockTransport::default();
        let registrations = Arc::clone(&registrations);
        transport
            .expect_do_register_listener()
            .returning(move |_, _, _| {
                registrations.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
        transport
            .expect_do_unregister_listener()
            .returning(|_, _, _| Ok(()));
        Arc::new(transport) as Arc<dyn up_rust::UTransport>
    };
    let profile = profile(true, 1);
    let left = Endpoint::new("left", "left-authority", transport())
        .with_native_profile(Arc::clone(&profile));
    let right = Endpoint::new("right", "right-authority", transport())
        .with_native_profile(Arc::clone(&profile));
    let subscriptions = Arc::new(usubscription_static_file::USubscriptionStaticFile::new(
        "../utils/usubscription-static-file/static-configs/testdata.json".into(),
    ));
    let mut streamer = UStreamer::new("native-policy", 4, subscriptions)
        .await
        .unwrap();
    let error = streamer.add_route_ref(&left, &right).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("allow_single_type_over_classic_hops"));
    assert_eq!(registrations.load(Ordering::SeqCst), 0);
    let policy = NativeRoutePolicy::new(
        "direct",
        ["left", "right"]
            .map(|name| NativeRouteHop {
                name: name.into(),
                api: NativeRouteApi::Classic,
                profile: Arc::clone(&profile),
            })
            .to_vec(),
    )
    .unwrap()
    .with_allow_single_type_over_classic_hops(true);
    streamer
        .add_route_ref_with_native_policy(&left, &right, Some(&policy))
        .await
        .unwrap();
    assert!(registrations.load(Ordering::SeqCst) > 0);
    streamer.delete_route_ref(&left, &right).await.unwrap();
}
