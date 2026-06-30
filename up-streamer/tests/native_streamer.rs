#![cfg(feature = "owned-frame-transport")]

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, timeout, Duration};
use up_rust::communication::{
    owned, CallOptions, RequestHandler, ServiceInvocationError, SubscriptionStatus, UPayload,
};
use up_rust::core::usubscription::{ResetReason, SubscriptionInfo, USubscription};
use up_rust::{
    try_project_umessage_to_frame_metadata, LocalUriProvider, StaticUriProvider, UAttributes,
    UCode, UListener, UMessage, UMessageBuilder, UOwnedFrame, UOwnedListener, UOwnedTransportImpl,
    UPayloadFormat, UStatus, UUri, ValidatedOwnedFrame,
};
use up_streamer::{OwnedFrameEndpoint, RouteCopySemantics, RouteKind, UStreamer};

#[derive(Default)]
struct EmptySubscription;

#[async_trait]
impl USubscription for EmptySubscription {
    async fn subscribe(
        &self,
        _topic: &UUri,
        _expiration: Option<u64>,
        _min_sample_period: Option<u32>,
    ) -> Result<SubscriptionStatus, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "subscribe is not used by this test",
        ))
    }

    async fn unsubscribe(&self, _topic: &UUri) -> Result<(), UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "unsubscribe is not used by this test",
        ))
    }

    async fn fetch_subscriptions_by_topic(
        &self,
        _topic: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Ok(Vec::new())
    }

    async fn fetch_subscriptions_by_subscriber(
        &self,
        _subscriber: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "fetch_subscriptions_by_subscriber is not used by this test",
        ))
    }

    async fn register_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn unregister_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn fetch_subscribers(&self, _topic: &UUri) -> Result<Vec<UUri>, UStatus> {
        Ok(Vec::new())
    }

    async fn reset(
        &self,
        _reason: ResetReason,
        _message: Option<String>,
        _before: Option<u64>,
    ) -> Result<(), UStatus> {
        Ok(())
    }
}

struct SeededSubscription {
    subscriptions: Vec<SubscriptionInfo>,
}

impl SeededSubscription {
    fn new(subscriptions: Vec<SubscriptionInfo>) -> Self {
        Self { subscriptions }
    }
}

#[async_trait]
impl USubscription for SeededSubscription {
    async fn subscribe(
        &self,
        _topic: &UUri,
        _expiration: Option<u64>,
        _min_sample_period: Option<u32>,
    ) -> Result<SubscriptionStatus, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "subscribe is not used by this test",
        ))
    }

    async fn unsubscribe(&self, _topic: &UUri) -> Result<(), UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "unsubscribe is not used by this test",
        ))
    }

    async fn fetch_subscriptions_by_topic(
        &self,
        _topic: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Ok(self.subscriptions.clone())
    }

    async fn fetch_subscriptions_by_subscriber(
        &self,
        _subscriber: &UUri,
    ) -> Result<Vec<SubscriptionInfo>, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "fetch_subscriptions_by_subscriber is not used by this test",
        ))
    }

    async fn register_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn unregister_for_notifications(&self, _topic: &UUri) -> Result<(), UStatus> {
        Ok(())
    }

    async fn fetch_subscribers(&self, _topic: &UUri) -> Result<Vec<UUri>, UStatus> {
        Ok(Vec::new())
    }

    async fn reset(
        &self,
        _reason: ResetReason,
        _message: Option<String>,
        _before: Option<u64>,
    ) -> Result<(), UStatus> {
        Ok(())
    }
}

#[derive(Clone)]
struct RegisteredOwnedListener {
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UOwnedListener>,
}

impl RegisteredOwnedListener {
    fn listener_id(listener: &Arc<dyn UOwnedListener>) -> usize {
        Arc::as_ptr(listener) as *const () as usize
    }

