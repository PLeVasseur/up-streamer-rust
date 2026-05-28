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
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use tokio::{runtime::Runtime, sync::Notify};
use up_rust::usubscription::{
    to_proto_uri, FetchSubscribersRequest, FetchSubscribersResponse, FetchSubscriptionsRequest,
    FetchSubscriptionsResponse, NotificationsRequest, ResetRequest, ResetResponse, SubscriberInfo,
    Subscription, SubscriptionRequest, SubscriptionResponse, USubscription, UnsubscribeRequest,
};
use up_rust::{
    zero_copy::{UVecTxBuffer, UZeroCopyListener, UZeroCopyTransport},
    UFrameBuilder, UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus, UTxLoanSpec, UUri,
};
use up_streamer::{OwnedFrameEndpoint, UStreamer};

const PAYLOAD_SIZES: [usize; 4] = [0, 64, 4 * 1024, 64 * 1024];
const FANOUT_ROUTE_COUNTS: [usize; 2] = [2, 8];
const FANOUT_PAYLOAD_SIZE: usize = 4 * 1024;

#[derive(Default)]
struct BenchSubscriptions {
    subscriptions: Vec<Subscription>,
}

#[async_trait]
impl USubscription for BenchSubscriptions {
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
    async fn send_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
        black_box(frame.payload_bytes().len());
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.sent_notify.notify_one();
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

#[derive(Default)]
struct MemoryZeroCopyTransport {
    listeners: Mutex<Vec<RegisteredZeroCopyListener>>,
    sent: AtomicUsize,
    sent_notify: Notify,
}

#[derive(Clone)]
struct RegisteredZeroCopyListener {
    source_filter: UUri,
    sink_filter: Option<UUri>,
    listener: Arc<dyn UZeroCopyListener<UOwnedFrame>>,
}

impl RegisteredZeroCopyListener {
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

impl MemoryZeroCopyTransport {
    async fn inject(&self, frame: UOwnedFrame) {
        let listeners = self
            .listeners
            .lock()
            .expect("listeners lock poisoned")
            .clone();
        for registration in listeners {
            if registration.matches_frame(&frame) {
                registration
                    .listener
                    .on_receive_zero_copy(frame.clone())
                    .await;
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
impl UZeroCopyTransport for MemoryZeroCopyTransport {
    type Tx = UVecTxBuffer;
    type Rx = UOwnedFrame;

    async fn loan_tx(&self, spec: UTxLoanSpec) -> Result<Self::Tx, UStatus> {
        UVecTxBuffer::with_alignment(
            spec.metadata().clone(),
            spec.payload_len(),
            spec.payload_alignment(),
        )
        .map_err(UStatus::from)
    }

    async fn send_zero_copy(&self, buffer: Self::Tx) -> Result<(), UStatus> {
        black_box(buffer.as_ref().len());
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.sent_notify.notify_one();
        Ok(())
    }

    async fn register_zero_copy_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
    ) -> Result<(), UStatus> {
        self.listeners
            .lock()
            .expect("listeners lock poisoned")
            .push(RegisteredZeroCopyListener {
                source_filter: source_filter.clone(),
                sink_filter: sink_filter.cloned(),
                listener,
            });
        Ok(())
    }

    async fn unregister_zero_copy_listener(
        &self,
        _source_filter: &UUri,
        _sink_filter: Option<&UUri>,
        listener: Arc<dyn UZeroCopyListener<Self::Rx>>,
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
    Arc::new(BenchSubscriptions::default())
}

fn subscription_source_with(subscriptions: Vec<Subscription>) -> Arc<dyn USubscription> {
    Arc::new(BenchSubscriptions { subscriptions })
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

fn topic(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0x4210, 1, 0x9001).expect("valid topic")
}

fn wildcard_filter(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0xFFFF_FFFF, 0xFF, 0xFFFF).expect("valid wildcard filter")
}

fn payload(payload_len: usize) -> Vec<u8> {
    (0..payload_len).map(|index| (index % 251) as u8).collect()
}

fn routed_frame(source_authority: &str, sink_authority: &str, payload: &[u8]) -> UOwnedFrame {
    let sink = UUri::try_from_parts(sink_authority, 0x4220, 1, 0).expect("valid sink URI");
    UFrameBuilder::notification(topic(source_authority), sink)
        .build_with_raw_payload(payload.to_vec())
        .expect("valid routed frame")
}

fn publish_frame(source_authority: &str, payload: &[u8]) -> UOwnedFrame {
    UFrameBuilder::publish(topic(source_authority))
        .build_with_raw_payload(payload.to_vec())
        .expect("valid publish frame")
}

fn subscriber(authority: &str) -> UUri {
    UUri::try_from_parts(authority, 0x4220, 1, 0).expect("valid subscriber URI")
}

fn owned_endpoint(
    name: &str,
    authority: &str,
    transport: Arc<MemoryOwnedTransport>,
) -> OwnedFrameEndpoint {
    OwnedFrameEndpoint::from_owned(name, authority, transport)
}

fn zero_copy_endpoint(
    name: &str,
    authority: &str,
    transport: Arc<MemoryZeroCopyTransport>,
) -> OwnedFrameEndpoint {
    OwnedFrameEndpoint::from_zero_copy_copying_adapter(name, authority, transport)
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
                        &owned_endpoint("ingress", "authority-a", ingress),
                        &owned_endpoint("egress", "authority-b", egress),
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
                let ingress_endpoint = owned_endpoint("ingress", "authority-a", ingress);
                let egress_endpoint = owned_endpoint("egress", "authority-b", egress);
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
    let mut group = c.benchmark_group("egress_forwarding");

    for payload_len in PAYLOAD_SIZES {
        let payload = payload(payload_len);

        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let _streamer = runtime.block_on(async {
            let mut streamer = UStreamer::new("bench", 1024, subscription_source())
                .await
                .expect("streamer");
            streamer
                .add_route_ref(
                    &owned_endpoint("ingress", "authority-a", ingress.clone()),
                    &owned_endpoint("egress", "authority-b", egress.clone()),
                )
                .await
                .expect("route registers");
            streamer
        });
        let mut expected_count = 0;
        group.bench_function(BenchmarkId::new("owned_to_owned", payload_len), |b| {
            b.iter(|| {
                expected_count += 1;
                runtime.block_on(async {
                    ingress
                        .inject(routed_frame("authority-a", "authority-b", &payload))
                        .await;
                    egress.wait_for_sent(expected_count).await;
                })
            })
        });

        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let _streamer = runtime.block_on(async {
            let mut streamer = UStreamer::new("bench", 1024, subscription_source())
                .await
                .expect("streamer");
            streamer
                .add_route_ref(
                    &owned_endpoint("ingress", "authority-a", ingress.clone()),
                    &zero_copy_endpoint("egress", "authority-b", egress.clone()),
                )
                .await
                .expect("route registers");
            streamer
        });
        let mut expected_count = 0;
        group.bench_function(BenchmarkId::new("owned_to_zero_copy", payload_len), |b| {
            b.iter(|| {
                expected_count += 1;
                runtime.block_on(async {
                    ingress
                        .inject(routed_frame("authority-a", "authority-b", &payload))
                        .await;
                    egress.wait_for_sent(expected_count).await;
                })
            })
        });

        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryOwnedTransport::default());
        let _streamer = runtime.block_on(async {
            let mut streamer = UStreamer::new("bench", 1024, subscription_source())
                .await
                .expect("streamer");
            streamer
                .add_route_ref(
                    &zero_copy_endpoint("ingress", "authority-a", ingress.clone()),
                    &owned_endpoint("egress", "authority-b", egress.clone()),
                )
                .await
                .expect("route registers");
            streamer
        });
        let mut expected_count = 0;
        group.bench_function(BenchmarkId::new("zero_copy_to_owned", payload_len), |b| {
            b.iter(|| {
                expected_count += 1;
                runtime.block_on(async {
                    ingress
                        .inject(routed_frame("authority-a", "authority-b", &payload))
                        .await;
                    egress.wait_for_sent(expected_count).await;
                })
            })
        });

        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egress = Arc::new(MemoryZeroCopyTransport::default());
        let _streamer = runtime.block_on(async {
            let mut streamer = UStreamer::new("bench", 1024, subscription_source())
                .await
                .expect("streamer");
            streamer
                .add_route_ref(
                    &zero_copy_endpoint("ingress", "authority-a", ingress.clone()),
                    &zero_copy_endpoint("egress", "authority-b", egress.clone()),
                )
                .await
                .expect("route registers");
            streamer
        });
        let mut expected_count = 0;
        group.bench_function(
            BenchmarkId::new("zero_copy_to_zero_copy", payload_len),
            |b| {
                b.iter(|| {
                    expected_count += 1;
                    runtime.block_on(async {
                        ingress
                            .inject(routed_frame("authority-a", "authority-b", &payload))
                            .await;
                        egress.wait_for_sent(expected_count).await;
                    })
                })
            },
        );
    }

    group.finish();
}

fn bench_fanout_forwarding(c: &mut Criterion) {
    let runtime = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("fanout_forwarding");

    for route_count in FANOUT_ROUTE_COUNTS {
        let payload = payload(FANOUT_PAYLOAD_SIZE);
        let topic = topic("authority-a");
        let egress_authorities = (0..route_count)
            .map(|index| format!("authority-b-{index}"))
            .collect::<Vec<_>>();
        let subscriptions = egress_authorities
            .iter()
            .map(|authority| subscription(topic.clone(), subscriber(authority)))
            .collect::<Vec<_>>();

        let ingress = Arc::new(MemoryOwnedTransport::default());
        let egresses = (0..route_count)
            .map(|_| Arc::new(MemoryOwnedTransport::default()))
            .collect::<Vec<_>>();
        let _streamer = runtime.block_on(async {
            let mut streamer = UStreamer::new(
                "bench",
                1024,
                subscription_source_with(subscriptions.clone()),
            )
            .await
            .expect("streamer");
            for (index, egress) in egresses.iter().enumerate() {
                streamer
                    .add_route_ref(
                        &owned_endpoint("ingress", "authority-a", ingress.clone()),
                        &owned_endpoint(
                            &format!("egress-{index}"),
                            &egress_authorities[index],
                            egress.clone(),
                        ),
                    )
                    .await
                    .expect("route registers");
            }
            streamer
        });
        let mut expected_count = 0;
        group.bench_function(
            BenchmarkId::new("owned_to_owned_routes", route_count),
            |b| {
                b.iter(|| {
                    expected_count += 1;
                    runtime.block_on(async {
                        ingress.inject(publish_frame("authority-a", &payload)).await;
                        for egress in &egresses {
                            egress.wait_for_sent(expected_count).await;
                        }
                    })
                })
            },
        );

        let ingress = Arc::new(MemoryZeroCopyTransport::default());
        let egresses = (0..route_count)
            .map(|_| Arc::new(MemoryZeroCopyTransport::default()))
            .collect::<Vec<_>>();
        let _streamer = runtime.block_on(async {
            let mut streamer = UStreamer::new(
                "bench",
                1024,
                subscription_source_with(subscriptions.clone()),
            )
            .await
            .expect("streamer");
            for (index, egress) in egresses.iter().enumerate() {
                streamer
                    .add_route_ref(
                        &zero_copy_endpoint("ingress", "authority-a", ingress.clone()),
                        &zero_copy_endpoint(
                            &format!("egress-{index}"),
                            &egress_authorities[index],
                            egress.clone(),
                        ),
                    )
                    .await
                    .expect("route registers");
            }
            streamer
        });
        let mut expected_count = 0;
        group.bench_function(
            BenchmarkId::new("zero_copy_to_zero_copy_routes", route_count),
            |b| {
                b.iter(|| {
                    expected_count += 1;
                    runtime.block_on(async {
                        ingress.inject(publish_frame("authority-a", &payload)).await;
                        for egress in &egresses {
                            egress.wait_for_sent(expected_count).await;
                        }
                    })
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    streamer_criterion,
    bench_routing_lookup,
    bench_publish_resolution,
    bench_ingress_registry,
    bench_egress_forwarding,
    bench_fanout_forwarding
);
criterion_main!(streamer_criterion);
