// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Prepared native route identity, bounded setup, copying, failure and lifecycle controls.
#![cfg(feature = "experimental-copy-minimized-routing")]

#[path = "support/allocations.rs"]
mod allocations;
#[path = "support/prepared.rs"]
mod prepared;
#[global_allocator]
static ALLOCATOR: allocations::TrackingAllocator = allocations::TrackingAllocator;
use std::sync::{atomic::Ordering, Arc, Mutex};
use test_case::test_case;
use up_rust::{
    communication::{zero_copy, SubscriptionStatus},
    core::usubscription::{ResetReason, SubscriptionInfo, USubscription},
    Frame, HeaderLimits, NativeProfile, NativeProfileAgreement, NativeProfileMode,
    NativeProfileTable, PayloadEncoding, PoolCore, PoolCoreConfig, PreparedTransportCore,
    ProtobufWire, RegistrationConfig, RegistrationInbox, RxAdmission, StableContainerWire,
    StablePayload, UCode, UFrameMetadata, UOwnedFrame, UStatus, UUri, UWithNativePrefixWire,
};
use up_streamer::{
    CopyMinimizedRouteOptions, PreparedEndpoint, RouteCopySemantics, RouteKind, UStreamer,
    ZeroCopyFrameEndpoint,
};

fn topic() -> UUri {
    UUri::try_from_parts("authority-a", 0x5BA0, 1, 0x8001).unwrap()
}
fn topic_two() -> UUri {
    UUri::try_from_parts("authority-a", 0x5BA0, 1, 0x8002).unwrap()
}
struct Subscriptions;
#[async_trait::async_trait]
impl USubscription for Subscriptions {
    async fn subscribe(
        &self,
        _: &UUri,
        _: Option<u64>,
        _: Option<u32>,
    ) -> Result<SubscriptionStatus, UStatus> {
        Ok(SubscriptionStatus::Subscribed)
    }
    async fn unsubscribe(&self, _: &UUri) -> Result<(), UStatus> {
        Ok(())
    }
    async fn fetch_subscriptions_by_topic(
        &self,
        _: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Ok([topic(), topic_two()]
            .into_iter()
            .map(|topic| {
                SubscriptionInfo::new(
                    topic,
                    UUri::try_from_parts("authority-b", 0x5678, 1, 0).unwrap(),
                    SubscriptionStatus::Subscribed,
                    None,
                    None,
                )
            })
            .collect())
    }
    async fn fetch_subscriptions_by_subscriber(
        &self,
        _: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        self.fetch_subscriptions_by_topic(&UUri::any()).await
    }
    async fn register_for_notifications(&self, _: &UUri) -> Result<(), UStatus> {
        Ok(())
    }
    async fn unregister_for_notifications(&self, _: &UUri) -> Result<(), UStatus> {
        Ok(())
    }
    async fn fetch_subscribers(&self, _: &UUri) -> Result<Vec<UUri>, UStatus> {
        Ok(Vec::new())
    }
    async fn reset(&self, _: ResetReason, _: Option<String>) -> Result<(), UStatus> {
        Ok(())
    }
}
type Endpoint = ZeroCopyFrameEndpoint<up_rust::ProtobufWireTransport<Arc<prepared::Core>>>;
fn endpoint(name: &str, authority: &str, core: &Arc<prepared::Core>) -> Endpoint {
    ZeroCopyFrameEndpoint::new(
        name,
        authority,
        Arc::new(Arc::clone(core).into_protobuf_transport()),
    )
    .with_routes(&[
        prepared::reservation(topic(), None),
        prepared::reservation(topic_two(), None),
    ])
    .unwrap()
}
async fn streamer() -> UStreamer {
    UStreamer::new("prepared-routing", 4, Arc::new(Subscriptions))
        .await
        .unwrap()
}
fn payload(bytes: Option<&[u8]>) -> UOwnedFrame {
    let builder = UFrameMetadata::publish(topic());
    let metadata = if bytes.is_some() {
        builder.with_payload_encoding(PayloadEncoding::PROTOBUF)
    } else {
        builder
    }
    .build()
    .unwrap();
    UOwnedFrame::new(metadata, bytes.map(bytes::Bytes::copy_from_slice)).unwrap()
}
fn assert_route_pair<
    I: PreparedEndpoint,
    E: PreparedEndpoint + up_rust::UHasWire<Wire = I::Wire>,
