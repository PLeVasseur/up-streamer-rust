// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual prepared iceoryx2 -> Streamer -> Zenoh SHM forwarding and lifecycle.
#![cfg(feature = "experimental-copy-minimized-routing")]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use up_rust::{
    communication::SubscriptionStatus,
    core::usubscription::{ResetReason, SubscriptionInfo, USubscription},
    Frame, HeaderAdmission, HeaderLimits, OpaqueTemplate, PayloadEncoding, PreparedTransportCore,
    ProtobufWire, RegistrationConfig, RegistrationInbox, RouteConfig, RouteLimits, SendError,
    TxHeaderParts, UCode, UFrameMetadata, UStatus, UUri, UWithNativePrefixWire, UUID,
};
use up_streamer::{
    PreparedForwardObserver, PreparedRouteReservation, UStreamer, ZeroCopyFrameEndpoint,
};
use up_transport_iceoryx2_rust::{
    Iceoryx2PreparedCore, Iceoryx2PubSubConfig, Iceoryx2RouteReservation,
};
use up_transport_zenoh::{
    zenoh_config, ZenohPreparedConfig, ZenohPreparedCore, ZenohPreparedRx, ZenohRouteReservation,
};

struct Subscriptions(SubscriptionInfo);
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
        Ok(vec![self.0.clone()])
    }
    async fn fetch_subscriptions_by_subscriber(
        &self,
        _: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Ok(vec![self.0.clone()])
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

type Observed = (UUID, usize, Option<usize>);
#[derive(Default)]
struct Observer {
    ingress: Mutex<Vec<UUID>>,
    copied: Mutex<Vec<Observed>>,
    completed: Mutex<Vec<(UUID, bool)>>,
}
impl PreparedForwardObserver for Observer {
    fn ingress(&self, header: &up_rust::RxHeader<'_>, _: Option<&[u8]>) {
        self.ingress.lock().unwrap().push(header.id.clone());
    }
    fn copied_metadata(&self, id: &UUID, metadata: &[u8]) {
        self.copied
            .lock()
            .unwrap()
            .push((id.clone(), metadata.len(), None));
    }
    fn copied_payload(&self, id: &UUID, payload: Option<&[u8]>) {
        let mut events = self.copied.lock().unwrap();
        let event = events
            .iter_mut()
            .rev()
            .find(|entry| &entry.0 == id)
            .unwrap();
        event.2 = payload.map(<[u8]>::len);
    }
    fn completed(&self, id: &UUID, result: Result<(), SendError>) {
        self.completed
            .lock()
            .unwrap()
            .push((id.clone(), result.is_ok()));
    }
}
fn native_config(endpoint: &str, listen: bool) -> zenoh_config::Config {
    let mut config = zenoh_config::Config::default();
    config
        .insert_json5("scouting/multicast/enabled", "false")
        .unwrap();
    config
        .insert_json5("scouting/gossip/enabled", "false")
        .unwrap();
    config
        .insert_json5(
            if listen {
                "listen/endpoints"
            } else {
                "connect/endpoints"
            },
            &format!("[\"{endpoint}\"]"),
        )
        .unwrap();
    if !listen {
        config.insert_json5("listen/endpoints", "[]").unwrap();
    }
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_native_bridge_preserves_opaque_metadata_order_presence_and_retained_storage() {
    let source = UUri::try_from_parts("authority-a", 0x5BA0, 1, 0x8001).unwrap();
    let subscriber = UUri::try_from_parts("authority-b", 0x5678, 1, 0).unwrap();
    let limits = RouteLimits {
        max_metadata_len: 3072,
        max_payload_len: 64,
        payload_alignment: 8,
    };
    let root = std::env::var("UP_ICEORYX2_TEST_ROOT").expect("recorded native namespace required");
    std::fs::create_dir_all(&root).unwrap();
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let prefix = format!(
        "bridge{}_{}_",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut config = Iceoryx2PubSubConfig::static_allocation(4096)
        .with_namespace(&root, &prefix)
        .unwrap();
    let native = &mut config
        .iceoryx2_config
        .as_mut()
        .unwrap()
        .defaults
        .publish_subscribe;
    native.max_subscribers = 2;
    native.subscriber_max_buffer_size = 32;
    native.subscriber_max_borrowed_samples = 8;
    native.publisher_history_size = 0;
    native.publisher_max_loaned_samples = 1;
    native.enable_safe_overflow = false;
    let ingress = Arc::new(
        Iceoryx2PreparedCore::new(
            config,
            &[Iceoryx2RouteReservation {
                source: source.clone(),
                sink: None,
                limits,
            }],
            8,
            2,
        )
        .unwrap(),
    );
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let endpoint = format!("tcp/127.0.0.1:{port}");
    let catalog = [ZenohRouteReservation {
        source: source.clone(),
        sink: None,
        limits,
        transmit: true,
    }];
    let setup = || ZenohPreparedConfig {
        shm_segment_size: 1024 * 1024,
        route_capacity: 8,
        registration_capacity: 2,
        allocation_observer: None,
    };
    let output = Arc::new(
        ZenohPreparedCore::new(
            native_config(&endpoint, true),
            "//bridge/4210/1/0",
            &catalog,
            setup(),
        )
        .await
        .unwrap(),
    );
    let receiver = Arc::new(
        ZenohPreparedCore::new(
            native_config(&endpoint, false),
            "//receiver/4210/1/0",
            &[ZenohRouteReservation {
                transmit: false,
                ..catalog[0].clone()
            }],
            setup(),
        )
        .await
        .unwrap(),
    );
    let inbox = Arc::new(
        RegistrationInbox::<ZenohPreparedRx>::new(RegistrationConfig {
            capacity: 4,
            overflow: up_rust::OverflowPolicy::Block {
                deadline: Duration::from_secs(5),
            },
        })
        .unwrap(),
    );
    let mut receiving = receiver.attach(&source, None, inbox.clone()).await.unwrap();
    output
        .wait_for_matching(&source, None, Duration::from_secs(5))
        .await
        .unwrap();
    let sender = OpaqueTemplate::for_wire::<ProtobufWire>()
        .unwrap()
        .route(
            Arc::clone(&ingress),
            source.clone(),
            None,
            limits,
            RouteConfig {
                max_token_len: 2048,
                allow_traceparent: true,
            },
        )
        .unwrap();
    let observer = Arc::new(Observer::default());
    let left = ZeroCopyFrameEndpoint::new(
        "native-in",
        "authority-a",
        Arc::new(Arc::clone(&ingress).into_protobuf_transport()),
    )
    .with_receive_routes(&[(source.clone(), None)])
    .unwrap();
    let right = ZeroCopyFrameEndpoint::new(
        "native-out",
        "authority-b",
        Arc::new(Arc::clone(&output).into_protobuf_transport()),
    )
    .with_observed_routes(
        &[PreparedRouteReservation {
            source: source.clone(),
            sink: None,
            limits,
            config: RouteConfig {
                max_token_len: 2048,
                allow_traceparent: true,
            },
        }],
        observer.clone(),
    )
    .unwrap();
    let stats = right.forwarding_stats().unwrap();
    let mut streamer = UStreamer::new(
        "native-bridge",
        4,
        Arc::new(Subscriptions(SubscriptionInfo::new(
            source.clone(),
            subscriber,
            SubscriptionStatus::Subscribed,
            None,
            None,
        ))),
    )
    .await
    .unwrap();
    let admission = HeaderAdmission::for_wire::<ProtobufWire>(HeaderLimits::default()).unwrap();
    let first = UFrameMetadata::publish(source.clone())
        .with_payload_encoding(PayloadEncoding::private_use(0xF123))
        .build()
        .unwrap();
    sender
        .send_bytes(TxHeaderParts::from(&first), Some(&[99; 8]))
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), inbox.recv())
            .await
            .is_err(),
        "native islands delivered without Streamer"
    );
    streamer
        .add_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    assert_eq!(
        streamer
            .add_selected_wire_copy_minimized_route_ref(&left, &right)
            .await
            .unwrap_err()
            .code(),
        UCode::AlreadyExists
    );
    let token = "t".repeat(1536);
    const ORDERED_MESSAGES: usize = 1000;
    let receiving_inbox = Arc::clone(&inbox);
    let receiving_token = token.clone();
    let ordered = tokio::spawn(async move {
        let admission = HeaderAdmission::for_wire::<ProtobufWire>(HeaderLimits::default()).unwrap();
        let mut ids = Vec::with_capacity(ORDERED_MESSAGES);
        for sequence in 0..u64::try_from(ORDERED_MESSAGES).unwrap() {
            let frame = tokio::time::timeout(Duration::from_secs(5), receiving_inbox.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(frame.sample().payload().as_shm().is_some());
            let admitted = admission.admit(frame).unwrap();
            if sequence == 0 {
                // Keep one processing lease while later values occupy the bounded
                // inbox. Publication must not depend on a per-send receive barrier.
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let header = admitted.header().unwrap();
            assert_eq!(header.token(), Some(receiving_token.as_str()));
            assert_eq!(
                header.payload_encoding,
                Some(PayloadEncoding::private_use(0xF123))
            );
            let payload = admitted.payload().unwrap();
            let bytes = payload.bytes();
            assert_eq!(u64::from_le_bytes(bytes[..8].try_into().unwrap()), sequence);
            assert!(bytes[8..].iter().all(|byte| *byte == 0x5a));
            ids.push(header.id.clone());
        }
        ids
    });
    let mut expected_ids = Vec::with_capacity(ORDERED_MESSAGES);
    for sequence in 0..u64::try_from(ORDERED_MESSAGES).unwrap() {
        let metadata = UFrameMetadata::publish(source.clone())
            .with_payload_encoding(PayloadEncoding::private_use(0xF123))
            .build()
            .unwrap();
        let mut header = TxHeaderParts::from(&metadata);
        header.token = Some(&token);
        let mut payload = [0x5a; 64];
        payload[..8].copy_from_slice(&sequence.to_le_bytes());
        sender.send_bytes(header, Some(&payload)).unwrap();
        expected_ids.push(metadata.id().clone());
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(ordered.await.unwrap(), expected_ids);
    for present in [false, true] {
        let builder = UFrameMetadata::publish(source.clone());
        let metadata = if present {
            builder.with_payload_encoding(PayloadEncoding::PROTOBUF)
        } else {
            builder
        }
        .build()
        .unwrap();
        sender
            .send_bytes(TxHeaderParts::from(&metadata), present.then_some(&[][..]))
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Frame::payload(&frame).is_some(), present);
        let admitted = admission.admit(frame).unwrap();
        assert_eq!(
            admitted.header().unwrap().payload_encoding.is_some(),
            present
        );
    }
    sender
        .send_bytes(TxHeaderParts::from(&first), Some(b"retained"))
        .unwrap();
    let retained = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
        .await
        .unwrap()
        .unwrap();
    let address = Frame::payload(&retained).unwrap().bytes().as_ptr();
    streamer
        .delete_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    assert!(streamer.route_diagnostics().is_empty());
    sender
        .send_bytes(TxHeaderParts::from(&first), Some(b"stopped!"))
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), inbox.recv())
            .await
            .is_err()
    );
    assert_eq!(stats.forwarded.load(Ordering::Acquire), 1003);
    assert_eq!(
        stats.copied_payload_bytes.load(Ordering::Acquire),
        1000 * 64 + 8
    );
    assert_eq!(stats.payload_copies.load(Ordering::Acquire), 1002);
    assert_eq!(stats.failed.load(Ordering::Acquire), 0);
    assert_eq!(observer.ingress.lock().unwrap().len(), 1003);
    assert_eq!(observer.copied.lock().unwrap().len(), 1003);
    assert_eq!(observer.completed.lock().unwrap().len(), 1003);
    receiver.detach(&mut receiving).unwrap();
    drop(receiving);
    drop(receiver);
    drop(sender);
    drop(left);
    drop(right);
    drop(streamer);
    drop(ingress);
    drop(output);
    assert_eq!(Frame::payload(&retained).unwrap().bytes().as_ptr(), address);
    assert_eq!(Frame::payload(&retained).unwrap().bytes(), b"retained");
}

