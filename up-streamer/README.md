# up-streamer

Generic native-frame uStreamer for bridging serializer-neutral uProtocol transports.

## Overview

`up-streamer` routes `UOwnedFrame` values between native transport endpoints. Owned transports such as Zenoh are registered with `OwnedFrameEndpoint::from_owned`; zero-copy transports such as iceoryx2 are registered with `OwnedFrameEndpoint::from_zero_copy`.

`OwnedFrameEndpoint` is an adapter boundary. A zero-copy ingress lease is copied into an owned frame before routing, and a zero-copy egress copies the owned frame payload into a transmit loan. This lets one router bridge owned and zero-copy transports, but it is not end-to-end zero-copy forwarding.

The crate does not depend on generated Protocol Buffers envelopes. Payload representation is carried by `UEncoding` and concrete payload codecs use the `WireFormat` traits from `up-rust`.

## Usage

```rust
use std::sync::Arc;
use up_rust::USubscription;
use up_streamer::{OwnedFrameEndpoint, UStreamer};

# async fn example(
#     usubscription: Arc<dyn USubscription>,
#     left_transport: Arc<dyn up_rust::UOwnedTransport>,
#     right_transport: Arc<dyn up_rust::UOwnedTransport>,
# ) -> Result<(), up_rust::UStatus> {
let mut streamer = UStreamer::new("native", 32, usubscription).await?;
let left = OwnedFrameEndpoint::from_owned("left", "left-authority", left_transport);
let right = OwnedFrameEndpoint::from_owned("right", "right-authority", right_transport);

streamer.add_route_ref(&left, &right).await?;
# Ok(())
# }
```

## Transport Modes

- `TransportMode::Owned`: the egress path calls `send_owned` with an owned frame.
- `TransportMode::ZeroCopy`: the endpoint is backed by a zero-copy transport, but streamer routing still crosses the owned-frame adapter boundary.

Zero-copy ingress routes copy receive leases into owned frames and use native subscription snapshots to register exact topic services when needed by transports such as iceoryx2.

## Verification

```bash
cargo check -p up-streamer --all-targets
cargo test -p up-streamer -- --nocapture
cargo test -p up-streamer --test actual_transports -- --nocapture
```
