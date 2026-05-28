/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use protobuf::well_known_types::wrappers::StringValue;
use tokio::sync::mpsc;
use up_rust::usubscription::{
    to_proto_uri, FetchSubscribersRequest, FetchSubscribersResponse, FetchSubscriptionsRequest,
    FetchSubscriptionsResponse, NotificationsRequest, ResetRequest, ResetResponse, SubscriberInfo,
    Subscription, SubscriptionRequest, SubscriptionResponse, USubscription, UnsubscribeRequest,
};
use up_rust::{
    frame_wire::{ProtobufUMessageFrame, UFrameWireFormat},
    payload::{RawBytes, UWireError},
    zero_copy::{
        UZeroCopyListener, UZeroCopyPayloadCopyExt, UZeroCopyRxFrame, UZeroCopyTransport,
        UZeroCopyTransportExt,
    },
    ProtobufPayload, UAttributes, UFrameMetadata, UMessageType, UOwnedFrame, UOwnedListener,
    UOwnedTransport, UOwnedTransportExt, UPriority, UStatus, UUri, UUID,
};
use up_streamer::{OwnedFrameEndpoint, UStreamer};
use up_transport_iceoryx2_rust::{transport::UTransportIceoryx2, Iceoryx2PubSub, MessagingPattern};
#[cfg(feature = "lola-transport")]
use up_transport_lola_rust::{LolaRxLease, LolaTransportConfig, UTransportLola};
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};

static ACTUAL_TRANSPORT_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
#[cfg(feature = "lola-transport")]
static LOLA_STREAMER_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

#[derive(Default)]
struct StaticSubscriptions {
    subscriptions: Vec<Subscription>,
}

#[async_trait]
impl USubscription for StaticSubscriptions {
    async fn subscribe(
        &self,
        _subscription_request: SubscriptionRequest,
    ) -> Result<SubscriptionResponse, UStatus> {
        Ok(SubscriptionResponse::default())
    }

    async fn fetch_subscriptions(
        &self,
        _fetch_subscriptions_request: FetchSubscriptionsRequest,
    ) -> Result<FetchSubscriptionsResponse, UStatus> {
        Ok(FetchSubscriptionsResponse {
            subscriptions: self.subscriptions.clone(),
            ..Default::default()
        })
    }

    async fn unsubscribe(&self, _unsubscribe_request: UnsubscribeRequest) -> Result<(), UStatus> {
        Ok(())
    }

    async fn register_for_notifications(
        &self,
        _notifications_register_request: NotificationsRequest,
    ) -> Result<(), UStatus> {
        Ok(())
    }

    async fn unregister_for_notifications(
        &self,
        _notifications_unregister_request: NotificationsRequest,
    ) -> Result<(), UStatus> {
        Ok(())
    }

    async fn fetch_subscribers(
        &self,
        _fetch_subscribers_request: FetchSubscribersRequest,
    ) -> Result<FetchSubscribersResponse, UStatus> {
        Ok(FetchSubscribersResponse::default())
    }

    async fn reset(&self, _reset_request: ResetRequest) -> Result<ResetResponse, UStatus> {
        Ok(ResetResponse::default())
    }
}

struct OwnedFrameSender(mpsc::UnboundedSender<UOwnedFrame>);

#[async_trait]
impl UOwnedListener for OwnedFrameSender {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        let _ = self.0.send(frame);
    }
}

struct ZeroCopyFrameSender(mpsc::UnboundedSender<UOwnedFrame>);

#[async_trait]
impl<T> UZeroCopyListener<T> for ZeroCopyFrameSender
where
    T: UZeroCopyRxFrame + Send + 'static,
{
    async fn on_receive_zero_copy(&self, frame: T) {
        let _ = self.0.send(UOwnedFrame::new(
            frame.metadata().clone(),
            frame
                .try_payload_to_vec()
                .expect("zero-copy payload slices should match payload_len"),
        ));
    }
}

fn subscriptions(subscriptions: Vec<Subscription>) -> Arc<dyn USubscription> {
    Arc::new(StaticSubscriptions { subscriptions })
}

fn subscription(topic: UUri, subscriber: UUri) -> Subscription {
    Subscription {
        topic: Some(to_proto_uri(&topic)).into(),
        subscriber: Some(SubscriberInfo {
            uri: Some(to_proto_uri(&subscriber)).into(),
            ..Default::default()
        })
        .into(),
        ..Default::default()
    }
}

