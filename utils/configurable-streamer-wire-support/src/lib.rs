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

use std::{fmt, str::FromStr};

#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use std::sync::Arc;

#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use up_rust::selected_wire_user_api::{ProtobufWire, StableContainerWireFormat};
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
))]
use up_rust::selected_wire_user_api::{
    ProtobufWireTransport, StableContainerWireTransport, UNativePrefixWireTransport,
};
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
))]
use up_rust::{UCode, UStatus};
#[cfg(not(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
)))]
use up_rust::{UCode, UStatus};
#[cfg(feature = "owned-frame-transport")]
use up_streamer::OwnedFrameEndpoint;
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
))]
use up_streamer::ZeroCopyFrameEndpoint;
use up_streamer::{CopyMinimizedRouteOptions, UStreamer};

#[cfg(feature = "iceoryx2-owned-frame")]
use up_transport_iceoryx2_rust::BenchmarkOwnedIceoryx2Core;
#[cfg(feature = "iceoryx2-zero-copy")]
use up_transport_iceoryx2_rust::Iceoryx2PubSub;
#[cfg(feature = "lola-owned-frame")]
use up_transport_lola_rust::LolaOwnedCore;
#[cfg(feature = "lola-transport")]
use up_transport_lola_rust::LolaZeroCopyCore;
#[cfg(feature = "zenoh-owned-frame")]
use up_transport_zenoh::ZenohOwnedCore;
#[cfg(feature = "zenoh-zero-copy")]
use up_transport_zenoh::ZenohZeroCopyCore;
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "owned-frame-transport"
))]
use up_wire_xcdrv2::XcdrV2Wire;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteWireFormat {
    Native,
    Protobuf,
    XcdrV2,
}

impl RouteWireFormat {
    pub fn parse(value: &str) -> Result<Self, UStatus> {
        value.parse()
    }

    pub const fn as_config_value(self) -> &'static str {
        match self {
            Self::Native => "up_native",
            Self::Protobuf => "protobuf",
            Self::XcdrV2 => "xcdrv2",
        }
    }

    fn label(self) -> &'static str {
        self.as_config_value()
    }
}

impl FromStr for RouteWireFormat {
    type Err = UStatus;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "up_native" | "native" => Ok(Self::Native),
            // Native-prefix protobuf metadata codec, not XCDRv2 metadata.
            "protobuf" => Ok(Self::Protobuf),
            // External XCDRv2 payload encoding carried with native-prefix metadata.
            "xcdrv2" | "xcdr_v2" | "xcdr-v2" => Ok(Self::XcdrV2),
            other => Err(invalid_config(format!(
                "unsupported route wire format: {other}"
            ))),
        }
    }
}

impl fmt::Display for RouteWireFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_config_value())
    }
}

#[derive(Clone)]
pub enum RouteWireEndpoint {
    #[cfg(feature = "zenoh-zero-copy")]
    ZenohNative(ZeroCopyFrameEndpoint<StableContainerWireTransport<ZenohZeroCopyCore>>),
    #[cfg(feature = "zenoh-zero-copy")]
    ZenohProtobuf(ZeroCopyFrameEndpoint<ProtobufWireTransport<ZenohZeroCopyCore>>),
    #[cfg(feature = "zenoh-zero-copy")]
    ZenohXcdrV2(ZeroCopyFrameEndpoint<UNativePrefixWireTransport<ZenohZeroCopyCore, XcdrV2Wire>>),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2Native(ZeroCopyFrameEndpoint<StableContainerWireTransport<Iceoryx2PubSub>>),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2Protobuf(ZeroCopyFrameEndpoint<ProtobufWireTransport<Iceoryx2PubSub>>),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2XcdrV2(ZeroCopyFrameEndpoint<UNativePrefixWireTransport<Iceoryx2PubSub, XcdrV2Wire>>),
    #[cfg(feature = "lola-transport")]
    LolaNative(ZeroCopyFrameEndpoint<StableContainerWireTransport<LolaZeroCopyCore>>),
    #[cfg(feature = "lola-transport")]
    LolaProtobuf(ZeroCopyFrameEndpoint<ProtobufWireTransport<LolaZeroCopyCore>>),
    #[cfg(feature = "lola-transport")]
    LolaXcdrV2(ZeroCopyFrameEndpoint<UNativePrefixWireTransport<LolaZeroCopyCore, XcdrV2Wire>>),
}

