# up-streamer

Generic native-frame uStreamer for bridging serializer-neutral uProtocol transports.

## Overview

`up-streamer` routes `UOwnedFrame` values between native transport endpoints. Owned transports such as Zenoh are registered with `Endpoint::from_owned`; zero-copy transports such as iceoryx2 are registered with `Endpoint::from_zero_copy`.

The crate does not depend on generated Protocol Buffers envelopes. Payload representation is carried by `UEncoding` and concrete payload codecs use the `WireFormat` traits from `up-rust`.

## Usage

```rust
use std::sync::Arc;
use up_rust::USubscription;
use up_streamer::{Endpoint, UStreamer};

# async fn example(
#     usubscription: Arc<dyn USubscription>,
#     left_transport: Arc<dyn up_rust::UOwnedTransport>,
#     right_transport: Arc<dyn up_rust::UOwnedTransport>,
# ) -> Result<(), up_rust::UStatus> {
let mut streamer = UStreamer::new("native", 32, usubscription).await?;
let left = Endpoint::from_owned("left", "left-authority", left_transport);
let right = Endpoint::from_owned("right", "right-authority", right_transport);

streamer.add_route_ref(&left, &right).await?;
# Ok(())
# }
```

## Transport Modes

- `TransportMode::Owned`: the egress path calls `send_owned` with an owned frame.
- `TransportMode::ZeroCopy`: the egress path reserves a transport loan and copies the incoming frame payload into it.

Zero-copy ingress routes use native subscription snapshots to register exact topic services when needed by transports such as iceoryx2.

## Verification

```bash
cargo check -p up-streamer --all-targets
cargo test -p up-streamer -- --nocapture
cargo test -p up-streamer --test actual_transports -- --nocapture
```
