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
    feature = "lola-transport"
))]
use std::sync::Arc;

#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
))]
use up_rust::{
    NativePrefixProtobufMetadataCodec, ProtobufWire, UCode, UProtocolNativeWire, UStatus,
    UWireTransport,
};
#[cfg(not(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
)))]
use up_rust::{UCode, UStatus};
#[cfg(any(
    feature = "zenoh-zero-copy",
    feature = "iceoryx2-zero-copy",
    feature = "lola-transport"
))]
use up_streamer::ZeroCopyFrameEndpoint;
use up_streamer::{CopyMinimizedRouteOptions, UStreamer};

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
        value.parse()
    }

    pub const fn as_config_value(self) -> &'static str {
        match self {
            Self::Native => "up_native",
            Self::Protobuf => "protobuf",
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
    ZenohNative(
        ZeroCopyFrameEndpoint<
            UWireTransport<
                ZenohZeroCopyCore,
                UProtocolNativeWire,
                NativePrefixProtobufMetadataCodec,
            >,
        >,
    ),
    #[cfg(feature = "zenoh-zero-copy")]
    ZenohProtobuf(
        ZeroCopyFrameEndpoint<
            UWireTransport<ZenohZeroCopyCore, ProtobufWire, NativePrefixProtobufMetadataCodec>,
        >,
    ),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2Native(
        ZeroCopyFrameEndpoint<
            UWireTransport<Iceoryx2PubSub, UProtocolNativeWire, NativePrefixProtobufMetadataCodec>,
        >,
    ),
    #[cfg(feature = "iceoryx2-zero-copy")]
    Iceoryx2Protobuf(
        ZeroCopyFrameEndpoint<
            UWireTransport<Iceoryx2PubSub, ProtobufWire, NativePrefixProtobufMetadataCodec>,
        >,
    ),
    #[cfg(feature = "lola-transport")]
    LolaNative(
        ZeroCopyFrameEndpoint<
            UWireTransport<
                LolaZeroCopyCore,
                UProtocolNativeWire,
                NativePrefixProtobufMetadataCodec,
            >,
        >,
    ),
    #[cfg(feature = "lola-transport")]
    LolaProtobuf(
        ZeroCopyFrameEndpoint<
            UWireTransport<LolaZeroCopyCore, ProtobufWire, NativePrefixProtobufMetadataCodec>,
        >,
    ),
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
            Arc::new(UWireTransport::new(
                core,
                UProtocolNativeWire,
                NativePrefixProtobufMetadataCodec,
            )),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::ZenohProtobuf(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(UWireTransport::new(
                core,
                ProtobufWire,
                NativePrefixProtobufMetadataCodec,
            )),
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
            Arc::new(UWireTransport::new(
                core,
                UProtocolNativeWire,
                NativePrefixProtobufMetadataCodec,
            )),
        )),
        RouteWireFormat::Protobuf => {
            RouteWireEndpoint::Iceoryx2Protobuf(ZeroCopyFrameEndpoint::new(
                name,
                authority,
                Arc::new(UWireTransport::new(
                    core,
                    ProtobufWire,
                    NativePrefixProtobufMetadataCodec,
                )),
            ))
        }
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
            Arc::new(UWireTransport::new(
                core,
                UProtocolNativeWire,
                NativePrefixProtobufMetadataCodec,
            )),
        )),
        RouteWireFormat::Protobuf => RouteWireEndpoint::LolaProtobuf(ZeroCopyFrameEndpoint::new(
            name,
            authority,
            Arc::new(UWireTransport::new(
                core,
                ProtobufWire,
                NativePrefixProtobufMetadataCodec,
            )),
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
        _ => Err(invalid_config(
            "copy_minimized route uses an unsupported route endpoint combination",
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
        for value in ["up_native", "native", "protobuf"]
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