    fn matches(&self, frame: &UOwnedFrame) -> bool {
        let attributes = frame.metadata().attributes();
        if !self.source_filter.matches(attributes.source()) {
            return false;
        }
        match (&self.sink_filter, attributes.sink()) {
            (Some(pattern), Some(candidate)) => pattern.matches(candidate),
            (None, None) => true,
            _ => false,
        }
    }

    fn same_registration(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: &Arc<dyn UOwnedListener>,
    ) -> bool {
        self.source_filter == *source_filter
            && self.sink_filter.as_ref() == sink_filter
            && Self::listener_id(&self.listener) == Self::listener_id(listener)
    }
}

#[derive(Default)]
struct RecordingOwnedTransport {
    sent: Mutex<Vec<UOwnedFrame>>,
    received: Mutex<Vec<UOwnedFrame>>,
    listeners: Mutex<Vec<RegisteredOwnedListener>>,
    unregister_count: AtomicUsize,
}

impl RecordingOwnedTransport {
    fn first_listener(&self) -> Arc<dyn UOwnedListener> {
        self.listeners
            .lock()
            .expect("listener lock")
            .first()
            .expect("route should register a listener")
            .listener
            .clone()
    }

    fn sent_frames(&self) -> Vec<UOwnedFrame> {
        self.sent.lock().expect("sent lock").clone()
    }

