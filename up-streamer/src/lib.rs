/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
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

//! Native-frame uStreamer for bridging serializer-neutral uProtocol transports.
//!
//! The public API is based on `UOwnedFrame`, `UOwnedTransport`, and
//! `UZeroCopyTransport`. It intentionally does not expose or depend on generated
//! Protocol Buffers message envelopes.
//!
//! The router itself forwards [`up_rust::UOwnedFrame`] values. Owned transports
//! are wrapped with [`OwnedFrameEndpoint::from_owned`]. True zero-copy transports
//! are wrapped with [`OwnedFrameEndpoint::from_zero_copy_copying_adapter`], which uses
//! [`up_rust::transport::UOwnedFrameEndpoint`] as an adapter boundary: zero-copy
//! ingress leases are copied into owned frames before routing, and owned egress
//! frames are copied into zero-copy transmit loans.
//!
//! This design lets one streamer bridge owned and zero-copy transports without
//! requiring every transport to expose the same concrete receive lease type. It
//! does not claim end-to-end zero-copy forwarding across the streamer boundary.

#![warn(rustdoc::bare_urls, rustdoc::broken_intra_doc_links)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod endpoint;
pub use endpoint::{OwnedFrameEndpoint, TransportMode};

mod subscription_sync_health;
pub use subscription_sync_health::SubscriptionSyncHealth;

mod ustreamer;
pub use ustreamer::UStreamer;
