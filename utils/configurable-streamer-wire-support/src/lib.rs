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

use std::sync::Arc;

use up_rust::{ProtobufWire, UCode, UProtocolNativeWire, UStatus, UWireTransport, UWithWire};
use up_streamer::{CopyMinimizedRouteOptions, UStreamer, ZeroCopyFrameEndpoint};

#[cfg(feature = "iceoryx2-zero-copy")]
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
#[cfg(feature = "lola-transport")]
use up_transport_lola_rust::LolaZeroCopyCore;
#[cfg(feature = "zenoh-zero-copy")]
use up_transport_zenoh::ZenohZeroCopyCore;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteWireFormat {
    Native,
    Protobuf,
}

impl RouteWireFormat {
    pub fn parse(value: &str) -> Result<Self, UStatus> {
        match value {
            "up_native" | "native" => Ok(Self::Native),
            "protobuf" => Ok(Self::Protobuf),
            other => Err(invalid_config(format!(
                "unsupported route wire format: {other}"
            ))),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Native => "up_native",
            Self::Protobuf => "protobuf",
        }
    }
}

#[derive(Clone)]
pub enum RouteWireEndpoint {
    #[cfg(feature = "zenoh-zero-copy")]
    ZenohNative(ZeroCopyFrameEndpoint<UWireTransport<ZenohZeroCopyCore, UProtocolNativeWire>>),
    #[cfg(feature = "zenoh-zero-copy")]
    ZenohProtobuf(ZeroCopyFrameEndpoint<UWireTransport<ZenohZeroCopyCore, ProtobufWire>>),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2Native(ZeroCopyFrameEndpoint<UWireTransport<Iceoryx2PubSub, UProtocolNativeWire>>),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2Protobuf(ZeroCopyFrameEndpoint<UWireTransport<Iceoryx2PubSub, ProtobufWire>>),
    #[cfg(feature = "lola-transport")]
    LolaNative(ZeroCopyFrameEndpoint<UWireTransport<LolaZeroCopyCore, UProtocolNativeWire>>),
    #[cfg(feature = "lola-transport")]
    LolaProtobuf(ZeroCopyFrameEndpoint<UWireTransport<LolaZeroCopyCore, ProtobufWire>>),
}

impl RouteWireEndpoint {
    pub fn route_wire_format(&self) -> RouteWireFormat {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(feature = "zenoh-zero-copy")]
            Self::ZenohNative(_) => RouteWireFormat::Native,
            #[cfg(feature = "zenoh-zero-copy")]
            Self::ZenohProtobuf(_) => RouteWireFormat::Protobuf,
            #[cfg(feature = "iceoryx2-zero-copy")]
            Self::Iceoryx2Native(_) => RouteWireFormat::Native,
            #[cfg(feature = "iceoryx2-zero-copy")]
            Self::Iceoryx2Protobuf(_) => RouteWireFormat::Protobuf,
            #[cfg(feature = "lola-transport")]
            Self::LolaNative(_) => RouteWireFormat::Native,
            #[cfg(feature = "lola-transport")]
            Self::LolaProtobuf(_) => RouteWireFormat::Protobuf,
            _ => unreachable!("no route wire endpoints are available without transport features"),
        }
    }
}

#[cfg(feature = "zenoh-zero-copy")]
pub fn zenoh_endpoint(
    name: &str,
    authority: &str,
    core: ZenohZeroCopyCore,
    route_wire_format: RouteWireFormat,
) -> RouteWireEndpoint {
    match route_wire_format {
        RouteWireFormat::Native => RouteWireEndpoint::ZenohNative(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_wire(UProtocolNativeWire)),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::ZenohProtobuf(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_wire(ProtobufWire)),
        )),
    }
}