#[cfg(feature = "owned-frame-transport")]
#[derive(Clone)]
pub struct RouteOwnedEndpoint {
    endpoint: OwnedFrameEndpoint,
    route_wire_format: RouteWireFormat,
}

#[cfg(feature = "owned-frame-transport")]
impl RouteOwnedEndpoint {
    pub fn endpoint(&self) -> &OwnedFrameEndpoint {
        &self.endpoint
    }

    pub fn route_wire_format(&self) -> RouteWireFormat {
        self.route_wire_format
    }
}

impl RouteWireEndpoint {
    pub fn route_wire_format(&self) -> RouteWireFormat {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(feature = "zenoh-zero-copy")]
            Self::ZenohNative(_) => RouteWireFormat::Native,
            #[cfg(feature = "zenoh-zero-copy")]
            Self::ZenohProtobuf(_) => RouteWireFormat::Protobuf,
            #[cfg(feature = "zenoh-zero-copy")]
            Self::ZenohXcdrV2(_) => RouteWireFormat::XcdrV2,
            #[cfg(feature = "iceoryx2-zero-copy")]
            Self::Iceoryx2Native(_) => RouteWireFormat::Native,
            #[cfg(feature = "iceoryx2-zero-copy")]
            Self::Iceoryx2Protobuf(_) => RouteWireFormat::Protobuf,
            #[cfg(feature = "iceoryx2-zero-copy")]
            Self::Iceoryx2XcdrV2(_) => RouteWireFormat::XcdrV2,
            #[cfg(feature = "lola-transport")]
            Self::LolaNative(_) => RouteWireFormat::Native,
            #[cfg(feature = "lola-transport")]
            Self::LolaProtobuf(_) => RouteWireFormat::Protobuf,
            #[cfg(feature = "lola-transport")]
            Self::LolaXcdrV2(_) => RouteWireFormat::XcdrV2,
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
            Arc::new(core.with_selected_wire(StableContainerWireFormat)),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::ZenohProtobuf(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_selected_wire(ProtobufWire)),
        )),
        RouteWireFormat::XcdrV2 => RouteWireEndpoint::ZenohXcdrV2(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_selected_wire(XcdrV2Wire)),
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
            Arc::new(core.with_selected_wire(StableContainerWireFormat)),
        )),
        RouteWireFormat::Protobuf => {
            RouteWireEndpoint::Iceoryx2Protobuf(ZeroCopyFrameEndpoint::new(
                name,
                authority,
                Arc::new(core.with_selected_wire(ProtobufWire)),
            ))
        }
        RouteWireFormat::XcdrV2 => RouteWireEndpoint::Iceoryx2XcdrV2(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_selected_wire(XcdrV2Wire)),
        )),
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
            Arc::new(core.with_selected_wire(StableContainerWireFormat)),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::LolaProtobuf(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_selected_wire(ProtobufWire)),
        )),
        RouteWireFormat::XcdrV2 => RouteWireEndpoint::LolaXcdrV2(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(core.with_selected_wire(XcdrV2Wire)),
        )),
    }
}

#[cfg(feature = "zenoh-owned-frame")]
pub fn zenoh_owned_endpoint(
    name: &str,
    authority: &str,
    core: ZenohOwnedCore,
    route_wire_format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    let transport: Arc<dyn up_rust::UOwnedTransport> = match route_wire_format {
        RouteWireFormat::Native => Arc::new(core.with_selected_wire(StableContainerWireFormat)),
        RouteWireFormat::Protobuf => Arc::new(core.with_selected_wire(ProtobufWire)),
        RouteWireFormat::XcdrV2 => Arc::new(core.with_selected_wire(XcdrV2Wire)),
    };
    RouteOwnedEndpoint {
        endpoint: OwnedFrameEndpoint::from_owned(name, authority, transport),
        route_wire_format,
    }
}