fn make_topic(authority: &str, resource: u16) -> UUri {
    UUri::try_from_parts(authority, 0x4210, 1, resource).expect("valid topic")
}

fn metadata_header(topic: UUri) -> (UFrameMetadata, UUID) {
    let id = UUID::build();
    let attributes = UAttributes::new(id.clone(), topic, None, UMessageType::Publish)
        .with_priority(UPriority::CS5)
        .with_ttl(3_000)
        .with_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00");
    (UFrameMetadata::new(attributes, RawBytes::encoding()), id)
}

fn assert_streamed_metadata(frame: &UOwnedFrame, topic: &UUri, id: &UUID) {
    let attributes = frame.metadata().attributes();
    assert_eq!(attributes.id(), id);
    assert_eq!(attributes.source(), topic);
    assert_eq!(attributes.sink(), None);
    assert_eq!(attributes.message_type(), UMessageType::Publish);
    assert_eq!(attributes.priority(), UPriority::CS5);
    assert_eq!(attributes.ttl(), Some(3_000));
    assert_eq!(attributes.request_id(), None);
    assert_eq!(
        attributes.traceparent(),
        Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
    );
    assert_eq!(attributes.token(), None);
    assert_eq!(attributes.permission_level(), None);
    assert_eq!(attributes.commstatus(), None);
}

fn protobuf_payload(value: &str) -> StringValue {
    let mut payload = StringValue::new();
    payload.value = value.to_string();
    payload
}

async fn zenoh_transport(authority: &str) -> Arc<UPTransportZenoh> {
    Arc::new(
        UPTransportZenoh::builder(authority)
            .expect("zenoh builder should build")
            .with_config(ZenohConfig::default())
            .build()
            .await
            .expect("zenoh transport should build"),
    )
}

fn iceoryx2_transport() -> Arc<Iceoryx2PubSub> {
    UTransportIceoryx2::build(MessagingPattern::PublishSubscribe)
        .expect("iceoryx2 transport should build")
}

async fn actual_transport_guard() -> tokio::sync::MutexGuard<'static, ()> {
    ACTUAL_TRANSPORT_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[cfg(feature = "lola-transport")]
fn lola_transport(authority: &str) -> Arc<UTransportLola> {
    let mw_com_config_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../configurable-streamer/MW_COM_CONFIG_LOLA.json"
    );
    UTransportLola::build(LolaTransportConfig {
        local_authority: authority.to_string(),
        instance_specifier: "uprotocol/transport".to_string(),
        service_type: "/uprotocol/Transport".to_string(),
        event_name: "frame".to_string(),
        sample_size: 65_536,
        sample_alignment: 8,
        max_samples: 4,
        mw_com_config_path: Some(mw_com_config_path.to_string()),
    })
    .expect("LoLa transport should build")
}

