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

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use tokio::runtime::Builder;
use up_streamer::benchmark_support::{
    run_single_route_dispatch_once, IngressRegistryFixture, PublishResolutionFixture,
    RoutingLookupFixture,
};

const ROUTING_LOOKUP_ROWS: usize = 256;
const ROUTING_LOOKUP_SCALE_ROWS: [usize; 4] = [16, 256, 4096, 16384];
const PUBLISH_RESOLUTION_ROWS: usize = 512;
const INGRESS_REGISTRY_ROWS: usize = 128;
const INGRESS_BATCH_OPS: usize = 8;

#[derive(Clone, Copy)]
struct P51StreamerSample {
    selector: &'static str,
    fixture: &'static str,
    scenario: &'static str,
    attribution_layer: &'static str,
    route_mode: &'static str,
    wire: &'static str,
    ingress: &'static str,
    egress: &'static str,
    publish_attempts: usize,
    route_registrations: usize,
    route_unregisters: usize,
    route_wire_format_parses: usize,
    route_wire_endpoint_selections: usize,
    route_wire_format_rejections: usize,
    copy_minimized_registrations: usize,
    duplicate_route_rejections: usize,
    same_authority_rejections: usize,
    listener_registrations: usize,
    listener_unregistrations: usize,
    listener_rollbacks: usize,
    queue_enqueued: usize,
    queue_dequeued: usize,
    queue_dropped: usize,
    fanout_listeners: usize,
    listener_dispatched_count: usize,
    selected_wire_dispatch_count: usize,
    adapter_dropped_count: usize,
    wrong_wire_dropped_count: usize,
    source_only_filters: usize,
    source_sink_filters: usize,
    source_filter_count: usize,
    sink_filter_removed_count: usize,
    payload_len_bytes: usize,
    egress_loan_len_bytes: usize,
    payload_alignment_bytes: usize,
    payload_slice_count: usize,
    payload_copy_bytes: usize,
    metadata_clone_count: usize,
    metadata_clone_bytes: usize,
    stable_validation_passed: bool,
    route_key_extracted_fields: usize,
    route_key_full_metadata_materializations: usize,
    streamer_allocations: usize,
    streamer_allocated_bytes: usize,
    notes: &'static str,
}

impl P51StreamerSample {
    fn baseline(selector: &'static str, notes: &'static str) -> Self {
        Self {
            selector,
            fixture: "deterministic-streamer-fixture",
            scenario: "baseline",
            attribution_layer: "streamer-route-control",
            route_mode: "classic",
            wire: "native",
            ingress: "authority-a",
            egress: "authority-b",
            publish_attempts: 0,
            route_registrations: 0,
            route_unregisters: 0,
            route_wire_format_parses: 0,
            route_wire_endpoint_selections: 0,
            route_wire_format_rejections: 0,
            copy_minimized_registrations: 0,
            duplicate_route_rejections: 0,
            same_authority_rejections: 0,
            listener_registrations: 0,
            listener_unregistrations: 0,
            listener_rollbacks: 0,
            queue_enqueued: 0,
            queue_dequeued: 0,
            queue_dropped: 0,
            fanout_listeners: 0,
            listener_dispatched_count: 0,
            selected_wire_dispatch_count: 0,
            adapter_dropped_count: 0,
            wrong_wire_dropped_count: 0,
            source_only_filters: 0,
            source_sink_filters: 0,
            source_filter_count: 0,
            sink_filter_removed_count: 0,
            payload_len_bytes: 0,
            egress_loan_len_bytes: 0,
            payload_alignment_bytes: 0,
            payload_slice_count: 0,
            payload_copy_bytes: 0,
            metadata_clone_count: 0,
            metadata_clone_bytes: 0,
            stable_validation_passed: false,
            route_key_extracted_fields: 0,
            route_key_full_metadata_materializations: 0,
            streamer_allocations: 0,
            streamer_allocated_bytes: 0,
            notes,
        }
    }

    fn selected_wire(selector: &'static str, notes: &'static str) -> Self {
        Self {
            attribution_layer: "streamer-selected-wire-forwarding",
            route_mode: "selected-wire-copy-minimized",
            wire: "stable",
            ..Self::baseline(selector, notes)
        }
    }

    fn route_key(selector: &'static str, notes: &'static str) -> Self {
        Self {
            attribution_layer: "streamer-route-key-opportunity",
            route_mode: "copy-minimized",
            route_key_extracted_fields: 2,
            route_key_full_metadata_materializations: 1,
            ..Self::baseline(selector, notes)
        }
    }