>() {
}

#[test]
fn selected_zenoh_iceoryx2_route_pairs_typecheck() {
    assert_route_pair::<
        up_rust::ProtobufWireTransport<Arc<up_transport_iceoryx2_rust::Iceoryx2PreparedCore>>,
        up_rust::ProtobufWireTransport<Arc<up_transport_zenoh::ZenohPreparedCore>>,
    >();
    assert_route_pair::<
        up_rust::ProtobufWireTransport<Arc<up_transport_zenoh::ZenohPreparedCore>>,
        up_rust::ProtobufWireTransport<Arc<up_transport_iceoryx2_rust::Iceoryx2PreparedCore>>,
    >();
}

#[tokio::test]
async fn copy_minimized_route_copies_payload_once_into_egress_loan() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    let stats = right.forwarding_stats().unwrap();
    let mut streamer = streamer().await;
    streamer
        .add_copy_minimized_route_ref_with_options(
            &left,
            &right,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .await
        .unwrap();
    let frame = payload(Some(b"one-copy"));
    let expected = frame.metadata().clone();
    input.inject::<ProtobufWire>(frame);
    prepared::wait_for(|| output.sent_frames().len() == 1).await;
    assert_eq!(output.sent_frames()[0].metadata(), &expected);
    assert_eq!(output.sent_frames()[0].payload_bytes(), b"one-copy");
    assert_eq!(stats.copied_payload_bytes.load(Ordering::Acquire), 8);
    assert_eq!(stats.payload_copies.load(Ordering::Acquire), 1);
    assert_eq!(
        streamer.route_diagnostics()[0].route_kind,
        RouteKind::CopyMinimized
    );
    assert_eq!(
        streamer.route_diagnostics()[0].copy_semantics,
        RouteCopySemantics::CopyMinimizedOneCopy
    );
    streamer
        .delete_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    input.inject::<ProtobufWire>(payload(Some(b"stopped")));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert_eq!(output.sent_frames().len(), 1);
}