    async fn wait_for_sent_count(&self, expected: usize) {
        for _ in 0..20 {
            if self.sent.lock().expect("sent lock").len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    async fn dispatch(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listener lock")
            .iter()
            .filter(|listener| listener.matches(&frame))
            .map(|listener| listener.listener.clone())
            .collect::<Vec<_>>();
        for listener in listeners {
            listener.on_receive_owned(frame.clone()).await;
        }
    }
}

#[async_trait]
impl UOwnedTransportImpl for RecordingOwnedTransport {
    async fn send_validated_owned(&self, frame: ValidatedOwnedFrame) -> Result<(), UStatus> {
        let frame = frame.into_inner();
        self.sent.lock().expect("sent lock").push(frame.clone());
        self.received
            .lock()
            .expect("received lock")
            .push(frame.clone());
        self.dispatch(frame).await;
        Ok(())
    }

    async fn receive_validated_owned(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
    ) -> Result<UOwnedFrame, UStatus> {
        for _ in 0..100 {
            let maybe_frame = {
                let mut received = self.received.lock().expect("received lock");
                received
                    .iter()
                    .position(|frame| {
                        let attributes = frame.metadata().attributes();
                        source_filter.matches(attributes.source())
                            && match (sink_filter, attributes.sink()) {
                                (Some(pattern), Some(candidate)) => pattern.matches(candidate),
                                (None, None) => true,
                                _ => false,
                            }
                    })
                    .map(|index| received.remove(index))
            };
            if let Some(frame) = maybe_frame {
                return Ok(frame);
            }
            sleep(Duration::from_millis(10)).await;
        }
        Err(UStatus::fail_with_code(
            UCode::NotFound,
            "no matching owned frame",
        ))
    }

    async fn register_validated_owned_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listener lock")
            .push(RegisteredOwnedListener {
                source_filter: source_filter.clone(),
                sink_filter: sink_filter.cloned(),
                listener,
            });
        Ok(())
    }

    async fn unregister_validated_owned_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listener lock")
            .retain(|registered| {
                !registered.same_registration(source_filter, sink_filter, &listener)
            });
        self.unregister_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingMessageListener {
    messages: Mutex<Vec<UMessage>>,
}

impl RecordingMessageListener {
    async fn wait_for_message_count(&self, expected: usize) {
        for _ in 0..100 {
            if self.messages.lock().expect("messages lock").len() >= expected {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    fn messages(&self) -> Vec<UMessage> {
        self.messages.lock().expect("messages lock").clone()
    }
}

#[async_trait]
impl UListener for RecordingMessageListener {
    async fn on_receive(&self, msg: UMessage) {
        self.messages.lock().expect("messages lock").push(msg);
    }
}

struct EchoRequestHandler;

#[async_trait]
impl RequestHandler for EchoRequestHandler {
    async fn handle_request(
        &self,
        resource_id: u16,
        message_attributes: &UAttributes,
        request_payload: Option<UPayload>,
    ) -> Result<Option<UPayload>, ServiceInvocationError> {
        assert_eq!(resource_id, 0x1000);
        assert!(message_attributes.is_request());
        Ok(request_payload)
    }
}

fn uri_provider(authority: &str) -> Arc<StaticUriProvider> {
    Arc::new(StaticUriProvider::new(authority, 0x5BA0, 0x01).expect("uri provider"))
}

fn topic_uri() -> UUri {
    UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic")
}

fn subscriber_uri() -> UUri {
    UUri::try_from_parts("authority-b", 0x5BA0, 0x01, 0x0000).expect("subscriber")
}

fn seeded_publish_subscription() -> Arc<dyn USubscription> {
    Arc::new(SeededSubscription::new(vec![SubscriptionInfo::new(
        topic_uri(),
        subscriber_uri(),
        SubscriptionStatus::Subscribed,
        None,
        None,
    )]))
}

fn owned_payload_frame() -> UOwnedFrame {
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(
        Bytes::from_static(b"protobuf-payload"),
        UPayloadFormat::Protobuf,
    )
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UOwnedFrame::new(metadata, message.payload()).expect("owned frame")
}

#[tokio::test]
async fn owned_route_forwards_standard_payload_as_owned_frame() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint =
        OwnedFrameEndpoint::from_owned("ingress", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("owned-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route add");

    let diagnostics = streamer.route_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].route.ingress_name, "ingress");
    assert_eq!(diagnostics[0].route.ingress_authority, "authority-a");
    assert_eq!(diagnostics[0].route.egress_name, "egress");
    assert_eq!(diagnostics[0].route.egress_authority, "authority-b");
    assert_eq!(
        diagnostics[0].route_kind,
        RouteKind::OwnedFrameCompatibility
    );
    assert_eq!(
        diagnostics[0].copy_semantics,
        RouteCopySemantics::OwnedOrMessageCopying
    );

    let frame = owned_payload_frame();
    ingress
        .first_listener()
        .on_receive_owned(frame.clone())
        .await;
    egress.wait_for_sent_count(1).await;

    let sent = egress.sent_frames();
    assert_eq!(sent, vec![frame]);
}

#[tokio::test]
async fn owned_l2_publish_routes_through_streamer() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint =
        OwnedFrameEndpoint::from_owned("ingress", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("owned-l2-publish", 4, seeded_publish_subscription())
        .await
        .expect("streamer");
    let publisher = owned::Endpoint::new(ingress.clone(), uri_provider("authority-a")).publisher();
    let subscriber = owned::Endpoint::new(egress.clone(), uri_provider("authority-b")).subscriber();
    let listener = Arc::new(RecordingMessageListener::default());

    subscriber
        .subscribe(&topic_uri(), listener.clone(), None)
        .await
        .expect("owned subscriber registered");
    streamer
        .add_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route add");

    publisher
        .publish(
            0x8001,
            CallOptions::for_publish(None, None, None),
            Some(UPayload::new("owned publish", UPayloadFormat::Text)),
        )
        .await
        .expect("owned publish succeeds");
    listener.wait_for_message_count(1).await;

    let messages = listener.messages();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].is_publish());
    assert_eq!(messages[0].source(), &topic_uri());
    assert_eq!(messages[0].sink(), None);
    assert_eq!(
        messages[0].payload(),
        Some(Bytes::from_static(b"owned publish"))
    );
    assert_eq!(ingress.sent_frames().len(), 1);
    assert_eq!(egress.sent_frames().len(), 1);
}

#[tokio::test]
async fn owned_l2_notifications_route_through_streamer() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint =
        OwnedFrameEndpoint::from_owned("ingress", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("owned-l2-notification", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");
    let ingress_l2 = owned::Endpoint::new(ingress.clone(), uri_provider("authority-a"));
    let egress_uri_provider = uri_provider("authority-b");
    let egress_l2 = owned::Endpoint::new(egress.clone(), egress_uri_provider.clone());
    let destination = egress_uri_provider.get_source_uri();
    let listener = Arc::new(RecordingMessageListener::default());

    egress_l2
        .notifier()
        .start_listening(&topic_uri(), listener.clone())
        .await
        .expect("owned notifier listener registered");
    streamer
        .add_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route add");

    ingress_l2
        .notifier()
        .notify(
            0x8001,
            &destination,
            CallOptions::for_notification(None, None, None),
            Some(UPayload::new("owned notification", UPayloadFormat::Text)),
        )
        .await
        .expect("owned notification succeeds");
    listener.wait_for_message_count(1).await;

    let messages = listener.messages();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].is_notification());
    assert_eq!(messages[0].source(), &topic_uri());
    assert_eq!(messages[0].sink_unchecked(), &destination);
    assert_eq!(
        messages[0].payload(),
        Some(Bytes::from_static(b"owned notification"))
    );
    assert_eq!(ingress.sent_frames().len(), 1);
    assert_eq!(egress.sent_frames().len(), 1);
}

