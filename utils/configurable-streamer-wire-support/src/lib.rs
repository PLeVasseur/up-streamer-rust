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
    feature = "dds-zero-copy",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame",
    feature = "dds-owned-frame"
))]
use std::sync::Arc;
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "dds-zero-copy",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use up_rust::selected_wire_user_api::{
    ProtobufWire, ProtobufWireTransport, StableContainerWireFormat, StableContainerWireTransport,
    UNativePrefixWireTransport, UWithNativePrefixWire as _,
};
use up_rust::{UCode, UStatus};
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "dds-zero-copy"
))]
use up_streamer::ZeroCopyFrameEndpoint;
use up_streamer::{CopyMinimizedRouteOptions, UStreamer};
#[cfg(feature = "owned-frame-transport")]
use up_streamer::{Endpoint, OwnedFrameEndpoint};

#[cfg(feature = "dds-owned-frame")]
use up_transport_dds::owned::UPTransportDdsOwned;
#[cfg(feature = "dds-zero-copy")]
use up_transport_dds::zero_copy::DdsZeroCopyCore;
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
    feature = "dds-zero-copy",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use up_wire_arrow::ArrowWire;
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "dds-zero-copy",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use up_wire_omgidl::OmgIdlWire;
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport",
    feature = "dds-zero-copy",
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
use up_wire_xcdrv2::XcdrV2Wire;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RouteWireFormat {
    Native,
    Protobuf,
    XcdrV2,
    Arrow,
    OmgIdl,
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
            Self::Arrow => "arrow",
            Self::OmgIdl => "omgidl",
        }
    }
}

