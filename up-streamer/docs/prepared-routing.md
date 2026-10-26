# Prepared native routing

`ZeroCopyFrameEndpoint` holds a concrete selected-wire adapter over an
`Arc<PreparedTransportCore>`. The `PreparedEndpoint` trait exposes that setup
composition; it is not a dynamically dispatched async transport. Configure
`PreparedRouteReservation` entries using `with_routes` before the endpoint is
used as an egress. Each entry fixes exact source/sink, metadata and payload
capacity, alignment and token/traceparent bounds. An ingress-only endpoint does
not need an egress catalog. Unknown addresses cannot grow it during traffic.

`with_receive_routes` declares the exact addresses in an ingress-only native
catalog. Streamer intersects its semantic filters with that catalog before
attaching subscribers, so pubsub-only configuration does not accidentally request
an unreserved RPC service. `with_routes` also declares the same receive addresses.
The original filters are still rechecked by SDK admission on actual metadata.

The native bindings own SHM/provider geometry, native resource setup and actual
sample lifetimes. Streamer checks selected wire/family/version and declared
native agreement consistency at route installation. Serialized opaque carriage
does not decode payloads or infer their types. The SDK's header admission checks
wire/family/layout, semantic metadata, token bounds and decoded source/sink
filters before forwarding. `sink=None` remains sinkless, not an arbitrary sink.

Each filter creates one generic SDK registration worker at setup. The native
callback pushes directly into its bounded inbox and the worker handles frames
sequentially; there is no second Streamer mailbox or per-message task. The
configured message queue capacity is retained. Block has a five-second local
admission deadline; it may hold a native delivery thread, including reentrant
publication. It does not change native receive pool capacities or imply that a
synchronous native send is end-to-end nonblocking.

TX routes use SDK `OpaqueRoute` and a storage-forwarding observation wrapper.
Payload bytes are copied once into a native egress loan. Metadata and payload
identity are carried without relabeling. Successful publication, failure and
unreserved addressing are distinct observations. The wrapper counts copied
payload bytes immediately before actual native publish, after SDK initialization
proof; a failed native put therefore still counts its completed copy. Failed
loans and pre-copy rejection do not count a copy. This is copy-minimized routing,
not end-to-end zero-copy.

Classic and owned adapter routes remain explicitly allocating/copying boundaries.
Their checked projections use the configured native agreement and retain the
complete-route SingleType classic-hop policy. Generic setup workers replace
per-message dispatch tasks, while legacy classic/owned callback APIs retain their
own allocation properties. Absent and present-empty payloads remain distinct.
Their bounded channel admission is asynchronous, so backpressure yields to the
worker even on a current-thread runtime. Native callback admission remains the
SDK's synchronous bounded policy. Close wakes pending compatibility senders and
the one worker drains queued frames; self-stop does not await its own task.

Deletion closes admission before physical detach and then drains admitted work.
A failed detach retains its exact registration and route map entry for retry.
Drop cancels a worker; stop/drain is an explicit operation. Already retained
native frames are independent of route/endpoint lifetime and are never revoked.
`forwarding_stats` reports native egress byte/copy/success/failure counters without
formatting or allocating on each frame. These counters are separate from optional
classic/owned diagnostic logging, native physical counts and application inbox
drop accounting.

`with_observed_routes` installs a setup-owned `PreparedForwardObserver`. It sees
admitted ingress, actual initialized metadata and payload extents in the egress
loan, and forwarding completion keyed by carried UUID. Copy callbacks
are absent for an unreserved address, but its admitted ingress still receives
one completion callback with the rejection. Unreserved and post-reservation send
failures retain separate counters. Metadata and payload views
are borrowed separately to preserve the native storage's exclusive access rules.
Observers cannot retain those views and must preallocate their recording capacity
for zero-allocation operation. Their costs are measured without subtraction.
This allows raw identity/copy/failure evidence without forcing per-message heap
formatting into the forwarding core. Owned-to-native adapters re-admit the encoded
metadata before opaque forwarding, so unknown/reserved incoming identifiers stay
carriage rather than being mistaken for reserved origination.