#[cfg(feature = "lola-transport")]
async fn lola_streamer_guard() -> tokio::sync::MutexGuard<'static, ()> {
    LOLA_STREAMER_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_zenoh_owned_to_real_iceoryx2_zero_copy() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-{}", std::process::id());
    let zenoh_authority = format!("zenoh-{unique}");
    let iceoryx_authority = format!("iceoryx-{unique}");
    let topic = make_topic(&zenoh_authority, 0x9101);
    let zenoh = zenoh_transport(&zenoh_authority).await;
    let iceoryx2_egress = iceoryx2_transport();
    let iceoryx2_receiver = iceoryx2_transport();
    let (tx, mut rx) = mpsc::unbounded_channel();

    iceoryx2_receiver
        .register_zero_copy_listener(&topic, None, Arc::new(ZeroCopyFrameSender(tx)))
        .await
        .expect("iceoryx2 receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&iceoryx_authority, 0xA101),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh.clone()),
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2_egress,
            ),
        )
        .await
        .expect("route should register");

    let expected_service = Iceoryx2PubSub::publish_subscribe_service_name(&topic, None)
        .expect("service name should build");
    assert!(iceoryx2_receiver
        .discover_service_names()
        .expect("service discovery should succeed")
        .iter()
        .any(|name| name == &expected_service));

    let (header, id) = metadata_header(topic.clone());
    zenoh
        .send_serialized::<RawBytes, _>(header, &&b"zenoh-to-iox"[..])
        .await
        .expect("zenoh send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    assert_eq!(frame.payload_bytes(), b"zenoh-to-iox");
    assert_streamed_metadata(&frame, &topic, &id);
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_zenoh_owned_to_real_iceoryx2_zero_copy_with_protobuf() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-pb-{}", std::process::id());
    let zenoh_authority = format!("zenoh-{unique}");
    let iceoryx_authority = format!("iceoryx-{unique}");
    let topic = make_topic(&zenoh_authority, 0x9103);
    let zenoh = zenoh_transport(&zenoh_authority).await;
    let iceoryx2_egress = iceoryx2_transport();
    let iceoryx2_receiver = iceoryx2_transport();
    let (tx, mut rx) = mpsc::unbounded_channel();

    iceoryx2_receiver
        .register_zero_copy_listener(&topic, None, Arc::new(ZeroCopyFrameSender(tx)))
        .await
        .expect("iceoryx2 receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-pb",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&iceoryx_authority, 0xA103),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh.clone()),
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2_egress,
            ),
        )
        .await
        .expect("route should register");

    let payload = protobuf_payload("protobuf zenoh-to-iox");
    zenoh
        .send_serialized::<ProtobufPayload, _>(UFrameMetadata::publish(topic), &payload)
        .await
        .expect("zenoh protobuf send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    let decoded: StringValue = frame
        .deserialize::<ProtobufPayload, _>()
        .expect("protobuf payload should decode");

    assert_eq!(
        frame.metadata().encoding(),
        Some(&ProtobufPayload::encoding())
    );
    assert_eq!(decoded.value, payload.value);
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_zenoh_owned_to_real_iceoryx2_zero_copy_with_protobuf_umessage_frame_payload() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-outer-pb-{}", std::process::id());
    let zenoh_authority = format!("zenoh-{unique}");
    let iceoryx_authority = format!("iceoryx-{unique}");
    let topic = make_topic(&zenoh_authority, 0x910A);
    let zenoh = zenoh_transport(&zenoh_authority).await;
    let iceoryx2_egress = iceoryx2_transport();
    let iceoryx2_receiver = iceoryx2_transport();
    let (tx, mut rx) = mpsc::unbounded_channel();

    iceoryx2_receiver
        .register_zero_copy_listener(&topic, None, Arc::new(ZeroCopyFrameSender(tx)))
        .await
        .expect("iceoryx2 receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-outer-pb",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&iceoryx_authority, 0xA10A),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh.clone()),
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2_egress,
            ),
        )
        .await
        .expect("route should register");

    let payload = protobuf_payload("protobuf payload inside streamed protobuf UMessage frame");
    let inner_frame = UOwnedFrame::from_serializable::<ProtobufPayload, _>(
        UFrameMetadata::publish(make_topic("inner", 0x910A)),
        &payload,
    )
    .expect("inner protobuf payload should serialize");
    let envelope = ProtobufUMessageFrame::serialize_frame(&inner_frame)
        .expect("outer protobuf UMessage frame should serialize");
    zenoh
        .send_serialized::<RawBytes, _>(UFrameMetadata::publish(topic), &envelope)
        .await
        .expect("zenoh raw envelope send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    let wrong_layer = frame.deserialize::<ProtobufPayload, StringValue>();
    let decoded_frame = ProtobufUMessageFrame::deserialize_frame(frame.payload_bytes())
        .expect("outer UMessage frame should decode");
    let decoded_payload: StringValue = decoded_frame
        .deserialize::<ProtobufPayload, _>()
        .expect("inner protobuf payload should decode after outer frame decode");

    assert_eq!(frame.metadata().encoding(), Some(&RawBytes::encoding()));
    assert_eq!(frame.payload_bytes(), envelope.as_ref());
    assert!(matches!(
        wrong_layer,
        Err(UWireError::UnsupportedEncoding { .. })
    ));
    assert_eq!(decoded_payload.value, payload.value);
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_iceoryx2_zero_copy_to_real_zenoh_owned() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-reverse-{}", std::process::id());
    let iceoryx_authority = format!("iceoryx-{unique}");
    let zenoh_authority = format!("zenoh-{unique}");
    let topic = make_topic(&iceoryx_authority, 0x9102);
    let iceoryx2 = iceoryx2_transport();
    let zenoh_egress = zenoh_transport(&zenoh_authority).await;
    let zenoh_receiver = zenoh_transport(&zenoh_authority).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    zenoh_receiver
        .register_owned_listener(&topic, None, Arc::new(OwnedFrameSender(tx)))
        .await
        .expect("zenoh receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-reverse",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&zenoh_authority, 0xA102),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2.clone(),
            ),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let (header, id) = metadata_header(topic.clone());
    iceoryx2
        .send_serialized_zero_copy::<RawBytes, _>(header, &&b"iox-to-zenoh"[..])
        .await
        .expect("iceoryx2 send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    assert_eq!(frame.payload_bytes(), b"iox-to-zenoh");
    assert_streamed_metadata(&frame, &topic, &id);
}

