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

mod endpoint;
pub use endpoint::{Endpoint, TransportMode};

mod subscription_sync_health;
pub use subscription_sync_health::SubscriptionSyncHealth;

mod ustreamer;
pub use ustreamer::UStreamer;