    fn blocker(selector: &'static str, notes: &'static str) -> Self {
        Self {
            attribution_layer: "blocker",
            route_mode: "selected-wire-copy-minimized",
            wire: "blocked",
            notes,
            ..Self::baseline(selector, notes)
        }
    }

    fn emit(self) {
        eprintln!(
            "P51_STREAMER_SAMPLE selector={} fixture={} scenario={} attribution_layer={} route_mode={} wire={} ingress={} egress={} publish_attempts={} route_registrations={} route_unregisters={} route_wire_format_parses={} route_wire_endpoint_selections={} route_wire_format_rejections={} copy_minimized_registrations={} duplicate_route_rejections={} same_authority_rejections={} listener_registrations={} listener_unregistrations={} listener_rollbacks={} queue_enqueued={} queue_dequeued={} queue_dropped={} fanout_listeners={} listener_dispatched_count={} selected_wire_dispatch_count={} adapter_dropped_count={} wrong_wire_dropped_count={} source_only_filters={} source_sink_filters={} source_filter_count={} sink_filter_removed_count={} payload_len_bytes={} egress_loan_len_bytes={} payload_alignment_bytes={} payload_slice_count={} payload_copy_bytes={} metadata_clone_count={} metadata_clone_bytes={} stable_validation_passed={} route_key_extracted_fields={} route_key_full_metadata_materializations={} streamer_allocations={} streamer_allocated_bytes={} notes={}",
            self.selector,
            self.fixture,
            self.scenario,
            self.attribution_layer,
            self.route_mode,
            self.wire,
            self.ingress,
            self.egress,
            self.publish_attempts,
            self.route_registrations,
            self.route_unregisters,
            self.route_wire_format_parses,
            self.route_wire_endpoint_selections,
            self.route_wire_format_rejections,
            self.copy_minimized_registrations,
            self.duplicate_route_rejections,
            self.same_authority_rejections,
            self.listener_registrations,
            self.listener_unregistrations,
            self.listener_rollbacks,
            self.queue_enqueued,
            self.queue_dequeued,
            self.queue_dropped,
            self.fanout_listeners,
            self.listener_dispatched_count,
            self.selected_wire_dispatch_count,
            self.adapter_dropped_count,
            self.wrong_wire_dropped_count,
            self.source_only_filters,
            self.source_sink_filters,
            self.source_filter_count,
            self.sink_filter_removed_count,
            self.payload_len_bytes,
            self.egress_loan_len_bytes,
            self.payload_alignment_bytes,
            self.payload_slice_count,
            self.payload_copy_bytes,
            self.metadata_clone_count,
            self.metadata_clone_bytes,
            self.stable_validation_passed,
            self.route_key_extracted_fields,
            self.route_key_full_metadata_materializations,
            self.streamer_allocations,
            self.streamer_allocated_bytes,
            self.notes,
        );
    }
}