impl FromStr for RouteWireFormat {
    type Err = UStatus;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "up_native" | "native" => Ok(Self::Native),
            "protobuf" => Ok(Self::Protobuf),
            "xcdrv2" | "xcdr_v2" | "xcdr-v2" => Ok(Self::XcdrV2),
            "arrow" | "arrow_ipc" | "arrow-ipc" => Ok(Self::Arrow),
            "omgidl" | "omg_idl" | "omg-idl" => Ok(Self::OmgIdl),
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

macro_rules! selected_transport_type {
    (native, $core:ty) => {
        StableContainerWireTransport<$core>
    };
    (protobuf, $core:ty) => {
        ProtobufWireTransport<$core>
    };
    (xcdrv2, $core:ty) => {
        UNativePrefixWireTransport<$core, XcdrV2Wire>
    };
    (arrow, $core:ty) => {
        UNativePrefixWireTransport<$core, ArrowWire>
    };
    (omgidl, $core:ty) => {
        UNativePrefixWireTransport<$core, OmgIdlWire>
    };
}

macro_rules! define_route_wire_family {
    ($name:ident, $wire:ident) => {
        #[derive(Clone)]
        pub enum $name {
            #[cfg(feature = "zenoh-zero-copy")]
            Zenoh(ZeroCopyFrameEndpoint<selected_transport_type!($wire, ZenohZeroCopyCore)>),
            #[cfg(feature = "iceoryx2-zero-copy")]
            Iceoryx2(ZeroCopyFrameEndpoint<selected_transport_type!($wire, Iceoryx2PubSub)>),
            #[cfg(feature = "lola-transport")]
            Lola(ZeroCopyFrameEndpoint<selected_transport_type!($wire, LolaZeroCopyCore)>),
            #[cfg(feature = "dds-zero-copy")]
            Dds(ZeroCopyFrameEndpoint<selected_transport_type!($wire, DdsZeroCopyCore)>),
        }

        impl $name {
            async fn add_route(
                &self,
                streamer: &mut UStreamer,
                egress: &Self,
                options: CopyMinimizedRouteOptions,
            ) -> Result<(), UStatus> {
                #[allow(unreachable_patterns)]
                match (self, egress) {
                    #[cfg(feature = "zenoh-zero-copy")]
                    (Self::Zenoh(left), Self::Zenoh(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "zenoh-zero-copy", feature = "iceoryx2-zero-copy"))]
                    (Self::Zenoh(left), Self::Iceoryx2(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "zenoh-zero-copy", feature = "lola-transport"))]
                    (Self::Zenoh(left), Self::Lola(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "zenoh-zero-copy", feature = "dds-zero-copy"))]
                    (Self::Zenoh(left), Self::Dds(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "iceoryx2-zero-copy", feature = "zenoh-zero-copy"))]
                    (Self::Iceoryx2(left), Self::Zenoh(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(feature = "iceoryx2-zero-copy")]
                    (Self::Iceoryx2(left), Self::Iceoryx2(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "iceoryx2-zero-copy", feature = "lola-transport"))]
                    (Self::Iceoryx2(left), Self::Lola(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "iceoryx2-zero-copy", feature = "dds-zero-copy"))]
                    (Self::Iceoryx2(left), Self::Dds(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "lola-transport", feature = "zenoh-zero-copy"))]
                    (Self::Lola(left), Self::Zenoh(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "lola-transport", feature = "iceoryx2-zero-copy"))]
                    (Self::Lola(left), Self::Iceoryx2(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(feature = "lola-transport")]
                    (Self::Lola(left), Self::Lola(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "lola-transport", feature = "dds-zero-copy"))]
                    (Self::Lola(left), Self::Dds(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "dds-zero-copy", feature = "zenoh-zero-copy"))]
                    (Self::Dds(left), Self::Zenoh(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "dds-zero-copy", feature = "iceoryx2-zero-copy"))]
                    (Self::Dds(left), Self::Iceoryx2(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(all(feature = "dds-zero-copy", feature = "lola-transport"))]
                    (Self::Dds(left), Self::Lola(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    #[cfg(feature = "dds-zero-copy")]
                    (Self::Dds(left), Self::Dds(right)) => streamer.add_selected_wire_copy_minimized_route_ref_with_options(left, right, options).await,
                    _ => Err(invalid_config("copy_minimized route uses an unsupported route endpoint combination")),
                }
            }

            #[cfg(feature = "owned-frame-transport")]
            async fn add_classic_to(
                &self,
                streamer: &mut UStreamer,
                ingress: &Endpoint,
                options: CopyMinimizedRouteOptions,
            ) -> Result<(), UStatus> {
                #[allow(unreachable_patterns)]
                match self {
                    #[cfg(feature = "zenoh-zero-copy")]
                    Self::Zenoh(endpoint) => streamer.add_classic_to_copy_minimized_route_ref(ingress, endpoint, options).await,
                    #[cfg(feature = "iceoryx2-zero-copy")]
                    Self::Iceoryx2(endpoint) => streamer.add_classic_to_copy_minimized_route_ref(ingress, endpoint, options).await,
                    #[cfg(feature = "lola-transport")]
                    Self::Lola(endpoint) => streamer.add_classic_to_copy_minimized_route_ref(ingress, endpoint, options).await,
                    #[cfg(feature = "dds-zero-copy")]
                    Self::Dds(endpoint) => streamer.add_classic_to_copy_minimized_route_ref(ingress, endpoint, options).await,
                    _ => Err(invalid_config("classic to copy_minimized route uses an unsupported route endpoint combination")),
                }
            }

            #[cfg(feature = "owned-frame-transport")]
            async fn add_to_classic(
                &self,
                streamer: &mut UStreamer,
                egress: &Endpoint,
            ) -> Result<(), UStatus> {
                #[allow(unreachable_patterns)]
                match self {
                    #[cfg(feature = "zenoh-zero-copy")]
                    Self::Zenoh(endpoint) => streamer.add_copy_minimized_to_classic_route_ref(endpoint, egress).await,
                    #[cfg(feature = "iceoryx2-zero-copy")]
                    Self::Iceoryx2(endpoint) => streamer.add_copy_minimized_to_classic_route_ref(endpoint, egress).await,
                    #[cfg(feature = "lola-transport")]
                    Self::Lola(endpoint) => streamer.add_copy_minimized_to_classic_route_ref(endpoint, egress).await,
                    #[cfg(feature = "dds-zero-copy")]
                    Self::Dds(endpoint) => streamer.add_copy_minimized_to_classic_route_ref(endpoint, egress).await,
                    _ => Err(invalid_config("copy_minimized to classic route uses an unsupported route endpoint combination")),
                }
            }

            #[cfg(feature = "owned-frame-transport")]
            async fn add_owned_to(
                &self,
                streamer: &mut UStreamer,
                ingress: &OwnedFrameEndpoint,
                options: CopyMinimizedRouteOptions,
            ) -> Result<(), UStatus> {
                #[allow(unreachable_patterns)]
                match self {
                    #[cfg(feature = "zenoh-zero-copy")]
                    Self::Zenoh(endpoint) => streamer.add_owned_to_copy_minimized_route_ref_with_options(ingress, endpoint, options).await,
                    #[cfg(feature = "iceoryx2-zero-copy")]
                    Self::Iceoryx2(endpoint) => streamer.add_owned_to_copy_minimized_route_ref_with_options(ingress, endpoint, options).await,
                    #[cfg(feature = "lola-transport")]
                    Self::Lola(endpoint) => streamer.add_owned_to_copy_minimized_route_ref_with_options(ingress, endpoint, options).await,
                    #[cfg(feature = "dds-zero-copy")]
                    Self::Dds(endpoint) => streamer.add_owned_to_copy_minimized_route_ref_with_options(ingress, endpoint, options).await,
                    _ => Err(invalid_config("owned-frame to copy_minimized route uses an unsupported route endpoint combination")),
                }
            }

            #[cfg(feature = "owned-frame-transport")]
            async fn add_to_owned(
                &self,
                streamer: &mut UStreamer,
                egress: &OwnedFrameEndpoint,
            ) -> Result<(), UStatus> {
                #[allow(unreachable_patterns)]
                match self {
                    #[cfg(feature = "zenoh-zero-copy")]
                    Self::Zenoh(endpoint) => streamer.add_copy_minimized_to_owned_route_ref(endpoint, egress).await,
                    #[cfg(feature = "iceoryx2-zero-copy")]
                    Self::Iceoryx2(endpoint) => streamer.add_copy_minimized_to_owned_route_ref(endpoint, egress).await,
                    #[cfg(feature = "lola-transport")]
                    Self::Lola(endpoint) => streamer.add_copy_minimized_to_owned_route_ref(endpoint, egress).await,
                    #[cfg(feature = "dds-zero-copy")]
                    Self::Dds(endpoint) => streamer.add_copy_minimized_to_owned_route_ref(endpoint, egress).await,
                    _ => Err(invalid_config("copy_minimized to owned-frame route uses an unsupported route endpoint combination")),
                }
            }
        }
    };
}

