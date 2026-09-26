// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0

use std::{sync::Arc, time::Duration};

use async_broadcast::broadcast;
use async_trait::async_trait;
use integration_test_utils::UPClientFoo;
use tokio::sync::mpsc;
use up_rust::{PayloadEncoding, UCode, UListener, UMessage, UMessageBuilder, UTransport, UUri};
use up_streamer::{Endpoint, UStreamer};
use usubscription_static_file::USubscriptionStaticFile;

struct Collect(mpsc::UnboundedSender<UMessage>);

#[async_trait]
impl UListener for Collect {
    async fn on_receive(&self, message: UMessage) {
        self.0.send(message).expect("collector remains alive");
    }
}

fn uri(authority: &str, resource: u16) -> UUri {
    UUri::try_from_parts(authority, 0x1234, 1, resource).unwrap()
}

fn notification(source: &str, sink: &str, sequence: u8) -> UMessage {
    UMessageBuilder::notification(uri(source, 0x8001), uri(sink, 0))
        .build_with_payload(vec![0, sequence, 0xFF, 0, sequence], PayloadEncoding::RAW)
        .unwrap()
}

async fn next(rx: &mut mpsc::UnboundedReceiver<UMessage>) -> UMessage {
    tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("delivery deadline")
        .expect("collector open")
}

async fn quiet(rx: &mut mpsc::UnboundedReceiver<UMessage>, duration: Duration) {
    assert!(
        tokio::time::timeout(duration, rx.recv()).await.is_err(),
        "unexpected delivery"
    );
}