#[cfg(feature = "owned-frame-transport")]
#[test_case::test_case(true, false; "owned to copy minimized publish requires bridge and stops after source")]
#[test_case::test_case(false, false; "copy minimized to owned publish requires bridge and stops after source")]
#[test_case::test_case(true, true; "owned to copy minimized notification is not duplicated by publish subscription")]
#[test_case::test_case(false, true; "copy minimized to owned notification is not duplicated by publish subscription")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn physical_islands_require_bridge_and_do_not_reflect(
    owned_ingress: bool,
    notification: bool,
) {
    use up_streamer::OwnedFrameEndpoint;
    use up_transport_iceoryx2_rust::{BenchmarkOwnedIceoryx2Core, Iceoryx2PubSub};
    let source = UUri::try_from_parts("authority-a", 0x5BA0, 1, 0x8001).unwrap();
    let target = UUri::try_from_parts("authority-b", 0x5678, 1, 0).unwrap();
    let sink = notification.then_some(target.clone());
    let limits = RouteLimits {
        max_metadata_len: 3072,
        max_payload_len: 8,
        payload_alignment: 8,
    };
    let root = std::env::var("UP_ICEORYX2_TEST_ROOT").unwrap();
    std::fs::create_dir_all(&root).unwrap();
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let config = |side| {
        let mut config = Iceoryx2PubSubConfig::static_allocation(65536)
            .with_namespace(
                &root,
                &format!("is{side}{}_{sequence}_", std::process::id()),
            )
            .unwrap();
        let native = &mut config
            .iceoryx2_config
            .as_mut()
            .unwrap()
            .defaults
            .publish_subscribe;
        native.max_subscribers = 2;
        native.subscriber_max_buffer_size = 32;
        native.subscriber_max_borrowed_samples = 8;
        native.publisher_history_size = 0;
        native.publisher_max_loaned_samples = 1;
        native.enable_safe_overflow = false;
        config
    };
    let left_config = config('a');
    let right_config = config('b');
    let catalog = [Iceoryx2RouteReservation {
        source: source.clone(),
        sink: sink.clone(),
        limits,
    }];
    let producer =
        Arc::new(Iceoryx2PreparedCore::new(left_config.clone(), &catalog, 8, 2).unwrap());
    let recipient =
        Arc::new(Iceoryx2PreparedCore::new(right_config.clone(), &catalog, 8, 2).unwrap());
    let inbox = Arc::new(
        RegistrationInbox::new(RegistrationConfig {
            capacity: 4,
            overflow: up_rust::OverflowPolicy::Block {
                deadline: Duration::from_secs(5),
            },
        })
        .unwrap(),
    );
    let mut receiving = recipient
        .attach(&source, sink.as_ref(), inbox.clone())
        .await
        .unwrap();
    let sender = OpaqueTemplate::for_wire::<ProtobufWire>()
        .unwrap()
        .route(
            Arc::clone(&producer),
            source.clone(),
            sink.clone(),
            limits,
            RouteConfig::default(),
        )
        .unwrap();
    let send = |value: u8| {
        let builder = match &sink {
            Some(sink) => UFrameMetadata::notification(source.clone(), sink.clone()),
            None => UFrameMetadata::publish(source.clone()),
        };
        let metadata = builder
            .with_payload_encoding(PayloadEncoding::PROTOBUF)
            .build()
            .unwrap();
        sender
            .send_bytes(TxHeaderParts::from(&metadata), Some(&[value; 8]))
            .unwrap();
        metadata.id().clone()
    };
    send(99);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), inbox.recv())
            .await
            .is_err(),
        "separate buses delivered without a bridge"
    );
    let owned_a = OwnedFrameEndpoint::from_owned(
        "a",
        "authority-a",
        Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::try_with_config(left_config).unwrap())
                .with_selected_wire(ProtobufWire),
        ),
    );
    let owned_b = OwnedFrameEndpoint::from_owned(
        "b",
        "authority-b",
        Arc::new(
            BenchmarkOwnedIceoryx2Core::new(Iceoryx2PubSub::try_with_config(right_config).unwrap())
                .with_selected_wire(ProtobufWire),
        ),
    );
    let left = ZeroCopyFrameEndpoint::new(
        "a",
        "authority-a",
        Arc::new(Arc::clone(&producer).into_protobuf_transport()),
    )
    .with_receive_routes(&[(source.clone(), sink.clone())])
    .unwrap();
    let right = ZeroCopyFrameEndpoint::new(
        "b",
        "authority-b",
        Arc::new(Arc::clone(&recipient).into_protobuf_transport()),
    )
    .with_routes(&[PreparedRouteReservation {
        source: source.clone(),
        sink: sink.clone(),
        limits,
        config: RouteConfig::default(),
    }])
    .unwrap();
    let mut streamer = UStreamer::new(
        "physical-islands",
        4,
        Arc::new(Subscriptions(SubscriptionInfo::new(
            source.clone(),
            target,
            SubscriptionStatus::Subscribed,
            None,
            None,
        ))),
    )
    .await
    .unwrap();
    if owned_ingress {
        streamer
            .add_owned_to_copy_minimized_route_ref(&owned_a, &right)
            .await
            .unwrap();
    } else {
        streamer
            .add_copy_minimized_to_owned_route_ref(&left, &owned_b)
            .await
            .unwrap();
    }
    let admission = HeaderAdmission::for_wire::<ProtobufWire>(HeaderLimits::default()).unwrap();
    for value in 0..5u8 {
        let id = send(value);
        let frame = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
            .await
            .unwrap()
            .unwrap();
        let frame = admission.admit(frame).unwrap();
        assert_eq!(frame.header().unwrap().id, id);
        assert_eq!(frame.header().unwrap().sink, sink);
        assert_eq!(frame.payload().unwrap().bytes(), &[value; 8]);
    }
    drop(sender);
    drop(producer);
    assert!(
        tokio::time::timeout(Duration::from_millis(350), inbox.recv())
            .await
            .is_err(),
        "frames recirculated after source stopped"
    );
    if owned_ingress {
        streamer
            .delete_owned_to_copy_minimized_route_ref(&owned_a, &right)
            .await
            .unwrap();
    } else {
        streamer
            .delete_copy_minimized_to_owned_route_ref(&left, &owned_b)
            .await
            .unwrap();
    }
    recipient.detach(&mut receiving).unwrap();
}
#[path = "support/carried_allocation_observer.rs"]
mod carried_allocation_observer;

