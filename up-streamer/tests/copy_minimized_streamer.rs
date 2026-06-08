#![cfg(feature = "experimental-copy-minimized-routing")]

use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use up_rust::core::usubscription::{
    ResetReason, SubscriptionInfo, SubscriptionStatus, USubscription,
};
use up_rust::{
    try_project_umessage_to_frame_metadata, InMemoryZeroCopyTransport, UCode, UFrameView,
    UMessageBuilder, UPayloadFormat, UStatus, UUri, UVecRxLease,
};
use up_rust::{UZeroCopyRxLease, UZeroCopyTransport};
use up_streamer::{
    CopyMinimizedRouteOptions, RouteCopySemantics, RouteKind, UStreamer, ZeroCopyFrameEndpoint,
};
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
use up_transport_zenoh::UPTransportZenoh;

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

fn payload_frame(payload: &'static [u8]) -> UVecRxLease {
    let message = UMessageBuilder::publish(
        UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("topic"),
    )
    .build_with_payload(Bytes::from_static(payload), UPayloadFormat::Protobuf)
    .expect("message");
    let metadata = try_project_umessage_to_frame_metadata(&message).expect("metadata");
    UVecRxLease::new(metadata, Some(payload.to_vec())).expect("zero-copy frame")
}

async fn wait_for_sent_count(transport: &InMemoryZeroCopyTransport, expected: usize) {
    for _ in 0..20 {
        if transport.sent_frames().len() >= expected {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

fn assert_route_pair<I, E>()
where
    I: UZeroCopyTransport + Send + Sync + 'static,
    I::Rx: UZeroCopyRxLease + Send + 'static,
    E: UZeroCopyTransport + Send + Sync + 'static,
{
    let _ = std::mem::size_of::<ZeroCopyFrameEndpoint<I>>();
    let _ = std::mem::size_of::<ZeroCopyFrameEndpoint<E>>();
}

#[test]
fn selected_zenoh_iceoryx2_route_pairs_typecheck() {
    assert_route_pair::<UPTransportZenoh, UPTransportZenoh>();
    assert_route_pair::<UPTransportZenoh, Iceoryx2PubSub>();
    assert_route_pair::<Iceoryx2PubSub, UPTransportZenoh>();
    assert_route_pair::<Iceoryx2PubSub, Iceoryx2PubSub>();
}

#[tokio::test]
async fn copy_minimized_route_copies_payload_once_into_egress_loan() {
    let ingress = Arc::new(InMemoryZeroCopyTransport::default());
    let egress = Arc::new(InMemoryZeroCopyTransport::default());
    let ingress_endpoint = ZeroCopyFrameEndpoint::new("ingress", "authority-a", ingress.clone());
    let egress_endpoint = ZeroCopyFrameEndpoint::new("egress", "authority-b", egress.clone());
    let mut streamer = UStreamer::new("copy-minimized-route", 4, Arc::new(EmptySubscription))
        .await
        .expect("streamer");

    streamer
        .add_copy_minimized_route_ref_with_options(
            &ingress_endpoint,
            &egress_endpoint,
            CopyMinimizedRouteOptions {
                payload_alignment: 8,
            },
        )
        .await
        .expect("copy-minimized route add");

    let diagnostics = streamer.route_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].route.ingress_name, "ingress");
    assert_eq!(diagnostics[0].route.ingress_authority, "authority-a");
    assert_eq!(diagnostics[0].route.egress_name, "egress");
    assert_eq!(diagnostics[0].route.egress_authority, "authority-b");
    assert_eq!(diagnostics[0].route_kind, RouteKind::CopyMinimized);
    assert_eq!(
        diagnostics[0].copy_semantics,
        RouteCopySemantics::CopyMinimizedOneCopy
    );

    ingress.inject(payload_frame(b"copy-minimized")).await;
    wait_for_sent_count(&egress, 1).await;

    let sent = egress.sent_frames();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].try_contiguous_payload(),
        Some(&b"copy-minimized"[..])
    );

    streamer
        .delete_copy_minimized_route_ref(&ingress_endpoint, &egress_endpoint)
        .await
        .expect("copy-minimized route delete");
    assert!(streamer.route_diagnostics().is_empty());

    ingress.inject(payload_frame(b"after-delete")).await;
    sleep(Duration::from_millis(20)).await;
    assert_eq!(egress.sent_frames().len(), 1);
}