#[tokio::test]
async fn owned_l2_rpc_routes_through_streamer() {
    let client_transport = Arc::new(RecordingOwnedTransport::default());
    let server_transport = Arc::new(RecordingOwnedTransport::default());
    let client_endpoint =
        OwnedFrameEndpoint::from_owned("client", "authority-a", client_transport.clone());
    let server_endpoint =
        OwnedFrameEndpoint::from_owned("server", "authority-b", server_transport.clone());
    let mut streamer = UStreamer::new("owned-l2-rpc", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");
    let client_l2 = owned::Endpoint::new(client_transport.clone(), uri_provider("authority-a"));
    let server_l2 = owned::Endpoint::new(server_transport.clone(), uri_provider("authority-b"));

    streamer
        .add_owned_route_ref(&client_endpoint, &server_endpoint)
        .await
        .expect("client to server route add");
    streamer
        .add_owned_route_ref(&server_endpoint, &client_endpoint)
        .await
        .expect("server to client route add");
    server_l2
        .rpc_server()
        .register_endpoint(None, 0x1000, Arc::new(EchoRequestHandler))
        .await
        .expect("owned RPC endpoint registered");

    let response = timeout(
        Duration::from_secs(2),
        client_l2.rpc_client().invoke_method(
            uri_provider("authority-b").get_resource_uri(0x1000),
            CallOptions::for_rpc_request(5_000, None, None, None),
            Some(UPayload::new("owned rpc", UPayloadFormat::Text)),
        ),
    )
    .await
    .expect("owned RPC should not hang")
    .expect("owned RPC succeeds")
    .expect("owned RPC response payload");

    assert_eq!(response.payload(), &Bytes::from_static(b"owned rpc"));
    assert_eq!(client_transport.sent_frames().len(), 2);
    assert_eq!(server_transport.sent_frames().len(), 2);
}

#[tokio::test]
async fn delete_owned_route_unregisters_owned_listener() {
    let ingress = Arc::new(RecordingOwnedTransport::default());
    let egress = Arc::new(RecordingOwnedTransport::default());
    let ingress_endpoint =
        OwnedFrameEndpoint::from_owned("ingress", "authority-a", ingress.clone());
    let egress_endpoint = OwnedFrameEndpoint::from_owned("egress", "authority-b", egress);
    let mut streamer = UStreamer::new("owned-route-delete", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route add");
    streamer
        .delete_owned_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("owned route delete");

    assert_eq!(ingress.unregister_count.load(Ordering::Relaxed), 1);
    assert!(streamer.route_diagnostics().is_empty());
}