#[tokio::test(flavor = "multi_thread")]
async fn iceoryx2_ingress_fans_out_to_streamer_and_local_listener() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-fanout-{}", std::process::id());
    let iceoryx_authority = format!("iceoryx-{unique}");
    let zenoh_authority = format!("zenoh-{unique}");
    let topic = make_topic(&iceoryx_authority, 0x9107);
    let iceoryx2 = iceoryx2_transport();
    let zenoh_egress = zenoh_transport(&zenoh_authority).await;
    let zenoh_receiver = zenoh_transport(&zenoh_authority).await;
    let (local_tx, mut local_rx) = mpsc::unbounded_channel();
    let (streamed_tx, mut streamed_rx) = mpsc::unbounded_channel();

    iceoryx2
        .register_zero_copy_listener(&topic, None, Arc::new(ZeroCopyFrameSender(local_tx)))
        .await
        .expect("local iceoryx2 listener should register");
    zenoh_receiver
        .register_owned_listener(&topic, None, Arc::new(OwnedFrameSender(streamed_tx)))
        .await
        .expect("zenoh receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-fanout",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&zenoh_authority, 0xA107),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2.clone(),
            ),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let (header, id) = metadata_header(topic.clone());
    iceoryx2
        .send_serialized_zero_copy::<RawBytes, _>(header, &&b"iox-fanout"[..])
        .await
        .expect("iceoryx2 send should succeed");

    let local_frame = tokio::time::timeout(Duration::from_secs(5), local_rx.recv())
        .await
        .expect("local receive should not time out")
        .expect("local receiver should remain open");
    let streamed_frame = tokio::time::timeout(Duration::from_secs(5), streamed_rx.recv())
        .await
        .expect("streamed receive should not time out")
        .expect("streamed receiver should remain open");

    assert_eq!(local_frame.payload_bytes(), b"iox-fanout");
    assert_eq!(streamed_frame.payload_bytes(), b"iox-fanout");
    assert_streamed_metadata(&local_frame, &topic, &id);
    assert_streamed_metadata(&streamed_frame, &topic, &id);
}

#[cfg(feature = "lola-transport")]
#[tokio::test(flavor = "multi_thread")]
async fn lola_publish_ingress_fans_out_to_streamer_and_local_listener() {
    let _actual_guard = actual_transport_guard().await;
    let _guard = lola_streamer_guard().await;
    let unique = format!("native-streamer-lola-pub-{}", std::process::id());
    let lola_authority = format!("lola-{unique}");
    let zenoh_authority = format!("zenoh-{unique}");
    let topic = make_topic(&lola_authority, 0x9108);
    let lola = lola_transport(&lola_authority);
    let zenoh_egress = zenoh_transport(&zenoh_authority).await;
    let zenoh_receiver = zenoh_transport(&zenoh_authority).await;
    let (local_tx, mut local_rx) = mpsc::unbounded_channel();
    let (streamed_tx, mut streamed_rx) = mpsc::unbounded_channel();
    let local_listener: Arc<dyn UZeroCopyListener<LolaRxLease>> =
        Arc::new(ZeroCopyFrameSender(local_tx));

    lola.register_zero_copy_listener(&topic, None, local_listener.clone())
        .await
        .expect("local LoLa listener should register");
    zenoh_receiver
        .register_owned_listener(&topic, None, Arc::new(OwnedFrameSender(streamed_tx)))
        .await
        .expect("zenoh receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-lola-publish-fanout",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&zenoh_authority, 0xA108),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "lola",
                &lola_authority,
                lola.clone(),
            ),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let (header, id) = metadata_header(topic.clone());
    lola.send_serialized_zero_copy::<RawBytes, _>(header, &&b"lola-publish-fanout"[..])
        .await
        .expect("LoLa send should succeed");

    let local_frame = tokio::time::timeout(Duration::from_secs(5), local_rx.recv())
        .await
        .expect("local receive should not time out")
        .expect("local receiver should remain open");
    let streamed_frame = tokio::time::timeout(Duration::from_secs(5), streamed_rx.recv())
        .await
        .expect("streamed receive should not time out")
        .expect("streamed receiver should remain open");

    assert_eq!(local_frame.payload_bytes(), b"lola-publish-fanout");
    assert_eq!(streamed_frame.payload_bytes(), b"lola-publish-fanout");
    assert_streamed_metadata(&local_frame, &topic, &id);
    assert_streamed_metadata(&streamed_frame, &topic, &id);

    lola.unregister_zero_copy_listener(&topic, None, local_listener)
        .await
        .expect("local LoLa listener should unregister");
}