define_route_wire_family!(NativeRouteEndpoint, native);
define_route_wire_family!(ProtobufRouteEndpoint, protobuf);
define_route_wire_family!(XcdrV2RouteEndpoint, xcdrv2);
define_route_wire_family!(ArrowRouteEndpoint, arrow);
define_route_wire_family!(OmgIdlRouteEndpoint, omgidl);

#[derive(Clone)]
pub enum RouteWireEndpoint {
    Native(NativeRouteEndpoint),
    Protobuf(ProtobufRouteEndpoint),
    XcdrV2(XcdrV2RouteEndpoint),
    Arrow(ArrowRouteEndpoint),
    OmgIdl(OmgIdlRouteEndpoint),
}

impl RouteWireEndpoint {
    pub const fn route_wire_format(&self) -> RouteWireFormat {
        match self {
            Self::Native(_) => RouteWireFormat::Native,
            Self::Protobuf(_) => RouteWireFormat::Protobuf,
            Self::XcdrV2(_) => RouteWireFormat::XcdrV2,
            Self::Arrow(_) => RouteWireFormat::Arrow,
            Self::OmgIdl(_) => RouteWireFormat::OmgIdl,
        }
    }
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

    pub const fn route_wire_format(&self) -> RouteWireFormat {
        self.route_wire_format
    }
}

