# up-streamer

Generic native-frame uStreamer for bridging serializer-neutral uProtocol transports.

## Overview

`up-streamer` routes `UOwnedFrame` values between native transport endpoints. Owned transports such as Zenoh are registered with `OwnedFrameEndpoint::from_owned`; zero-copy transports such as iceoryx2 are registered with `OwnedFrameEndpoint::from_zero_copy_copying_adapter`.

`OwnedFrameEndpoint` is an adapter boundary. A zero-copy ingress lease is copied into an owned frame before routing, and a zero-copy egress reserves a transmit loan with final metadata before copying the owned frame payload into that loan. This lets one router bridge owned and zero-copy transports, but it is not end-to-end zero-copy forwarding.

The crate does not depend on generated Protocol Buffers envelopes. Payload representation is carried by `PayloadEncoding` and concrete payload codecs use the `PayloadFormat` traits from `up-rust`.

| Streamer piece | Responsibility |
| --- | --- |
| `UStreamer` | Owns routes and forwards `UOwnedFrame` values. |
| `OwnedFrameEndpoint::from_owned` | Wraps a transport that already sends and receives owned frames. |
| `OwnedFrameEndpoint::from_zero_copy_copying_adapter` | Wraps a true zero-copy transport through an explicit copying adapter. |
| `PayloadFormat` | Chosen by applications at frame boundaries; the streamer does not reinterpret payload bytes. |
| `UFrameWireFormat` | Only used if an application intentionally carries an encoded whole frame as payload bytes. |
| `UStreamer::data_plane_health` | Reports egress send failures, closed ingress queues, and route refresh unregister failures. |

## Usage

```rust
use std::sync::Arc;
use up_rust::USubscription;
use up_streamer::{OwnedFrameEndpoint, UStreamer};

async fn example(
    usubscription: Arc<dyn USubscription>,
    left_transport: Arc<dyn up_rust::UOwnedTransport>,
    right_transport: Arc<dyn up_rust::UOwnedTransport>,
) -> Result<(), up_rust::UStatus> {
let mut streamer = UStreamer::new("native", 32, usubscription).await?;
let left = OwnedFrameEndpoint::from_owned("left", "left-authority", left_transport);
let right = OwnedFrameEndpoint::from_owned("right", "right-authority", right_transport);

streamer.add_route_ref(&left, &right).await?;
Ok(())
}
```

Zero-copy transports can be bridged through the same router, but the adapter copies at the streamer boundary:

```rust
use std::sync::Arc;
use up_rust::zero_copy::UZeroCopyTransport;
use up_rust::USubscription;
use up_streamer::{OwnedFrameEndpoint, UStreamer};

async fn example<T>(
    usubscription: Arc<dyn USubscription>,
    owned_transport: Arc<dyn up_rust::UOwnedTransport>,
    zero_copy_transport: Arc<T>,
) -> Result<(), up_rust::UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
{
let mut streamer = UStreamer::new("native", 32, usubscription).await?;
let owned = OwnedFrameEndpoint::from_owned("owned", "left-authority", owned_transport);
let zero_copy = OwnedFrameEndpoint::from_zero_copy_copying_adapter(
    "shared-memory",
    "right-authority",
    zero_copy_transport,
);

streamer.add_route_ref(&owned, &zero_copy).await?;
Ok(())
}
```

The route above is useful for bridging network/broker transports with shared-memory transports. It should not be described as end-to-end zero-copy forwarding because `UStreamer` routes owned frames internally.

## Transport Modes

- `TransportMode::Owned`: the egress path calls `send_owned` with an owned frame.
- `TransportMode::ZeroCopy`: the endpoint is backed by a zero-copy transport, but streamer routing still crosses the owned-frame adapter boundary.

Zero-copy ingress routes copy receive leases into owned frames and use native subscription snapshots to register exact topic services when needed by transports such as iceoryx2.

## Health

`UStreamer::subscription_sync_health()` reports uSubscription refresh outcomes. `UStreamer::data_plane_health()` reports route data-plane failures such as egress send errors, closed ingress queues, and old listener registrations that could not be removed during route refresh. Egress send failures are logged at `warn` level and reflected in data-plane health; they are not hidden as debug-only telemetry.

## Experimental Copy-Minimized Helper

The `experimental-loaned-frame` feature exposes `send_loaned_frame_copy_minimized` for experiments that hold an ingress `LoanedFrame` lease and copy its ordered payload slices directly into a zero-copy egress transmit loan. This avoids an intermediate owned payload allocation, but it still copies payload bytes into the egress loan and is not zero-copy-preserving forwarding.

## Transport Implementer Checklist

1. Implement `UOwnedTransport` for network, brokered, or in-process transports that own payload buffers.
2. Implement `UZeroCopyTransport` only when the transport can loan transmit storage or return receive leases without hidden copies.
3. Preserve `UAttributes` and `PayloadEncoding` across the transport boundary.
4. Expose only application payload bytes through `payload_mut()`, `payload_reader()`, or `contiguous_payload()`.
5. Use `OwnedFrameEndpoint::from_zero_copy_copying_adapter` only when the streamer intentionally crosses from zero-copy leases into owned routing.

## Verification

```bash
cargo check -p up-streamer --all-targets
cargo test -p up-streamer -- --nocapture
cargo test -p up-streamer --test actual_transports -- --nocapture
```