#[tokio::test]
async fn copy_minimized_route_preserves_ordered_bytes_without_payload_interpretation() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    let mut streamer = streamer().await;
    streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    // Frame supplies a contiguous extent. Opaque routing preserves its byte order
    // and carried encoding without invoking a payload decoder.
    let bytes = b"first-second-third".to_vec();
    let metadata = UFrameMetadata::publish(topic())
        .with_payload_encoding(PayloadEncoding::from_registry_entry(9))
        .build()
        .unwrap();
    input.inject::<ProtobufWire>(
        UOwnedFrame::with_payload(metadata.clone(), bytes.clone()).unwrap(),
    );
    prepared::wait_for(|| output.sent_frames().len() == 1).await;
    assert_eq!(output.sent_frames()[0].metadata(), &metadata);
    assert_eq!(output.sent_frames()[0].payload_bytes(), bytes);
    assert_eq!(
        right
            .forwarding_stats()
            .unwrap()
            .payload_copies
            .load(Ordering::Acquire),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn prepared_forwarding_does_not_allocate_an_intermediate_payload_or_metadata() {
    let control = allocations::Scope::start();
    drop(std::hint::black_box(Vec::<u8>::with_capacity(37)));
    let observed_control = control.finish();
    assert!(observed_control.0 >= 1 && observed_control.1 >= 37);
    let pool = || {
        Arc::new(
            PoolCore::new(PoolCoreConfig {
                slots: 8,
                slot_bytes: 8192,
                alignment: 8,
                max_routes: 8,
                max_registrations: 8,
            })
            .unwrap(),
        )
    };
    let input = pool();
    let output = pool();
    let left = ZeroCopyFrameEndpoint::new(
        "in",
        "authority-a",
        Arc::new(Arc::clone(&input).into_protobuf_transport()),
    )
    .with_receive_routes(&[(topic(), None)])
    .unwrap();
    let right = ZeroCopyFrameEndpoint::new(
        "out",
        "authority-b",
        Arc::new(Arc::clone(&output).into_protobuf_transport()),
    )
    .with_routes(&[prepared::reservation(topic(), None)])
    .unwrap();
    let inbox = Arc::new(
        RegistrationInbox::new(RegistrationConfig {
            capacity: 2,
            overflow: up_rust::OverflowPolicy::DropNewest,
        })
        .unwrap(),
    );
    let mut receiver = output.attach(&topic(), None, inbox.clone()).await.unwrap();
    let sender = up_rust::OpaqueTemplate::for_wire::<ProtobufWire>()
        .unwrap()
        .route(
            Arc::clone(&input),
            topic(),
            None,
            prepared::limits(),
            up_rust::RouteConfig::default(),
        )
        .unwrap();
    let token = "t".repeat(2048);
    let metadata = UFrameMetadata::publish(topic())
        .with_payload_encoding(PayloadEncoding::PROTOBUF)
        .with_token(token)
        .build()
        .unwrap();
    let mut streamer = streamer().await;
    streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    // Let setup-created workers park without sending any warm-up frame. All
    // forwarding tasks use this runtime's one observed thread.
    tokio::task::yield_now().await;
    let scope = allocations::Scope::start();
    for _ in 0..10 {
        sender
            .send_bytes(
                up_rust::TxHeaderParts::from(&metadata),
                Some(b"retained bytes"),
            )
            .unwrap();
        let frame = inbox.recv().await.unwrap();
        assert_eq!(frame.payload().unwrap().bytes(), b"retained bytes");
        drop(frame);
    }
    let counted = scope.finish();
    assert_eq!(
        counted,
        (0, 0),
        "all calls and bytes, including the first forwarded frame"
    );
    assert_eq!(
        right
            .forwarding_stats()
            .unwrap()
            .payload_copies
            .load(Ordering::Acquire),
        10
    );
    streamer
        .delete_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    output.detach(&mut receiver).unwrap();
}

#[tokio::test]
async fn copy_minimized_route_rejects_duplicate_same_authority_and_missing_delete() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    let same = endpoint("same", "authority-a", &output);
    let mut streamer = streamer().await;
    assert_eq!(
        streamer
            .add_copy_minimized_route_ref(&left, &same)
            .await
            .unwrap_err()
            .code(),
        UCode::InvalidArgument
    );
    assert_eq!(
        streamer
            .delete_copy_minimized_route_ref(&left, &right)
            .await
            .unwrap_err()
            .code(),
        UCode::NotFound
    );
    streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    assert_eq!(
        streamer
            .add_copy_minimized_route_ref(&left, &right)
            .await
            .unwrap_err()
            .code(),
        UCode::AlreadyExists
    );
}

#[tokio::test]
async fn copy_minimized_route_preserves_no_payload_and_present_empty_payload() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    let mut streamer = streamer().await;
    streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    input.inject::<ProtobufWire>(payload(None));
    input.inject::<ProtobufWire>(payload(Some(&[])));
    prepared::wait_for(|| output.sent_frames().len() == 2).await;
    assert!(output.sent_frames()[0].payload().is_none());
    assert_eq!(
        output.sent_frames()[1].payload().map(|bytes| bytes.len()),
        Some(0)
    );
}

#[tokio::test]
async fn copy_minimized_route_rolls_back_after_partial_registration_failure() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    input.fail_attach.store(2, Ordering::Release);
    let mut streamer = streamer().await;
    assert!(streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .is_err());
    assert!(streamer.route_diagnostics().is_empty());
    assert_eq!(input.detach_calls.load(Ordering::Acquire), 1);
    input.inject::<ProtobufWire>(payload(Some(b"not-forwarded")));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(output.sent_frames().is_empty());
}