macro_rules! make_route_endpoint {
    ($transport:ident, $name:expr, $authority:expr, $core:expr, $format:expr) => {
        match $format {
            RouteWireFormat::Native => RouteWireEndpoint::Native(NativeRouteEndpoint::$transport(
                ZeroCopyFrameEndpoint::new(
                    $name,
                    $authority,
                    Arc::new($core.into_native_prefix_wire_transport(StableContainerWireFormat)),
                ),
            )),
            RouteWireFormat::Protobuf => RouteWireEndpoint::Protobuf(
                ProtobufRouteEndpoint::$transport(ZeroCopyFrameEndpoint::new(
                    $name,
                    $authority,
                    Arc::new($core.into_native_prefix_wire_transport(ProtobufWire)),
                )),
            ),
            RouteWireFormat::XcdrV2 => RouteWireEndpoint::XcdrV2(XcdrV2RouteEndpoint::$transport(
                ZeroCopyFrameEndpoint::new(
                    $name,
                    $authority,
                    Arc::new($core.into_native_prefix_wire_transport(XcdrV2Wire)),
                ),
            )),
            RouteWireFormat::Arrow => RouteWireEndpoint::Arrow(ArrowRouteEndpoint::$transport(
                ZeroCopyFrameEndpoint::new(
                    $name,
                    $authority,
                    Arc::new($core.into_native_prefix_wire_transport(ArrowWire)),
                ),
            )),
            RouteWireFormat::OmgIdl => RouteWireEndpoint::OmgIdl(OmgIdlRouteEndpoint::$transport(
                ZeroCopyFrameEndpoint::new(
                    $name,
                    $authority,
                    Arc::new($core.into_native_prefix_wire_transport(OmgIdlWire)),
                ),
            )),
        }
    };
}

#[cfg(feature = "zenoh-zero-copy")]
pub fn zenoh_endpoint(
    name: &str,
    authority: &str,
    core: ZenohZeroCopyCore,
    format: RouteWireFormat,
) -> RouteWireEndpoint {
    make_route_endpoint!(Zenoh, name, authority, core, format)
}

#[cfg(feature = "iceoryx2-zero-copy")]
pub fn iceoryx2_endpoint(
    name: &str,
    authority: &str,
    core: Iceoryx2PubSub,
    format: RouteWireFormat,
) -> RouteWireEndpoint {
    make_route_endpoint!(Iceoryx2, name, authority, core, format)
}

#[cfg(feature = "lola-transport")]
pub fn lola_endpoint(
    name: &str,
    authority: &str,
    core: LolaZeroCopyCore,
    format: RouteWireFormat,
) -> RouteWireEndpoint {
    make_route_endpoint!(Lola, name, authority, core, format)
}

#[cfg(feature = "dds-zero-copy")]
pub fn dds_endpoint(
    name: &str,
    authority: &str,
    core: DdsZeroCopyCore,
    format: RouteWireFormat,
) -> RouteWireEndpoint {
    make_route_endpoint!(Dds, name, authority, core, format)
}

#[cfg(feature = "owned-frame-transport")]
fn owned_endpoint(
    name: &str,
    authority: &str,
    transport: Arc<dyn up_rust::UOwnedTransport>,
    route_wire_format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    RouteOwnedEndpoint {
        endpoint: OwnedFrameEndpoint::from_owned(name, authority, transport),
        route_wire_format,
    }
}

#[cfg(any(
    feature = "zenoh-owned-frame",
    feature = "iceoryx2-owned-frame",
    feature = "lola-owned-frame"
))]
macro_rules! make_owned_endpoint {
    ($name:expr, $authority:expr, $core:expr, $format:expr) => {{
        let transport: Arc<dyn up_rust::UOwnedTransport> = match $format {
            RouteWireFormat::Native => {
                Arc::new($core.with_selected_wire(StableContainerWireFormat))
            }
            RouteWireFormat::Protobuf => Arc::new($core.with_selected_wire(ProtobufWire)),
            RouteWireFormat::XcdrV2 => Arc::new($core.with_selected_wire(XcdrV2Wire)),
            RouteWireFormat::Arrow => Arc::new($core.with_selected_wire(ArrowWire)),
            RouteWireFormat::OmgIdl => Arc::new($core.with_selected_wire(OmgIdlWire)),
        };
        owned_endpoint($name, $authority, transport, $format)
    }};
}