#[cfg(feature = "iceoryx2-zero-copy")]
pub fn iceoryx2_endpoint(
    name: &str,
    authority: &str,
    core: Iceoryx2PubSub,
    route_wire_format: RouteWireFormat,
) -> RouteWireEndpoint {
    match route_wire_format {
        RouteWireFormat::Native => RouteWireEndpoint::Iceoryx2Native(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_wire(UProtocolNativeWire)),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::Iceoryx2Protobuf(
            ZeroCopyFrameEndpoint::new(name, authority, Arc::new(core.with_wire(ProtobufWire))),
        ),
    }
}

#[cfg(feature = "lola-transport")]
pub fn lola_endpoint(
    name: &str,
    authority: &str,
    core: LolaZeroCopyCore,
    route_wire_format: RouteWireFormat,
) -> RouteWireEndpoint {
    match route_wire_format {
        RouteWireFormat::Native => RouteWireEndpoint::LolaNative(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_wire(UProtocolNativeWire)),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::LolaProtobuf(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_wire(ProtobufWire)),
        )),
    }
}

pub async fn add_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteWireEndpoint,
    egress: &RouteWireEndpoint,
    route_wire_format: RouteWireFormat,
    options: CopyMinimizedRouteOptions,
) -> Result<(), UStatus> {
    if ingress.route_wire_format() != route_wire_format
        || egress.route_wire_format() != route_wire_format
    {
        return Err(invalid_config(format!(
            "route wire format {} does not match both route endpoints",
            route_wire_format.label()
        )));
    }

    #[allow(unreachable_patterns)]
    match (ingress, egress) {
        #[cfg(feature = "zenoh-zero-copy")]
        (RouteWireEndpoint::ZenohNative(left), RouteWireEndpoint::ZenohNative(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "iceoryx2-zero-copy"))]
        (RouteWireEndpoint::ZenohNative(left), RouteWireEndpoint::Iceoryx2Native(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "lola-transport"))]
        (RouteWireEndpoint::ZenohNative(left), RouteWireEndpoint::LolaNative(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "zenoh-zero-copy"))]
        (RouteWireEndpoint::Iceoryx2Native(left), RouteWireEndpoint::ZenohNative(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (RouteWireEndpoint::Iceoryx2Native(left), RouteWireEndpoint::Iceoryx2Native(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "lola-transport"))]
        (RouteWireEndpoint::Iceoryx2Native(left), RouteWireEndpoint::LolaNative(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "zenoh-zero-copy"))]
        (RouteWireEndpoint::LolaNative(left), RouteWireEndpoint::ZenohNative(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "iceoryx2-zero-copy"))]
        (RouteWireEndpoint::LolaNative(left), RouteWireEndpoint::Iceoryx2Native(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "lola-transport")]
        (RouteWireEndpoint::LolaNative(left), RouteWireEndpoint::LolaNative(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        (RouteWireEndpoint::ZenohProtobuf(left), RouteWireEndpoint::ZenohProtobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "iceoryx2-zero-copy"))]
        (RouteWireEndpoint::ZenohProtobuf(left), RouteWireEndpoint::Iceoryx2Protobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "lola-transport"))]
        (RouteWireEndpoint::ZenohProtobuf(left), RouteWireEndpoint::LolaProtobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "zenoh-zero-copy"))]
        (RouteWireEndpoint::Iceoryx2Protobuf(left), RouteWireEndpoint::ZenohProtobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (RouteWireEndpoint::Iceoryx2Protobuf(left), RouteWireEndpoint::Iceoryx2Protobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "lola-transport"))]
        (RouteWireEndpoint::Iceoryx2Protobuf(left), RouteWireEndpoint::LolaProtobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "zenoh-zero-copy"))]
        (RouteWireEndpoint::LolaProtobuf(left), RouteWireEndpoint::ZenohProtobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "iceoryx2-zero-copy"))]
        (RouteWireEndpoint::LolaProtobuf(left), RouteWireEndpoint::Iceoryx2Protobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "lola-transport")]
        (RouteWireEndpoint::LolaProtobuf(left), RouteWireEndpoint::LolaProtobuf(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        _ => Err(invalid_config(
            "copy_minimized route uses an unsupported route endpoint combination",
        )),
    }
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}