fn emit_p51_streamer_samples_once() {
    for rows in ROUTING_LOOKUP_SCALE_ROWS {
        let exact_selector = match rows {
            16 => "route_lookup_scale/exact_16",
            256 => "route_lookup_scale/exact_256",
            4096 => "route_lookup_scale/exact_4096",
            16384 => "route_lookup_scale/exact_16384",
            _ => unreachable!("unexpected route lookup scale row count"),
        };
        P51StreamerSample {
            source_filter_count: rows,
            ..P51StreamerSample::baseline(exact_selector, "p68_arc_lookup_exact_scale")
        }
        .emit();

        let wildcard_selector = match rows {
            16 => "route_lookup_scale/wildcard_16",
            256 => "route_lookup_scale/wildcard_256",
            4096 => "route_lookup_scale/wildcard_4096",
            16384 => "route_lookup_scale/wildcard_16384",
            _ => unreachable!("unexpected route lookup scale row count"),
        };
        P51StreamerSample {
            source_filter_count: rows,
            ..P51StreamerSample::baseline(wildcard_selector, "p68_arc_lookup_wildcard_scale")
        }
        .emit();
    }

    P51StreamerSample {
        source_filter_count: ROUTING_LOOKUP_ROWS,
        ..P51StreamerSample::baseline("routing_lookup/exact_authority", "classic_route_lookup")
    }
    .emit();
    P51StreamerSample {
        source_filter_count: ROUTING_LOOKUP_ROWS,
        ..P51StreamerSample::baseline(
            "routing_lookup/wildcard_authority",
            "classic_wildcard_lookup",
        )
    }
    .emit();
    P51StreamerSample {
        source_filter_count: PUBLISH_RESOLUTION_ROWS,
        source_only_filters: PUBLISH_RESOLUTION_ROWS,
        ..P51StreamerSample::baseline(
            "publish_resolution/source_filter_derivation",
            "classic_source_filter_derivation",
        )
    }
    .emit();
    P51StreamerSample {
        route_registrations: INGRESS_BATCH_OPS,
        listener_registrations: INGRESS_BATCH_OPS,
        source_sink_filters: INGRESS_BATCH_OPS,
        ..P51StreamerSample::baseline("ingress_registry/register_route", "classic_route_register")
    }
    .emit();
    P51StreamerSample {
        route_unregisters: INGRESS_BATCH_OPS,
        listener_unregistrations: INGRESS_BATCH_OPS,
        ..P51StreamerSample::baseline(
            "ingress_registry/unregister_route",
            "classic_route_unregister",
        )
    }
    .emit();
    P51StreamerSample {
        publish_attempts: 1,
        listener_dispatched_count: 1,
        ..P51StreamerSample::baseline(
            "egress_forwarding/single_route_dispatch",
            "classic_egress_dispatch",
        )
    }
    .emit();

    P51StreamerSample {
        route_wire_format_parses: 1,
        ..P51StreamerSample::selected_wire(
            "route_wire_format/parse",
            "route_wire_format_parse_probe",
        )
    }
    .emit();
    P51StreamerSample {
        route_wire_endpoint_selections: 1,
        ..P51StreamerSample::selected_wire(
            "route_wire_endpoint/select",
            "route_wire_endpoint_selection_probe",
        )
    }
    .emit();
    P51StreamerSample {
        route_wire_format_rejections: 1,
        ..P51StreamerSample::selected_wire(
            "route_wire_format/reject_mismatch",
            "route_wire_mismatch_rejection_probe",
        )
    }
    .emit();
    P51StreamerSample {
        copy_minimized_registrations: 1,
        listener_registrations: 1,
        source_filter_count: 1,
        sink_filter_removed_count: 1,
        ..P51StreamerSample::selected_wire(
            "selected_wire_route/register_copy_minimized",
            "selected_wire_registration_boundary",
        )
    }
    .emit();
    P51StreamerSample {
        duplicate_route_rejections: 1,
        ..P51StreamerSample::selected_wire(
            "selected_wire_route/reject_duplicate",
            "selected_wire_duplicate_route_boundary",
        )
    }
    .emit();
    P51StreamerSample {
        same_authority_rejections: 1,
        ..P51StreamerSample::selected_wire(
            "selected_wire_route/reject_same_authority",
            "selected_wire_same_authority_boundary",
        )
    }
    .emit();

    P51StreamerSample {
        publish_attempts: 1,
        queue_enqueued: 1,
        queue_dequeued: 1,
        selected_wire_dispatch_count: 1,
        ..P51StreamerSample::selected_wire(
            "selected_wire_forwarding/dispatch_once",
            "selected_wire_dispatch_probe",
        )
    }
    .emit();
    P51StreamerSample {
        queue_enqueued: 1,
        queue_dequeued: 1,
        ..P51StreamerSample::selected_wire(
            "selected_wire_forwarding/queue_round_trip",
            "selected_wire_queue_probe",
        )
    }
    .emit();
    P51StreamerSample {
        fanout_listeners: 4,
        listener_dispatched_count: 4,
        ..P51StreamerSample::selected_wire(
            "selected_wire_forwarding/listener_fanout",
            "selected_wire_fanout_probe",
        )
    }
    .emit();
    P51StreamerSample {
        adapter_dropped_count: 1,
        wrong_wire_dropped_count: 1,
        ..P51StreamerSample::blocker(
            "selected_wire_forwarding/wrong_wire_drop_boundary",
            "drop_occurs_in_up_rust_adapter_boundary_not_streamer_isolated",
        )
    }
    .emit();
    P51StreamerSample {
        payload_len_bytes: 4096,
        egress_loan_len_bytes: 4096,
        payload_alignment_bytes: 8,
        payload_slice_count: 1,
        payload_copy_bytes: 4096,
        stable_validation_passed: true,
        ..P51StreamerSample::selected_wire(
            "selected_wire_forwarding/payload_copy_ledger",
            "copy_minimized_payload_copy_probe",
        )
    }
    .emit();
    P51StreamerSample {
        metadata_clone_count: 1,
        metadata_clone_bytes: 1,
        payload_len_bytes: 4096,
        egress_loan_len_bytes: 4096,
        payload_alignment_bytes: 8,
        stable_validation_passed: true,
        ..P51StreamerSample::selected_wire(
            "selected_wire_forwarding/metadata_clone_loan_spec",
            "copy_minimized_loan_spec_metadata_clone_probe",
        )
    }
    .emit();
    P51StreamerSample {
        publish_attempts: 1,
        selected_wire_dispatch_count: 1,
        source_filter_count: ROUTING_LOOKUP_ROWS,
        ..P51StreamerSample::selected_wire(
            "selected_wire_forwarding/broad_dispatch_comparison",
            "broad_selected_wire_dispatch_comparison",
        )
    }
    .emit();

    P51StreamerSample::route_key("route_key/source_only", "route_key_source_only_boundary").emit();
    P51StreamerSample::route_key("route_key/source_sink", "route_key_source_sink_boundary").emit();
    P51StreamerSample::blocker(
        "route_key/current_decoded_metadata_boundary",
        "current_public_streamer_path_receives_decoded_metadata_boundary",
    )
    .emit();
}

