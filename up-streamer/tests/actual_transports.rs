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

use std::sync::Arc;
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
    ProtobufWire, RawBytes, UAttributes, UCode, UFrameMetadata, UMessageType, UOwnedFrame,
    UOwnedListener, UOwnedTransport, UOwnedTransportExt, UPriority, UStatus, UUri,
    UZeroCopyListener, UZeroCopyRxFrame, UZeroCopyTransport, UZeroCopyTransportExt, WireFormat,
    UUID,
};
use up_streamer::{OwnedFrameEndpoint, UStreamer};
use up_transport_iceoryx2_rust::{
    transport::UTransportIceoryx2, Iceoryx2PubSub, Iceoryx2RxLease, MessagingPattern,
};
use up_transport_zenoh::{zenoh_config::Config as ZenohConfig, UPTransportZenoh};

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
impl UZeroCopyListener<Iceoryx2RxLease> for ZeroCopyFrameSender {
    async fn on_receive_zero_copy(&self, frame: Iceoryx2RxLease) {
        let _ = self.0.send(UOwnedFrame::new(
            frame.metadata().clone(),
            frame.payload().to_vec(),
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

fn metadata_header(topic: UUri, token: &str, commstatus: UCode) -> (UFrameMetadata, UUID, UUID) {
    let id = UUID::build();
    let request_id = UUID::build();
    let attributes = UAttributes::new(id.clone(), topic, None, UMessageType::Publish)
        .with_priority(UPriority::CS5)
        .with_ttl(3_000)
        .with_request_id(request_id.clone())
        .with_traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
        .with_token(token)
        .with_permission_level(11)
        .with_comm_status(commstatus);
    (
        UFrameMetadata::new(attributes, RawBytes::encoding()),
        id,
        request_id,
    )
}

fn assert_streamed_metadata(
    frame: &UOwnedFrame,
    topic: &UUri,
    id: &UUID,
    request_id: &UUID,
    token: &str,
    commstatus: UCode,
) {
    let attributes = frame.metadata().attributes();
    assert_eq!(attributes.id(), id);
    assert_eq!(attributes.source(), topic);
    assert_eq!(attributes.sink(), None);
    assert_eq!(attributes.message_type(), UMessageType::Publish);
    assert_eq!(attributes.priority(), UPriority::CS5);
    assert_eq!(attributes.ttl(), Some(3_000));
    assert_eq!(attributes.request_id(), Some(request_id));
    assert_eq!(
        attributes.traceparent(),
        Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
    );
    assert_eq!(attributes.token(), Some(token));
    assert_eq!(attributes.permission_level(), Some(11));
    assert_eq!(attributes.commstatus(), Some(commstatus));
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

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_zenoh_owned_to_real_iceoryx2_zero_copy() {
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
            &OwnedFrameEndpoint::from_zero_copy("iceoryx2", &iceoryx_authority, iceoryx2_egress),
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

    let (header, id, request_id) =
        metadata_header(topic.clone(), "zenoh-to-iox-token", UCode::UNAVAILABLE);
    zenoh
        .send_serialized::<RawBytes, _>(header, &&b"zenoh-to-iox"[..])
        .await
        .expect("zenoh send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    assert_eq!(frame.payload_bytes(), b"zenoh-to-iox");
    assert_streamed_metadata(
        &frame,
        &topic,
        &id,
        &request_id,
        "zenoh-to-iox-token",
        UCode::UNAVAILABLE,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_zenoh_owned_to_real_iceoryx2_zero_copy_with_protobuf() {
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
            &OwnedFrameEndpoint::from_zero_copy("iceoryx2", &iceoryx_authority, iceoryx2_egress),
        )
        .await
        .expect("route should register");

    let payload = protobuf_payload("protobuf zenoh-to-iox");
    zenoh
        .send_serialized::<ProtobufWire, _>(UFrameMetadata::publish(topic), &payload)
        .await
        .expect("zenoh protobuf send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    let decoded: StringValue = frame
        .deserialize::<ProtobufWire, _>()
        .expect("protobuf payload should decode");

    assert_eq!(frame.metadata().encoding(), &ProtobufWire::encoding());
    assert_eq!(decoded.value, payload.value);
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_iceoryx2_zero_copy_to_real_zenoh_owned() {
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
            &OwnedFrameEndpoint::from_zero_copy("iceoryx2", &iceoryx_authority, iceoryx2.clone()),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let (header, id, request_id) = metadata_header(
        topic.clone(),
        "iox-to-zenoh-token",
        UCode::RESOURCE_EXHAUSTED,
    );
    iceoryx2
        .send_serialized_zero_copy::<RawBytes, _>(header, &&b"iox-to-zenoh"[..])
        .await
        .expect("iceoryx2 send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    assert_eq!(frame.payload_bytes(), b"iox-to-zenoh");
    assert_streamed_metadata(
        &frame,
        &topic,
        &id,
        &request_id,
        "iox-to-zenoh-token",
        UCode::RESOURCE_EXHAUSTED,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_real_iceoryx2_zero_copy_to_real_zenoh_owned_with_protobuf() {
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
            &OwnedFrameEndpoint::from_zero_copy("iceoryx2", &iceoryx_authority, iceoryx2.clone()),
            &OwnedFrameEndpoint::from_owned("zenoh", &zenoh_authority, zenoh_egress),
        )
        .await
        .expect("route should register");

    let payload = protobuf_payload("protobuf iox-to-zenoh");
    iceoryx2
        .send_serialized_zero_copy::<ProtobufWire, _>(UFrameMetadata::publish(topic), &payload)
        .await
        .expect("iceoryx2 protobuf send should succeed");

    let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("receive should not time out")
        .expect("receiver should remain open");
    let decoded: StringValue = frame
        .deserialize::<ProtobufWire, _>()
        .expect("protobuf payload should decode");

    assert_eq!(frame.metadata().encoding(), &ProtobufWire::encoding());
    assert_eq!(decoded.value, payload.value);
}