#[cfg(feature = "iceoryx2-owned-frame")]
pub fn iceoryx2_owned_endpoint(
    name: &str,
    authority: &str,
    core: BenchmarkOwnedIceoryx2Core,
    route_wire_format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    let transport: Arc<dyn up_rust::UOwnedTransport> = match route_wire_format {
        RouteWireFormat::Native => Arc::new(core.with_selected_wire(StableContainerWireFormat)),
        RouteWireFormat::Protobuf => Arc::new(core.with_selected_wire(ProtobufWire)),
        RouteWireFormat::XcdrV2 => Arc::new(core.with_selected_wire(XcdrV2Wire)),
    };
    RouteOwnedEndpoint {
        endpoint: OwnedFrameEndpoint::from_owned(name, authority, transport),
        route_wire_format,
    }
}

#[cfg(feature = "lola-owned-frame")]
pub fn lola_owned_endpoint(
    name: &str,
    authority: &str,
    core: LolaOwnedCore,
    route_wire_format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    let transport: Arc<dyn up_rust::UOwnedTransport> = match route_wire_format {
        RouteWireFormat::Native => Arc::new(core.with_selected_wire(StableContainerWireFormat)),
        RouteWireFormat::Protobuf => Arc::new(core.with_selected_wire(ProtobufWire)),
        RouteWireFormat::XcdrV2 => Arc::new(core.with_selected_wire(XcdrV2Wire)),
    };
    RouteOwnedEndpoint {
        endpoint: OwnedFrameEndpoint::from_owned(name, authority, transport),
        route_wire_format,
    }
}

pub async fn add_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteWireEndpoint,
    egress: &RouteWireEndpoint,
    route_wire_format: RouteWireFormat,
    options: CopyMinimizedRouteOptions,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        ingress.route_wire_format(),
        egress.route_wire_format(),
        route_wire_format,
    )?;

    #[cfg(not(any(
        feature = "zenoh-zero-copy",
        feature = "iceoryx2-zero-copy",
        feature = "lola-transport"
    )))]
    {
        let _ = &mut *streamer;
        let _ = &options;
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
        #[cfg(feature = "zenoh-zero-copy")]
        (RouteWireEndpoint::ZenohXcdrV2(left), RouteWireEndpoint::ZenohXcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "iceoryx2-zero-copy"))]
        (RouteWireEndpoint::ZenohXcdrV2(left), RouteWireEndpoint::Iceoryx2XcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "zenoh-zero-copy", feature = "lola-transport"))]
        (RouteWireEndpoint::ZenohXcdrV2(left), RouteWireEndpoint::LolaXcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "zenoh-zero-copy"))]
        (RouteWireEndpoint::Iceoryx2XcdrV2(left), RouteWireEndpoint::ZenohXcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        (RouteWireEndpoint::Iceoryx2XcdrV2(left), RouteWireEndpoint::Iceoryx2XcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "iceoryx2-zero-copy", feature = "lola-transport"))]
        (RouteWireEndpoint::Iceoryx2XcdrV2(left), RouteWireEndpoint::LolaXcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "zenoh-zero-copy"))]
        (RouteWireEndpoint::LolaXcdrV2(left), RouteWireEndpoint::ZenohXcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(all(feature = "lola-transport", feature = "iceoryx2-zero-copy"))]
        (RouteWireEndpoint::LolaXcdrV2(left), RouteWireEndpoint::Iceoryx2XcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        #[cfg(feature = "lola-transport")]
        (RouteWireEndpoint::LolaXcdrV2(left), RouteWireEndpoint::LolaXcdrV2(right)) => {
            streamer
                .add_selected_wire_copy_minimized_route_ref_with_options(left, right, options)
                .await
        }
        _ => Err(invalid_config(
            "copy_minimized route uses an unsupported route endpoint combination",
        )),
    }
}

#[cfg(feature = "owned-frame-transport")]
pub async fn add_owned_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteOwnedEndpoint,
    egress: &RouteOwnedEndpoint,
    route_wire_format: RouteWireFormat,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        ingress.route_wire_format(),
        egress.route_wire_format(),
        route_wire_format,
    )?;
    streamer
        .add_owned_route_ref(ingress.endpoint(), egress.endpoint())
        .await
}