async fn barrier(bus: &UPClientFoo, acknowledgements: &mut mpsc::UnboundedReceiver<UMessage>) {
    let marker = notification("control", "control", 0);
    bus.send(marker.clone()).await.unwrap();
    assert_eq!(next(acknowledgements).await, marker);
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_remote_bus_requires_bridge_and_route_removal_is_scoped() {
    let (local_tx, local_rx) = broadcast(128);
    let (remote_tx, remote_rx) = broadcast(128);
    let local = Arc::new(UPClientFoo::new("local", local_rx, local_tx).await);
    let remote = Arc::new(UPClientFoo::new("remote", remote_rx, remote_tx).await);
    let (tx, mut received) = mpsc::unbounded_channel();
    let listener: Arc<dyn UListener> = Arc::new(Collect(tx));
    let (tx, mut acknowledgements) = mpsc::unbounded_channel();
    let control: Arc<dyn UListener> = Arc::new(Collect(tx));
    for bus in [&local, &remote] {
        bus.register_listener(&UUri::any(), Some(&uri("control", 0)), control.clone())
            .await
            .unwrap();
    }
    local
        .register_listener(&UUri::any(), Some(&uri("local", 0)), listener.clone())
        .await
        .unwrap();
    for authority in ["remote-a", "remote-b"] {
        remote
            .register_listener(&UUri::any(), Some(&uri(authority, 0)), listener.clone())
            .await
            .unwrap();
    }

    local
        .send(notification("local", "remote-a", 100))
        .await
        .unwrap();
    remote
        .send(notification("remote-b", "local", 101))
        .await
        .unwrap();
    // Bus barriers prove the pre-bridge messages were processed before routes exist.
    barrier(&local, &mut acknowledgements).await;
    barrier(&remote, &mut acknowledgements).await;
    quiet(&mut received, Duration::from_millis(100)).await;

    let subscriptions = Arc::new(USubscriptionStaticFile::new(
        "../utils/usubscription-static-file/static-configs/testdata.json".into(),
    ));
    let mut streamer = UStreamer::new("shared-remote-controls", 32, subscriptions)
        .await
        .unwrap();
    let local_endpoint = Endpoint::new("local", "local", local.clone());
    let remote_a = Endpoint::new("remote-a", "remote-a", remote.clone());
    let remote_b = Endpoint::new("remote-b", "remote-b", remote.clone());
    for peer in [&remote_a, &remote_b] {
        streamer
            .add_route(local_endpoint.clone(), peer.clone())
            .await
            .unwrap();
        streamer
            .add_route(peer.clone(), local_endpoint.clone())
            .await
            .unwrap();
    }

    let mut expected = Vec::new();
    for sequence in 0..5 {
        for authority in ["remote-a", "remote-b"] {
            let outward = notification("local", authority, sequence);
            local.send(outward.clone()).await.unwrap();
            expected.push(outward);
            let inward = notification(authority, "local", sequence);
            remote.send(inward.clone()).await.unwrap();
            expected.push(inward);
        }
    }
    for _ in 0..20 {
        let message = next(&mut received).await;
        let index = expected
            .iter()
            .position(|sent| sent.attributes().id() == message.attributes().id())
            .expect("no duplicate or unsubmitted identity");
        assert_eq!(
            expected.swap_remove(index),
            message,
            "all metadata and bytes preserved"
        );
    }
    assert!(expected.is_empty());
    quiet(&mut received, Duration::from_millis(350)).await;

    streamer
        .delete_route(local_endpoint.clone(), remote_b.clone())
        .await
        .unwrap();
    streamer
        .delete_route(remote_b.clone(), local_endpoint.clone())
        .await
        .unwrap();
    local
        .send(notification("local", "remote-b", 110))
        .await
        .unwrap();
    remote
        .send(notification("remote-b", "local", 111))
        .await
        .unwrap();
    barrier(&local, &mut acknowledgements).await;
    barrier(&remote, &mut acknowledgements).await;
    quiet(&mut received, Duration::from_millis(100)).await;
    let retained = notification("remote-a", "local", 112);
    remote.send(retained.clone()).await.unwrap();
    assert_eq!(
        next(&mut received).await,
        retained,
        "removing B preserves A's registration"
    );

    streamer
        .delete_route(local_endpoint.clone(), remote_a.clone())
        .await
        .unwrap();
    streamer
        .delete_route(remote_a, local_endpoint)
        .await
        .unwrap();
    local
        .send(notification("local", "remote-a", 113))
        .await
        .unwrap();
    remote
        .send(notification("remote-a", "local", 114))
        .await
        .unwrap();
    barrier(&local, &mut acknowledgements).await;
    barrier(&remote, &mut acknowledgements).await;
    quiet(&mut received, Duration::from_millis(100)).await;
    local
        .unregister_listener(&UUri::any(), Some(&uri("local", 0)), listener.clone())
        .await
        .unwrap();
    for authority in ["remote-a", "remote-b"] {
        remote
            .unregister_listener(&UUri::any(), Some(&uri(authority, 0)), listener.clone())
            .await
            .unwrap();
    }
    for bus in [&local, &remote] {
        bus.unregister_listener(&UUri::any(), Some(&uri("control", 0)), control.clone())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn fixture_publish_matching_and_unregister_preserve_full_filter_identity() {
    let (tx, rx) = broadcast(32);
    let bus = UPClientFoo::new("publish-controls", rx, tx).await;
    let (tx, mut received) = mpsc::unbounded_channel();
    let listener: Arc<dyn UListener> = Arc::new(Collect(tx));
    let (tx, mut acknowledgements) = mpsc::unbounded_channel();
    let control: Arc<dyn UListener> = Arc::new(Collect(tx));
    bus.register_listener(&UUri::any(), Some(&uri("control", 0)), control.clone())
        .await
        .unwrap();
    let source_a = UUri::try_from_parts("source-a", 0x1234, 1, 0xFFFF).unwrap();
    let source_b = uri("source-b", 0x8001);
    bus.register_listener(&source_a, None, listener.clone())
        .await
        .unwrap();
    bus.register_listener(&source_b, None, listener.clone())
        .await
        .unwrap();
    assert_eq!(
        bus.register_listener(&source_a, None, listener.clone())
            .await
            .unwrap_err()
            .code(),
        UCode::AlreadyExists
    );
    let publish = |source| {
        UMessageBuilder::publish(source)
            .build_with_payload(vec![0, 0xFF, 42], PayloadEncoding::RAW)
            .unwrap()
    };
    let first = publish(uri("source-a", 0x8002));
    bus.send(first.clone()).await.unwrap();
    assert_eq!(next(&mut received).await, first);
    bus.send(notification("source-a", "unsubscribed", 5))
        .await
        .unwrap();
    barrier(&bus, &mut acknowledgements).await;
    quiet(&mut received, Duration::from_millis(50)).await;
    bus.unregister_listener(&source_a, None, listener.clone())
        .await
        .unwrap();
    bus.send(publish(uri("source-a", 0x8002))).await.unwrap();
    let retained = publish(source_b.clone());
    bus.send(retained.clone()).await.unwrap();
    assert_eq!(next(&mut received).await, retained);
    barrier(&bus, &mut acknowledgements).await;
    quiet(&mut received, Duration::from_millis(50)).await;
    bus.unregister_listener(&source_b, None, listener)
        .await
        .unwrap();
    bus.unregister_listener(&UUri::any(), Some(&uri("control", 0)), control)
        .await
        .unwrap();
}