#[cfg(feature = "zenoh-owned-frame")]
pub fn zenoh_owned_endpoint(
    name: &str,
    authority: &str,
    core: ZenohOwnedCore,
    format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    make_owned_endpoint!(name, authority, core, format)
}

#[cfg(feature = "iceoryx2-owned-frame")]
pub fn iceoryx2_owned_endpoint(
    name: &str,
    authority: &str,
    core: BenchmarkOwnedIceoryx2Core,
    format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    make_owned_endpoint!(name, authority, core, format)
}

#[cfg(feature = "lola-owned-frame")]
pub fn lola_owned_endpoint(
    name: &str,
    authority: &str,
    core: LolaOwnedCore,
    format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    make_owned_endpoint!(name, authority, core, format)
}

#[cfg(feature = "dds-owned-frame")]
pub fn dds_owned_endpoint(
    name: &str,
    authority: &str,
    transport: UPTransportDdsOwned,
    format: RouteWireFormat,
) -> RouteOwnedEndpoint {
    owned_endpoint(name, authority, Arc::new(transport), format)
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
    match (ingress, egress) {
        (RouteWireEndpoint::Native(left), RouteWireEndpoint::Native(right)) => {
            left.add_route(streamer, right, options).await
        }
        (RouteWireEndpoint::Protobuf(left), RouteWireEndpoint::Protobuf(right)) => {
            left.add_route(streamer, right, options).await
        }
        (RouteWireEndpoint::XcdrV2(left), RouteWireEndpoint::XcdrV2(right)) => {
            left.add_route(streamer, right, options).await
        }
        (RouteWireEndpoint::Arrow(left), RouteWireEndpoint::Arrow(right)) => {
            left.add_route(streamer, right, options).await
        }
        (RouteWireEndpoint::OmgIdl(left), RouteWireEndpoint::OmgIdl(right)) => {
            left.add_route(streamer, right, options).await
        }
        _ => Err(invalid_config(
            "copy_minimized route wire endpoint types do not match",
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
pub async fn add_classic_to_copy_minimized_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &Endpoint,
    egress: &RouteWireEndpoint,
    route_wire_format: RouteWireFormat,
    options: CopyMinimizedRouteOptions,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        egress.route_wire_format(),
        egress.route_wire_format(),
        route_wire_format,
    )?;
    match egress {
        RouteWireEndpoint::Native(endpoint) => {
            endpoint.add_classic_to(streamer, ingress, options).await
        }
        RouteWireEndpoint::Protobuf(endpoint) => {
            endpoint.add_classic_to(streamer, ingress, options).await
        }
        RouteWireEndpoint::XcdrV2(endpoint) => {
            endpoint.add_classic_to(streamer, ingress, options).await
        }
        RouteWireEndpoint::Arrow(endpoint) => {
            endpoint.add_classic_to(streamer, ingress, options).await
        }
        RouteWireEndpoint::OmgIdl(endpoint) => {
            endpoint.add_classic_to(streamer, ingress, options).await
        }
    }
}

#[cfg(feature = "owned-frame-transport")]
pub async fn add_copy_minimized_to_classic_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteWireEndpoint,
    egress: &Endpoint,
    route_wire_format: RouteWireFormat,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        ingress.route_wire_format(),
        ingress.route_wire_format(),
        route_wire_format,
    )?;
    match ingress {
        RouteWireEndpoint::Native(endpoint) => endpoint.add_to_classic(streamer, egress).await,
        RouteWireEndpoint::Protobuf(endpoint) => endpoint.add_to_classic(streamer, egress).await,
        RouteWireEndpoint::XcdrV2(endpoint) => endpoint.add_to_classic(streamer, egress).await,
        RouteWireEndpoint::Arrow(endpoint) => endpoint.add_to_classic(streamer, egress).await,
        RouteWireEndpoint::OmgIdl(endpoint) => endpoint.add_to_classic(streamer, egress).await,
    }
}