#[cfg(feature = "owned-frame-transport")]
pub async fn add_owned_to_copy_minimized_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteOwnedEndpoint,
    egress: &RouteWireEndpoint,
    route_wire_format: RouteWireFormat,
    options: CopyMinimizedRouteOptions,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        ingress.route_wire_format(),
        egress.route_wire_format(),
        route_wire_format,
    )?;

    #[allow(unreachable_patterns)]
    match egress {
        #[cfg(feature = "zenoh-zero-copy")]
        RouteWireEndpoint::ZenohNative(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        RouteWireEndpoint::ZenohProtobuf(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        RouteWireEndpoint::ZenohXcdrV2(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        RouteWireEndpoint::Iceoryx2Native(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        RouteWireEndpoint::Iceoryx2Protobuf(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        RouteWireEndpoint::Iceoryx2XcdrV2(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "lola-transport")]
        RouteWireEndpoint::LolaNative(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "lola-transport")]
        RouteWireEndpoint::LolaProtobuf(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        #[cfg(feature = "lola-transport")]
        RouteWireEndpoint::LolaXcdrV2(right) => {
            streamer
                .add_owned_to_copy_minimized_route_ref_with_options(
                    ingress.endpoint(),
                    right,
                    options,
                )
                .await
        }
        _ => Err(invalid_config(
            "owned-frame to copy_minimized route uses an unsupported route endpoint combination",
        )),
    }
}

#[cfg(feature = "owned-frame-transport")]
pub async fn add_copy_minimized_to_owned_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteWireEndpoint,
    egress: &RouteOwnedEndpoint,
    route_wire_format: RouteWireFormat,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        ingress.route_wire_format(),
        egress.route_wire_format(),
        route_wire_format,
    )?;

    #[allow(unreachable_patterns)]
    match ingress {
        #[cfg(feature = "zenoh-zero-copy")]
        RouteWireEndpoint::ZenohNative(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        RouteWireEndpoint::ZenohProtobuf(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "zenoh-zero-copy")]
        RouteWireEndpoint::ZenohXcdrV2(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        RouteWireEndpoint::Iceoryx2Native(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        RouteWireEndpoint::Iceoryx2Protobuf(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "iceoryx2-zero-copy")]
        RouteWireEndpoint::Iceoryx2XcdrV2(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "lola-transport")]
        RouteWireEndpoint::LolaNative(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "lola-transport")]
        RouteWireEndpoint::LolaProtobuf(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        #[cfg(feature = "lola-transport")]
        RouteWireEndpoint::LolaXcdrV2(left) => {
            streamer
                .add_copy_minimized_to_owned_route_ref(left, egress.endpoint())
                .await
        }
        _ => Err(invalid_config(
            "copy_minimized to owned-frame route uses an unsupported route endpoint combination",
        )),
    }
}

pub fn validate_route_wire_formats(
    ingress: RouteWireFormat,
    egress: RouteWireFormat,
    route: RouteWireFormat,
) -> Result<(), UStatus> {
    if ingress != route || egress != route {
        return Err(invalid_config(format!(
            "route wire format {} does not match both route endpoints",
            route.label()
        )));
    }

    Ok(())
}

fn invalid_config(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_wire_format_accepts_legacy_native_alias_and_emits_canonical_config_value() {
        let native = RouteWireFormat::parse("native").expect("native alias parses");

        assert_eq!(native, RouteWireFormat::Native);
        assert_eq!(native.as_config_value(), "up_native");
        assert_eq!(native.to_string(), "up_native");
    }

    #[test]
    fn route_wire_format_rejects_open_plugin_names() {
        let error = RouteWireFormat::parse("third_party_plugin").expect_err("open plugin rejected");

        assert_eq!(error.get_code(), UCode::InvalidArgument);
    }

    #[test]
    fn mismatched_route_wire_endpoint_formats_are_rejected_before_typed_route_call() {
        let error = validate_route_wire_formats(
            RouteWireFormat::Native,
            RouteWireFormat::Protobuf,
            RouteWireFormat::Native,
        )
        .expect_err("mismatch rejected");

        assert_eq!(error.get_code(), UCode::InvalidArgument);
    }

    #[test]
    fn route_wire_format_parse_smoke_bound_is_tiny_for_config_reification() {
        let start = std::time::Instant::now();
        let mut parsed = 0_usize;
        for value in ["up_native", "native", "protobuf", "xcdrv2"]
            .into_iter()
            .cycle()
            .take(10_000)
        {
            RouteWireFormat::parse(value).expect("known wire parses");
            parsed += 1;
        }

        assert_eq!(parsed, 10_000);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "closed route-wire parse smoke should be negligible"
        );
    }
}