#[tokio::test]
async fn copy_minimized_route_delete_failure_keeps_route_registered() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    let mut streamer = streamer().await;
    streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    input.fail_detach.store(true, Ordering::Release);
    assert!(streamer
        .delete_copy_minimized_route_ref(&left, &right)
        .await
        .is_err());
    assert_eq!(streamer.route_diagnostics().len(), 1);
    input.inject::<ProtobufWire>(payload(Some(b"closed-admission")));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(output.sent_frames().is_empty());
    streamer
        .delete_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    assert!(streamer.route_diagnostics().is_empty());
}

#[tokio::test]
async fn selected_wire_copy_minimized_route_accepts_same_static_wire() {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let right = endpoint("out", "authority-b", &output);
    let mut streamer = streamer().await;
    streamer
        .add_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    input.inject::<ProtobufWire>(payload(Some(b"selected")));
    prepared::wait_for(|| output.sent_frames().len() == 1).await;
    assert_eq!(output.sent_frames()[0].payload_bytes(), b"selected");
}

#[derive(Default)]
struct FailureObserver {
    events: Mutex<Vec<(&'static str, up_rust::UUID)>>,
    outcome: Mutex<Option<Result<(), up_rust::SendError>>>,
}
impl up_streamer::PreparedForwardObserver for FailureObserver {
    fn ingress(&self, header: &up_rust::RxHeader<'_>, payload: Option<&[u8]>) {
        assert_eq!(payload, Some(b"failure".as_slice()));
        self.events
            .lock()
            .unwrap()
            .push(("ingress", header.id.clone()));
    }
    fn copied_metadata(&self, id: &up_rust::UUID, metadata: &[u8]) {
        assert!(!metadata.is_empty());
        self.events.lock().unwrap().push(("metadata", id.clone()));
    }
    fn copied_payload(&self, id: &up_rust::UUID, payload: Option<&[u8]>) {
        assert_eq!(payload, Some(b"failure".as_slice()));
        self.events.lock().unwrap().push(("payload", id.clone()));
    }
    fn completed(&self, id: &up_rust::UUID, result: Result<(), up_rust::SendError>) {
        self.events.lock().unwrap().push(("completed", id.clone()));
        *self.outcome.lock().unwrap() = Some(result);
    }
}

#[test_case(true; "copied then failed native put")]
#[test_case(false; "unreserved address rejected before copy")]
#[tokio::test]
async fn failed_forwarding_completes_observation_with_actual_copy_count(reserved: bool) {
    let input = Arc::new(prepared::Core::default());
    let output = Arc::new(prepared::Core::default());
    let left = endpoint("in", "authority-a", &input);
    let observer = Arc::new(FailureObserver::default());
    let right = ZeroCopyFrameEndpoint::new(
        "out",
        "authority-b",
        Arc::new(Arc::clone(&output).into_protobuf_transport()),
    )
    .with_observed_routes(
        &[prepared::reservation(
            if reserved { topic() } else { topic_two() },
            None,
        )],
        observer.clone(),
    )
    .unwrap();
    let stats = right.forwarding_stats().unwrap();
    output.fail_publish.store(true, Ordering::Release);
    let mut streamer = streamer().await;
    streamer
        .add_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    let frame = payload(Some(b"failure"));
    let id = frame.metadata().id().clone();
    input.inject::<ProtobufWire>(frame);
    prepared::wait_for(|| observer.outcome.lock().unwrap().is_some()).await;
    let expected_error = if reserved {
        up_rust::SendError::Delivery
    } else {
        up_rust::SendError::Loan(up_rust::LoanError::InvalidRoute)
    };
    assert_eq!(*observer.outcome.lock().unwrap(), Some(Err(expected_error)));
    let stages = if reserved {
        vec!["ingress", "metadata", "payload", "completed"]
    } else {
        vec!["ingress", "completed"]
    };
    assert_eq!(
        *observer.events.lock().unwrap(),
        stages
            .into_iter()
            .map(|stage| (stage, id.clone()))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        stats.copied_payload_bytes.load(Ordering::Acquire),
        if reserved { 7 } else { 0 }
    );
    assert_eq!(
        stats.payload_copies.load(Ordering::Acquire),
        u64::from(reserved)
    );
    assert_eq!(stats.failed.load(Ordering::Acquire), u64::from(reserved));
    assert_eq!(
        stats.unreserved.load(Ordering::Acquire),
        u64::from(!reserved)
    );
    assert_eq!(stats.forwarded.load(Ordering::Acquire), 0);
    assert!(output.sent_frames().is_empty());
    streamer
        .delete_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
}

#[tokio::test]
async fn selected_wire_copy_minimized_route_drops_mismatched_wire_before_forwarding() {
    let input = Arc::new(prepared::Core::for_wire::<up_rust::UProtocolNativeWire>());
    let output = Arc::new(prepared::Core::default());
    let left = ZeroCopyFrameEndpoint::new(
        "in",
        "authority-a",
        Arc::new(Arc::clone(&input).into_protobuf_transport()),
    )
    .with_receive_routes(&[(topic(), None)])
    .unwrap();
    let right = endpoint("out", "authority-b", &output);
    let mut streamer = streamer().await;
    streamer
        .add_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    input.inject::<up_rust::UProtocolNativeWire>(payload(Some(b"wrong-wire")));
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    assert!(output.sent_frames().is_empty());
    assert_eq!(
        right
            .forwarding_stats()
            .unwrap()
            .copied_payload_bytes
            .load(Ordering::Acquire),
        0
    );
}

#[repr(C)]
#[derive(up_rust::StablePayload, up_rust::StablePayloadInit)]
#[stable_payload(type_name = "uprotocol.test.StreamerPreparedPayload")]
struct StableValue {
    sequence: u32,
    bytes: [u8; 64],
}
fn profile(domain: &str, version: u32, encoding: u16) -> Arc<NativeProfile> {
    Arc::new(
        NativeProfile::new(
            domain,
            version,
            NativeProfileMode::Table(
                NativeProfileTable::new([(
                    PayloadEncoding::private_use(u32::from(encoding)),
                    StableValue::native_representation(),
                )])
                .unwrap(),
            ),
        )
        .unwrap(),
    )
}
fn agreement(profile: &Arc<NativeProfile>) -> NativeProfileAgreement {
    NativeProfileAgreement::new(Arc::clone(profile), profile).unwrap()
}

#[test_case(false; "same prepared core types")]
#[test_case(true; "heterogeneous prepared core types")]
#[tokio::test]
async fn zero_copy_l2_stable_publish_routes_through_streamer(heterogeneous: bool) {
    async fn run<C: PreparedTransportCore>(input: Arc<C>) {
        let profile = profile("prepared", 1, 0xF101);
        let agreement = agreement(&profile);
        let source_transport =
            Arc::new(Arc::clone(&input).into_stable_container_transport(agreement.clone()));
        let output = Arc::new(prepared::Core::for_wire::<StableContainerWire>());
        let left = ZeroCopyFrameEndpoint::new("in", "authority-a", Arc::clone(&source_transport))
            .with_native_profile(Arc::clone(&profile))
            .with_receive_routes(&[(topic(), None)])
            .unwrap();
        let right = ZeroCopyFrameEndpoint::new(
            "out",
            "authority-b",
            Arc::new(Arc::clone(&output).into_stable_container_transport(agreement.clone())),
        )
        .with_native_profile(profile)
        .with_routes(&[prepared::reservation(topic(), None)])
        .unwrap();
        let inbox = Arc::new(
            RegistrationInbox::new(RegistrationConfig {
                capacity: 2,
                overflow: up_rust::OverflowPolicy::DropNewest,
            })
            .unwrap(),
        );
        let mut receiver = output.attach(&topic(), None, inbox.clone()).await.unwrap();
        let publisher = zero_copy::Publisher::<_, StableValue>::new(
            &source_transport,
            topic(),
            zero_copy::SendConfig::default(),
        )
        .unwrap();
        let mut streamer = streamer().await;
        streamer
            .add_selected_wire_copy_minimized_route_ref(&left, &right)
            .await
            .unwrap();
        publisher
            .publish_stable(up_rust::MessageFields::default(), |init| {
                init.into_initializer().sequence(17).bytes_fill(42).finish()
            })
            .unwrap();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), inbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(frame.payload().is_some());
        let typed = RxAdmission::<StableValue>::new(agreement, HeaderLimits::default())
            .unwrap()
            .admit(frame)
            .unwrap();
        assert_eq!(typed.payload().unwrap().sequence, 17);
        assert_eq!(typed.payload().unwrap().bytes, [42; 64]);
        assert_eq!(
            right
                .forwarding_stats()
                .unwrap()
                .payload_copies
                .load(Ordering::Acquire),
            1
        );
        streamer
            .delete_selected_wire_copy_minimized_route_ref(&left, &right)
            .await
            .unwrap();
        output.detach(&mut receiver).unwrap();
    }
    if heterogeneous {
        run(Arc::new(
            PoolCore::new(PoolCoreConfig {
                slots: 8,
                slot_bytes: 8192,
                alignment: 8,
                max_routes: 8,
                max_registrations: 8,
            })
            .unwrap(),
        ))
        .await;
    } else {
        run(Arc::new(prepared::Core::for_wire::<StableContainerWire>())).await;
    }
}

