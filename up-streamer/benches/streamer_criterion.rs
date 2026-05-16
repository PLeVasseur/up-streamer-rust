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

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use async_trait::async_trait;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tokio::{runtime::Runtime, sync::Notify};
use up_rust::usubscription::{
    FetchSubscribersRequest, FetchSubscribersResponse, FetchSubscriptionsRequest,
    FetchSubscriptionsResponse, NotificationsRequest, ResetRequest, ResetResponse,
    SubscriptionRequest, SubscriptionResponse, USubscription, UnsubscribeRequest,
};
use up_rust::{UFrameMetadata, UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus, UUri};
use up_streamer::{OwnedFrameEndpoint, UStreamer};

#[derive(Default)]
struct EmptySubscriptions;

#[async_trait]
impl USubscription for EmptySubscriptions {
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
        Ok(FetchSubscriptionsResponse::default())
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

#[derive(Default)]
struct MemoryOwnedTransport {
    listeners: Mutex<Vec<RegisteredOwnedListener>>,
    sent: AtomicUsize,
    sent_notify: Notify,
}

#[derive(Clone)]
struct RegisteredOwnedListener {
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UOwnedListener>,
}

impl RegisteredOwnedListener {
    fn matches_frame(&self, frame: &UOwnedFrame) -> bool {
        if !self.source_filter.matches(frame.metadata().source()) {
            return false;
        }
        if let Some(sink_filter) = &self.sink_filter {
            frame
                .metadata()
                .sink()
                .is_some_and(|sink| sink_filter.matches(sink))
        } else {
            frame.metadata().sink().is_none()
        }
    }
}

impl MemoryOwnedTransport {
    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listeners lock poisoned")
            .clone();
        for registration in listeners {
            if registration.matches_frame(&frame) {
                registration.listener.on_receive_owned(frame.clone()).await;
            }
        }
    }

    async fn wait_for_sent(&self, count: usize) {
        while self.sent.load(Ordering::SeqCst) < count {
            self.sent_notify.notified().await;
        }
    }
}

#[async_trait]
impl UOwnedTransport for MemoryOwnedTransport {
    async fn send_owned(&self, _frame: UOwnedFrame) -> Result<(), UStatus> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.sent_notify.notify_waiters();
        Ok(())
    }

    async fn register_owned_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listeners lock poisoned")
            .push(RegisteredOwnedListener {
                source_filter: source_filter.clone(),
                sink_filter: sink_filter.cloned(),
                listener,
            });
        Ok(())
    }

    async fn unregister_owned_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UOwnedListener>,
    ) -> Result<(), UStatus> {
        let mut listeners = self.listeners.lock().expect("listeners lock poisoned");
        if let Some(index) = listeners
            .iter()
            .position(|existing| Arc::ptr_eq(&existing.listener, &listener))
        {
            listeners.remove(index);
        }
        Ok(())
    }
}

fn subscription_source() -> Arc<dyn USubscription> {
    Arc::new(EmptySubscriptions)
}

fn topic(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid topic")
}

fn wildcard_filter(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0xFFFF_FFFF, 0xFF, 0xFFFF).expect("valid wildcard filter")
}

fn frame(authority: &str) -> UOwnedFrame {
    UOwnedFrame::new(
        UFrameMetadata::publish(topic(authority)),
        b"native-stream".as_slice(),
    )
}

fn endpoint(
    name: &str,
    authority: &str,
    transport: Arc<MemoryOwnedTransport>,
) -> OwnedFrameEndpoint {
    OwnedFrameEndpoint::from_owned(name, authority, transport)
}

fn bench_routing_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing_lookup");
    let exact = topic("authority-a");
    let exact_candidate = topic("authority-a");
    let wildcard = wildcard_filter("authority-a");

    group.bench_function("exact_authority", |b| {
        b.iter(|| black_box(&exact).matches(black_box(&exact_candidate)))
    });
    group.bench_function("wildcard_authority", |b| {
        b.iter(|| black_box(&wildcard).matches(black_box(&exact_candidate)))
    });
    group.finish();
}

fn bench_publish_resolution(c: &mut Criterion) {
    c.benchmark_group("publish_resolution")
        .bench_function("source_filter_derivation", |b| {
            b.iter(|| black_box(wildcard_filter(black_box("authority-a"))))
        });
}

fn bench_ingress_registry(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("ingress_registry");

    group.bench_function("register_route", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let ingress = Arc::new(MemoryOwnedTransport::default());
                let egress = Arc::new(MemoryOwnedTransport::default());
                let mut streamer = UStreamer::new("bench", 16, subscription_source())
                    .await
                    .expect("streamer");
                streamer
                    .add_route_ref(
                        &endpoint("ingress", "authority-a", ingress),
                        &endpoint("egress", "authority-b", egress),
                    )
                    .await
                    .expect("route registers");
                black_box(streamer.name());
            })
        })
    });

    group.bench_function("unregister_route", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let ingress = Arc::new(MemoryOwnedTransport::default());
                let egress = Arc::new(MemoryOwnedTransport::default());
                let ingress_endpoint = endpoint("ingress", "authority-a", ingress);
                let egress_endpoint = endpoint("egress", "authority-b", egress);
                let mut streamer = UStreamer::new("bench", 16, subscription_source())
                    .await
                    .expect("streamer");
                streamer
                    .add_route_ref(&ingress_endpoint, &egress_endpoint)
                    .await
                    .expect("route registers");
                streamer
                    .delete_route_ref(&ingress_endpoint, &egress_endpoint)
                    .await
                    .expect("route unregisters");
            })
        })
    });

    group.finish();
}

fn bench_egress_forwarding(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let ingress = Arc::new(MemoryOwnedTransport::default());
    let egress = Arc::new(MemoryOwnedTransport::default());
    let _streamer = runtime.block_on(async {
        let mut streamer = UStreamer::new("bench", 1024, subscription_source())
            .await
            .expect("streamer");
        streamer
            .add_route_ref(
                &endpoint("ingress", "authority-a", ingress.clone()),
                &endpoint("egress", "authority-b", egress.clone()),
            )
            .await
            .expect("route registers");
        streamer
    });

    let mut expected_count = 0;
    c.benchmark_group("egress_forwarding")
        .bench_function("single_route_dispatch", |b| {
            b.iter(|| {
                expected_count += 1;
                runtime.block_on(async {
                    ingress.inject(frame("authority-a")).await;
                    egress.wait_for_sent(expected_count).await;
                })
            })
        });
}

criterion_group!(
    streamer_criterion,
    bench_routing_lookup,
    bench_publish_resolution,
    bench_ingress_registry,
    bench_egress_forwarding
);
criterion_main!(streamer_criterion);