#[cfg(feature = "owned-frame-transport")]
pub async fn add_classic_to_owned_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &Endpoint,
    egress: &RouteOwnedEndpoint,
    route_wire_format: RouteWireFormat,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        egress.route_wire_format(),
        egress.route_wire_format(),
        route_wire_format,
    )?;
    streamer
        .add_classic_to_owned_route_ref(ingress, egress.endpoint())
        .await
}

#[cfg(feature = "owned-frame-transport")]
pub async fn add_owned_to_classic_route_wire_format(
    streamer: &mut UStreamer,
    ingress: &RouteOwnedEndpoint,
    egress: &Endpoint,
    route_wire_format: RouteWireFormat,
) -> Result<(), UStatus> {
    validate_route_wire_formats(
        ingress.route_wire_format(),
        ingress.route_wire_format(),
        route_wire_format,
    )?;
    streamer
        .add_owned_to_classic_route_ref(ingress.endpoint(), egress)
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
    match egress {
        RouteWireEndpoint::Native(endpoint) => {
            endpoint
                .add_owned_to(streamer, ingress.endpoint(), options)
                .await
        }
        RouteWireEndpoint::Protobuf(endpoint) => {
            endpoint
                .add_owned_to(streamer, ingress.endpoint(), options)
                .await
        }
        RouteWireEndpoint::XcdrV2(endpoint) => {
            endpoint
                .add_owned_to(streamer, ingress.endpoint(), options)
                .await
        }
        RouteWireEndpoint::Arrow(endpoint) => {
            endpoint
                .add_owned_to(streamer, ingress.endpoint(), options)
                .await
        }
        RouteWireEndpoint::OmgIdl(endpoint) => {
            endpoint
                .add_owned_to(streamer, ingress.endpoint(), options)
                .await
        }
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
    match ingress {
        RouteWireEndpoint::Native(endpoint) => {
            endpoint.add_to_owned(streamer, egress.endpoint()).await
        }
        RouteWireEndpoint::Protobuf(endpoint) => {
            endpoint.add_to_owned(streamer, egress.endpoint()).await
        }
        RouteWireEndpoint::XcdrV2(endpoint) => {
            endpoint.add_to_owned(streamer, egress.endpoint()).await
        }
        RouteWireEndpoint::Arrow(endpoint) => {
            endpoint.add_to_owned(streamer, egress.endpoint()).await
        }
        RouteWireEndpoint::OmgIdl(endpoint) => {
            endpoint.add_to_owned(streamer, egress.endpoint()).await
        }
    }
}

pub fn validate_route_wire_formats(
    ingress: RouteWireFormat,
    egress: RouteWireFormat,
    route: RouteWireFormat,
) -> Result<(), UStatus> {
    if ingress != route || egress != route {
        return Err(invalid_config(format!(
            "route wire format {route} does not match both route endpoints"
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
    fn route_wire_format_accepts_all_closed_formats_and_aliases() {
        let cases = [
            ("native", RouteWireFormat::Native, "up_native"),
            ("protobuf", RouteWireFormat::Protobuf, "protobuf"),
            ("xcdr-v2", RouteWireFormat::XcdrV2, "xcdrv2"),
            ("arrow-ipc", RouteWireFormat::Arrow, "arrow"),
            ("omg-idl", RouteWireFormat::OmgIdl, "omgidl"),
        ];
        for (input, expected, canonical) in cases {
            let parsed = RouteWireFormat::parse(input).expect("known wire parses");
            assert_eq!(parsed, expected);
            assert_eq!(parsed.as_config_value(), canonical);
        }
    }

    #[test]
    fn route_wire_format_rejects_open_plugin_names() {
        let error = RouteWireFormat::parse("third_party_plugin").expect_err("open plugin rejected");
        assert_eq!(error.code(), UCode::InvalidArgument);
    }

    #[test]
    fn mismatched_route_wire_endpoint_formats_are_rejected_before_typed_route_call() {
        let error = validate_route_wire_formats(
            RouteWireFormat::Arrow,
            RouteWireFormat::OmgIdl,
            RouteWireFormat::Arrow,
        )
        .expect_err("mismatch rejected");
        assert_eq!(error.code(), UCode::InvalidArgument);
    }
}