#[derive(Clone, Copy)]
enum NativeRouteMismatch {
    MissingIngress,
    MissingEgress,
    MissingBoth,
    Domain,
    Version,
    Allocation,
    Adapter,
}
#[test_case(NativeRouteMismatch::MissingIngress; "missing ingress declaration")]
#[test_case(NativeRouteMismatch::MissingEgress; "missing egress declaration")]
#[test_case(NativeRouteMismatch::MissingBoth; "configured adapters cannot bypass declarations")]
#[test_case(NativeRouteMismatch::Domain; "foreign deployment domain")]
#[test_case(NativeRouteMismatch::Version; "different generation")]
#[test_case(NativeRouteMismatch::Allocation; "same domain and version but different full content")]
#[test_case(NativeRouteMismatch::Adapter; "adapter disagrees with matching endpoint declarations")]
#[tokio::test]
async fn native_route_rejects_mismatches_before_activation(mismatch: NativeRouteMismatch) {
    let base = profile("base", 1, 0xF101);
    let peer = match mismatch {
        NativeRouteMismatch::Domain | NativeRouteMismatch::Adapter => profile("other", 1, 0xF101),
        NativeRouteMismatch::Version => profile("base", 2, 0xF101),
        NativeRouteMismatch::Allocation => profile("base", 1, 0xF102),
        _ => Arc::clone(&base),
    };
    let input = Arc::new(prepared::Core::for_wire::<StableContainerWire>());
    let output = Arc::new(prepared::Core::for_wire::<StableContainerWire>());
    let mut left = ZeroCopyFrameEndpoint::new(
        "in",
        "authority-a",
        Arc::new(Arc::clone(&input).into_stable_container_transport(agreement(&base))),
    );
    let mut right = ZeroCopyFrameEndpoint::new(
        "out",
        "authority-b",
        Arc::new(Arc::clone(&output).into_stable_container_transport(agreement(&peer))),
    )
    .with_routes(&[prepared::reservation(topic(), None)])
    .unwrap();
    if !matches!(
        mismatch,
        NativeRouteMismatch::MissingIngress | NativeRouteMismatch::MissingBoth
    ) {
        left = left.with_native_profile(Arc::clone(&base));
    }
    if !matches!(
        mismatch,
        NativeRouteMismatch::MissingEgress | NativeRouteMismatch::MissingBoth
    ) {
        right = right.with_native_profile(if matches!(mismatch, NativeRouteMismatch::Adapter) {
            base
        } else {
            peer
        });
    }
    let mut streamer = streamer().await;
    assert!(streamer
        .add_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .is_err());
    assert!(streamer.route_diagnostics().is_empty());
    assert_eq!(input.attach_calls.load(Ordering::Acquire), 0);
}