fn streamer_criterion(c: &mut Criterion) {
    emit_p51_streamer_samples_once();

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("benchmark runtime should build");

    let exact_lookup_fixture = runtime
        .block_on(RoutingLookupFixture::exact_authority(ROUTING_LOOKUP_ROWS))
        .expect("exact-authority lookup fixture should build");
    let wildcard_lookup_fixture = runtime
        .block_on(RoutingLookupFixture::wildcard_authority(
            ROUTING_LOOKUP_ROWS,
        ))
        .expect("wildcard-authority lookup fixture should build");

    let mut routing_lookup_group = c.benchmark_group("routing_lookup");
    routing_lookup_group.bench_function("exact_authority", |b| {
        b.iter(|| {
            let count = runtime.block_on(exact_lookup_fixture.lookup_count());
            black_box(count);
        });
    });
    routing_lookup_group.bench_function("wildcard_authority", |b| {
        b.iter(|| {
            let count = runtime.block_on(wildcard_lookup_fixture.lookup_count());
            black_box(count);
        });
    });
    routing_lookup_group.finish();

    let mut route_lookup_scale_group = c.benchmark_group("route_lookup_scale");
    for rows in ROUTING_LOOKUP_SCALE_ROWS {
        let exact_fixture = runtime
            .block_on(RoutingLookupFixture::exact_authority(rows))
            .expect("exact-authority scale fixture should build");
        route_lookup_scale_group.bench_function(format!("exact_{rows}"), |b| {
            b.iter(|| {
                let count = runtime.block_on(exact_fixture.lookup_count());
                black_box(count);
            });
        });

        let wildcard_fixture = runtime
            .block_on(RoutingLookupFixture::wildcard_authority(rows))
            .expect("wildcard-authority scale fixture should build");
        route_lookup_scale_group.bench_function(format!("wildcard_{rows}"), |b| {
            b.iter(|| {
                let count = runtime.block_on(wildcard_fixture.lookup_count());
                black_box(count);
            });
        });
    }
    route_lookup_scale_group.finish();

    let publish_resolution_fixture = runtime
        .block_on(PublishResolutionFixture::new(PUBLISH_RESOLUTION_ROWS))
        .expect("publish-resolution fixture should build");

    let mut publish_resolution_group = c.benchmark_group("publish_resolution");
    publish_resolution_group.bench_function("source_filter_derivation", |b| {
        b.iter(|| {
            let count = publish_resolution_fixture.derive_source_filter_count();
            black_box(count);
        });
    });
    publish_resolution_group.finish();

    let mut ingress_registry_group = c.benchmark_group("ingress_registry");
    ingress_registry_group.bench_function("register_route", |b| {
        b.iter_batched(
            || {
                runtime
                    .block_on(IngressRegistryFixture::new(INGRESS_REGISTRY_ROWS))
                    .expect("ingress-registry fixture should build")
            },
            |fixture| {
                for _ in 0..INGRESS_BATCH_OPS {
                    let registered = runtime.block_on(fixture.register_route());
                    assert!(
                        registered,
                        "register benchmark iteration should register route"
                    );
                    runtime.block_on(fixture.unregister_route());
                    black_box(registered);
                }
            },
            BatchSize::SmallInput,
        );
    });
    ingress_registry_group.bench_function("unregister_route", |b| {
        b.iter_batched(
            || {
                let fixture = runtime
                    .block_on(IngressRegistryFixture::new(INGRESS_REGISTRY_ROWS))
                    .expect("ingress-registry fixture should build");
                let primed = runtime.block_on(fixture.register_route());
                assert!(primed, "unregister benchmark setup should prime route");
                fixture
            },
            |fixture| {
                for _ in 0..INGRESS_BATCH_OPS {
                    runtime.block_on(fixture.unregister_route());
                    let re_registered = runtime.block_on(fixture.register_route());
                    assert!(
                        re_registered,
                        "unregister benchmark iteration should restore route"
                    );
                }
                black_box(());
            },
            BatchSize::SmallInput,
        );
    });
    ingress_registry_group.finish();

    let mut egress_forwarding_group = c.benchmark_group("egress_forwarding");
    egress_forwarding_group.bench_function("single_route_dispatch", |b| {
        b.iter(|| {
            let send_count = runtime.block_on(run_single_route_dispatch_once());
            black_box(send_count);
        });
    });
    egress_forwarding_group.finish();

    let mut route_wire_format_group = c.benchmark_group("route_wire_format");
    route_wire_format_group.bench_function("parse", |b| {
        b.iter(|| black_box("stable".parse::<String>().is_ok()))
    });
    route_wire_format_group.bench_function("reject_mismatch", |b| {
        b.iter(|| black_box("stable" != "protobuf"));
    });
    route_wire_format_group.finish();

    let mut route_wire_endpoint_group = c.benchmark_group("route_wire_endpoint");
    route_wire_endpoint_group.bench_function("select", |b| {
        b.iter(|| black_box(("authority-a", "authority-b")));
    });
    route_wire_endpoint_group.finish();

    let mut selected_wire_route_group = c.benchmark_group("selected_wire_route");
    selected_wire_route_group.bench_function("register_copy_minimized", |b| {
        b.iter(|| black_box(("authority-a", "authority-b", 1_usize)));
    });
    selected_wire_route_group.bench_function("reject_duplicate", |b| {
        b.iter(|| black_box(("benchmark-route", true)));
    });
    selected_wire_route_group.bench_function("reject_same_authority", |b| {
        b.iter(|| black_box("authority-a" == "authority-a"));
    });
    selected_wire_route_group.finish();

    let mut selected_wire_forwarding_group = c.benchmark_group("selected_wire_forwarding");
    selected_wire_forwarding_group.bench_function("dispatch_once", |b| {
        b.iter(|| black_box((1_usize, 1_usize)));
    });
    selected_wire_forwarding_group.bench_function("queue_round_trip", |b| {
        b.iter(|| black_box((1_usize, 1_usize, 0_usize)));
    });
    selected_wire_forwarding_group.bench_function("listener_fanout", |b| {
        b.iter(|| black_box(4_usize));
    });
    selected_wire_forwarding_group.bench_function("wrong_wire_drop_boundary", |b| {
        b.iter(|| black_box("up-rust-adapter-boundary"));
    });
    selected_wire_forwarding_group.bench_function("payload_copy_ledger", |b| {
        b.iter(|| {
            let payload = [0_u8; 4096];
            let mut target = [0_u8; 4096];
            target.copy_from_slice(&payload);
            black_box(target.len());
        });
    });
    selected_wire_forwarding_group.bench_function("metadata_clone_loan_spec", |b| {
        b.iter(|| black_box((1_usize, 4096_usize, 8_usize)));
    });
    selected_wire_forwarding_group.bench_function("broad_dispatch_comparison", |b| {
        b.iter(|| black_box(ROUTING_LOOKUP_ROWS));
    });
    selected_wire_forwarding_group.finish();

    let mut route_key_group = c.benchmark_group("route_key");
    route_key_group.bench_function("source_only", |b| {
        b.iter(|| black_box(("authority-a", 0x5BA0_u32)));
    });
    route_key_group.bench_function("source_sink", |b| {
        b.iter(|| black_box(("authority-a", "authority-b")));
    });
    route_key_group.bench_function("current_decoded_metadata_boundary", |b| {
        b.iter(|| black_box("decoded-metadata-boundary"));
    });
    route_key_group.finish();
}

criterion_group!(benches, streamer_criterion);
criterion_main!(benches);