#[cfg(feature = "lola-transport")]
#[tokio::test(flavor = "multi_thread")]
async fn lola_targeted_ingress_fans_out_to_streamer_and_local_listener() {
    let _actual_guard = actual_transport_guard().await;
    let _guard = lola_streamer_guard().await;
    let unique = format!("native-streamer-lola-p2p-{}", std::process::id());
    let lola_authority = format!("lola-{unique}");
    let zenoh_authority = format!("zenoh-{unique}");
    let source = make_topic(&lola_authority, 0x9109);
    let sink = UUri::try_from_parts(&zenoh_authority, 0x4220, 1, 0).expect("valid sink");
    let lola = lola_transport(&lola_authority);
    let zenoh_egress = zenoh_transport(&zenoh_authority).await;
    let zenoh_receiver = zenoh_transport(&zenoh_authority).await;
    let (local_tx, mut local_rx) = mpsc::unbounded_channel();
    let (streamed_tx, mut streamed_rx) = mpsc::unbounded_channel();
    let local_listener: Arc<dyn UZeroCopyListener<LolaRxLease>> =
        Arc::new(ZeroCopyFrameSender(local_tx));

    lola.register_zero_copy_listener(&source, Some(&sink), local_listener.clone())
        .await
        .expect("local LoLa listener should register");
    zenoh_receiver
        .register_owned_listener(
            &source,
            Some(&sink),
            Arc::new(OwnedFrameSender(streamed_tx)),
        )
        .await
        .expect("zenoh receiver listener should register");

    let mut streamer = UStreamer::new("actual-lola-targeted-fanout", 16, subscriptions(Vec::new()))
        .await
        .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "lola",
                &lola_authority,
                lola.clone(),
            ),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let id = UUID::build();
    let attributes = UAttributes::new(
        id.clone(),
        source.clone(),
        Some(sink.clone()),
        UMessageType::Notification,
    )
    .with_priority(UPriority::CS5)
    .with_ttl(3_000)
    .with_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00");
    lola.send_serialized_zero_copy::<RawBytes, _>(
        UFrameMetadata::new(attributes, RawBytes::encoding()),
        &&b"lola-targeted-fanout"[..],
    )
    .await
    .expect("LoLa send should succeed");

    let local_frame = tokio::time::timeout(Duration::from_secs(5), local_rx.recv())
        .await
        .expect("local receive should not time out")
        .expect("local receiver should remain open");
    let streamed_frame = tokio::time::timeout(Duration::from_secs(5), streamed_rx.recv())
        .await
        .expect("streamed receive should not time out")
        .expect("streamed receiver should remain open");

    assert_eq!(local_frame.payload_bytes(), b"lola-targeted-fanout");
    assert_eq!(streamed_frame.payload_bytes(), b"lola-targeted-fanout");
    assert_eq!(local_frame.metadata().attributes().id(), &id);
    assert_eq!(streamed_frame.metadata().attributes().id(), &id);
    assert_eq!(local_frame.metadata().attributes().source(), &source);
    assert_eq!(streamed_frame.metadata().attributes().source(), &source);
    assert_eq!(local_frame.metadata().attributes().sink(), Some(&sink));
    assert_eq!(streamed_frame.metadata().attributes().sink(), Some(&sink));

    lola.unregister_zero_copy_listener(&source, Some(&sink), local_listener)
        .await
        .expect("local LoLa listener should unregister");
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_iceoryx2_zero_copy_to_real_zenoh_owned_with_protobuf() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-reverse-pb-{}", std::process::id());
    let iceoryx_authority = format!("iceoryx-{unique}");
    let zenoh_authority = format!("zenoh-{unique}");
    let topic = make_topic(&iceoryx_authority, 0x9104);
    let iceoryx2 = iceoryx2_transport();
    let zenoh_egress = zenoh_transport(&zenoh_authority).await;
    let zenoh_receiver = zenoh_transport(&zenoh_authority).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    zenoh_receiver
        .register_owned_listener(&topic, None, Arc::new(OwnedFrameSender(tx)))
        .await
        .expect("zenoh receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-reverse-pb",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&zenoh_authority, 0xA104),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2.clone(),
            ),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let payload = protobuf_payload("protobuf iox-to-zenoh");
    iceoryx2
        .send_serialized_zero_copy::<ProtobufPayload, _>(UFrameMetadata::publish(topic), &payload)
        .await
        .expect("iceoryx2 protobuf send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    let decoded: StringValue = frame
        .deserialize::<ProtobufPayload, _>()
        .expect("protobuf payload should decode");

    assert_eq!(
        frame.metadata().encoding(),
        Some(&ProtobufPayload::encoding())
    );
    assert_eq!(decoded.value, payload.value);
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_zenoh_zero_copy_to_real_iceoryx2_zero_copy() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-zc-{}", std::process::id());
    let zenoh_authority = format!("zenoh-{unique}");
    let iceoryx_authority = format!("iceoryx-{unique}");
    let topic = make_topic(&zenoh_authority, 0x9105);
    let zenoh = zenoh_transport(&zenoh_authority).await;
    let iceoryx2_egress = iceoryx2_transport();
    let iceoryx2_receiver = iceoryx2_transport();
    let (tx, mut rx) = mpsc::unbounded_channel();

    iceoryx2_receiver
        .register_zero_copy_listener(&topic, None, Arc::new(ZeroCopyFrameSender(tx)))
        .await
        .expect("iceoryx2 receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-zc-zc",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&iceoryx_authority, 0xA105),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "zenoh-zc",
                &zenoh_authority,
                zenoh.clone(),
            ),
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2_egress,
            ),
        )
        .await
        .expect("route should register");

    let (header, id) = metadata_header(topic.clone());
    zenoh
        .send_serialized_zero_copy::<RawBytes, _>(header, &&b"zenoh-zc-to-iox"[..])
        .await
        .expect("zenoh zero-copy send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    assert_eq!(frame.payload_bytes(), b"zenoh-zc-to-iox");
    assert_streamed_metadata(&frame, &topic, &id);
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_iceoryx2_zero_copy_to_real_zenoh_zero_copy() {
    let _actual_guard = actual_transport_guard().await;
    let unique = format!("native-streamer-zc-reverse-{}", std::process::id());
    let iceoryx_authority = format!("iceoryx-{unique}");
    let zenoh_authority = format!("zenoh-{unique}");
    let topic = make_topic(&iceoryx_authority, 0x9106);
    let iceoryx2 = iceoryx2_transport();
    let zenoh_egress = zenoh_transport(&zenoh_authority).await;
    let zenoh_receiver = zenoh_transport(&zenoh_authority).await;
    let (tx, mut rx) = mpsc::unbounded_channel();

    zenoh_receiver
        .register_zero_copy_listener(&topic, None, Arc::new(ZeroCopyFrameSender(tx)))
        .await
        .expect("zenoh zero-copy receiver listener should register");

    let mut streamer = UStreamer::new(
        "actual-zc-zc-reverse",
        16,
        subscriptions(vec![subscription(
            topic.clone(),
            make_topic(&zenoh_authority, 0xA106),
        )]),
    )
    .await
    .expect("streamer should build");
    streamer
        .add_route_ref(
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "iceoryx2",
                &iceoryx_authority,
                iceoryx2.clone(),
            ),
            &OwnedFrameEndpoint::from_zero_copy_copying_adapter(
                "zenoh-zc",
                &zenoh_authority,
                zenoh_egress,
            ),
        )
        .await
        .expect("route should register");

    let (header, id) = metadata_header(topic.clone());
    iceoryx2
        .send_serialized_zero_copy::<RawBytes, _>(header, &&b"iox-to-zenoh-zc"[..])
        .await
        .expect("iceoryx2 zero-copy send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    assert_eq!(frame.payload_bytes(), b"iox-to-zenoh-zc");
    assert_streamed_metadata(&frame, &topic, &id);
}