#[repr(C)]
#[derive(up_rust::StablePayload)]
#[stable_payload(type_name = "uprotocol.test.CarriedExpectedSingleType")]
struct CarriedExpected {
    first: u32,
    second: u32,
}
#[repr(C)]
#[derive(up_rust::StablePayload)]
#[stable_payload(type_name = "uprotocol.test.CarriedForeignSingleType")]
struct CarriedForeign {
    first: u32,
    second: u32,
}

async fn carried_observation_barrier(signal: &str, release: &str) {
    if let Some(path) = std::env::var_os(signal) {
        let release = std::env::var_os(release).expect("recorded observation release required");
        assert!(!std::path::Path::new(&path).exists());
        std::fs::write(path, b"READY\n").unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !std::path::Path::new(&release).is_file() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native observation controller release deadline");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transported_equal_size_foreign_token_id_zero_is_rejected_before_typed_payload() {
    use up_rust::{
        AdmissionError, NativeProfile, NativeProfileAgreement, NativeProfileMode, NativeSingleType,
        RxAdmission, StableContainerWire, StablePayload,
    };
    assert_eq!(
        std::mem::size_of::<CarriedExpected>(),
        std::mem::size_of::<CarriedForeign>()
    );
    assert_eq!(
        std::mem::align_of::<CarriedExpected>(),
        std::mem::align_of::<CarriedForeign>()
    );
    let expected = CarriedExpected::native_representation();
    let foreign = CarriedForeign::native_representation();
    assert_ne!(expected.token(), foreign.token());
    let profile = Arc::new(
        NativeProfile::new(
            "carried-control",
            1,
            NativeProfileMode::SingleType(NativeSingleType::new(expected.clone())),
        )
        .unwrap(),
    );
    let agreement = NativeProfileAgreement::new(profile.clone(), &profile).unwrap();
    let typed =
        RxAdmission::<CarriedExpected>::new(agreement.clone(), HeaderLimits::default()).unwrap();
    let source = UUri::try_from_parts("authority-a", 0x5BA0, 1, 0x8001).unwrap();
    let subscriber = UUri::try_from_parts("authority-b", 0x5678, 1, 0).unwrap();
    let limits = RouteLimits {
        max_metadata_len: 3072,
        max_payload_len: 8,
        payload_alignment: 4,
    };
    let root = std::env::var("UP_ICEORYX2_TEST_ROOT").expect("recorded native namespace required");
    std::fs::create_dir_all(&root).unwrap();
    let mut config = Iceoryx2PubSubConfig::static_allocation(4096)
        .with_namespace(&root, &format!("carried{}_", std::process::id()))
        .unwrap();
    let native = &mut config
        .iceoryx2_config
        .as_mut()
        .unwrap()
        .defaults
        .publish_subscribe;
    native.max_subscribers = 2;
    native.subscriber_max_buffer_size = 2;
    native.subscriber_max_borrowed_samples = 2;
    native.publisher_history_size = 0;
    native.publisher_max_loaned_samples = 1;
    native.enable_safe_overflow = false;
    let ingress = Arc::new(
        Iceoryx2PreparedCore::new(
            config,
            &[Iceoryx2RouteReservation {
                source: source.clone(),
                sink: None,
                limits,
            }],
            8,
            2,
        )
        .unwrap(),
    );
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("tcp/127.0.0.1:{}", socket.local_addr().unwrap().port());
    drop(socket);
    let catalog = [ZenohRouteReservation {
        source: source.clone(),
        sink: None,
        limits,
        transmit: true,
    }];
    let observer = carried_allocation_observer::configured().unwrap();
    let setup = || ZenohPreparedConfig {
        shm_segment_size: 1024 * 1024,
        route_capacity: 8,
        registration_capacity: 2,
        allocation_observer: observer,
    };
    let output = Arc::new(
        ZenohPreparedCore::new(
            native_config(&endpoint, true),
            "//bridge/4210/1/0",
            &catalog,
            setup(),
        )
        .await
        .unwrap(),
    );
    let receiver = Arc::new(
        ZenohPreparedCore::new(
            native_config(&endpoint, false),
            "//receiver/4210/1/0",
            &[ZenohRouteReservation {
                transmit: false,
                ..catalog[0].clone()
            }],
            setup(),
        )
        .await
        .unwrap(),
    );
    let inbox = Arc::new(
        RegistrationInbox::<ZenohPreparedRx>::new(RegistrationConfig {
            capacity: 2,
            overflow: up_rust::OverflowPolicy::Block {
                deadline: Duration::from_secs(5),
            },
        })
        .unwrap(),
    );
    let mut registration = receiver.attach(&source, None, inbox.clone()).await.unwrap();
    output
        .wait_for_matching(&source, None, Duration::from_secs(5))
        .await
        .unwrap();
    let sender = OpaqueTemplate::for_wire::<StableContainerWire>()
        .unwrap()
        .route(
            ingress.clone(),
            source.clone(),
            None,
            limits,
            RouteConfig::default(),
        )
        .unwrap();
    let left = ZeroCopyFrameEndpoint::new(
        "native-in",
        "authority-a",
        Arc::new(
            ingress
                .clone()
                .into_stable_container_transport(agreement.clone()),
        ),
    )
    .with_native_profile(profile.clone())
    .with_receive_routes(&[(source.clone(), None)])
    .unwrap();
    let right = ZeroCopyFrameEndpoint::new(
        "native-out",
        "authority-b",
        Arc::new(output.clone().into_stable_container_transport(agreement)),
    )
    .with_native_profile(profile)
    .with_routes(&[PreparedRouteReservation {
        source: source.clone(),
        sink: None,
        limits,
        config: RouteConfig::default(),
    }])
    .unwrap();
    let stats = right.forwarding_stats().unwrap();
    let mut streamer = UStreamer::new(
        "carried-negative",
        2,
        Arc::new(Subscriptions(SubscriptionInfo::new(
            source.clone(),
            subscriber,
            SubscriptionStatus::Subscribed,
            None,
            None,
        ))),
    )
    .await
    .unwrap();
    streamer
        .add_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    carried_observation_barrier(
        "UPROTOCOL_ALLOCATION_READY_FILE",
        "UPROTOCOL_SOURCE_START_FILE",
    )
    .await;
    let bytes = [17u32.to_ne_bytes(), 42u32.to_ne_bytes()].concat();
    for (token, positive) in [(foreign.token(), false), (expected.token(), true)] {
        let metadata = UFrameMetadata::publish(source.clone())
            .with_payload_encoding(PayloadEncoding::CONTRACT_DEFINED)
            .with_native_type_token(token)
            .build()
            .unwrap();
        sender
            .send_bytes(TxHeaderParts::from(&metadata), Some(&bytes))
            .unwrap();
        let raw = tokio::time::timeout(Duration::from_secs(5), inbox.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(raw.sample().payload().as_shm().is_some());
        let admitted = HeaderAdmission::for_wire::<StableContainerWire>(HeaderLimits::default())
            .unwrap()
            .admit(raw)
            .unwrap();
        assert_eq!(admitted.header().unwrap().id, *metadata.id());
        assert_eq!(
            admitted.header().unwrap().payload_encoding,
            Some(PayloadEncoding::CONTRACT_DEFINED)
        );
        assert_eq!(admitted.header().unwrap().native_type_token, Some(token));
        assert_eq!(admitted.payload().unwrap().bytes(), bytes.as_slice());
        if positive {
            let frame = typed.admit(admitted.into_frame()).unwrap();
            assert_eq!(frame.payload().unwrap().first, 17);
            assert_eq!(frame.payload().unwrap().second, 42);
        } else {
            assert!(matches!(
                typed.admit(admitted.into_frame()),
                Err(AdmissionError::WrongPayloadType)
            ));
            println!("CARRIED_FOREIGN_TOKEN_CONTROL transported=true encoding=0 equal_size=8 typed_rejection=WrongPayloadType");
        }
    }
    assert_eq!(stats.forwarded.load(Ordering::Acquire), 2);
    assert_eq!(stats.copied_payload_bytes.load(Ordering::Acquire), 16);
    assert_eq!(stats.failed.load(Ordering::Acquire), 0);
    carried_observation_barrier(
        "UPROTOCOL_ALLOCATION_COMPLETE_FILE",
        "UPROTOCOL_SOURCE_FINISH_FILE",
    )
    .await;
    streamer
        .delete_selected_wire_copy_minimized_route_ref(&left, &right)
        .await
        .unwrap();
    receiver.detach(&mut registration).unwrap();
}
