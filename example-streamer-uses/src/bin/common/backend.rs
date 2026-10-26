// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Binding construction shared by the role entrypoints and wire selectors.
#[cfg(feature = "mqtt-transport")]
pub async fn mqtt(
    options: up_transport_mqtt5::Mqtt5TransportOptions,
    authority: impl Into<String>,
) -> Result<up_transport_mqtt5::Mqtt5Transport, up_rust::UStatus> {
    let transport = up_transport_mqtt5::Mqtt5Transport::new(options, authority).await?;
    transport.connect().await?;
    Ok(transport)
}
#[cfg(feature = "vsomeip-transport")]
pub fn someip(
    uri: up_rust::UUri,
    remote: &up_transport_vsomeip::AuthorityName,
    config: &std::path::Path,
    encoding: up_rust::PayloadEncoding,
) -> Result<up_transport_vsomeip::UPTransportVsomeip, up_rust::UStatus> {
    up_transport_vsomeip::UPTransportVsomeip::new_with_config_and_transport_config(
        uri,
        remote,
        config,
        None,
        up_transport_vsomeip::TransportConfig::new(encoding),
    )
}
#[cfg(feature = "zenoh-transport")]
pub async fn zenoh(
    config: up_transport_zenoh::zenoh_config::Config,
    uri: impl Into<String>,
) -> Result<up_transport_zenoh::UPTransportZenoh, up_rust::UStatus> {
    up_transport_zenoh::UPTransportZenoh::new(config, uri).await
}
#[cfg(feature = "zenoh-transport")]
pub async fn zenoh_owned(
    config: up_transport_zenoh::zenoh_config::Config,
    uri: impl Into<String>,
) -> Result<up_transport_zenoh::ZenohOwnedCore, up_rust::UStatus> {
    up_transport_zenoh::ZenohOwnedCore::new(config, uri).await
}
#[cfg(feature = "zenoh-transport")]
pub async fn zenoh_owned_selected(
    config: up_transport_zenoh::zenoh_config::Config,
    uri: impl Into<String>,
    encoding: super::encoding::Encoding,
    native: &super::payloads::NativeContext,
) -> Result<std::sync::Arc<dyn up_rust::UOwnedTransport>, up_rust::UStatus> {
    use super::encoding::Encoding;
    use std::sync::Arc;
    use up_rust::{UOwnedTransport, UWithNativePrefixWire};
    let core = zenoh_owned(config, uri).await?;
    let transport: Arc<dyn UOwnedTransport> = match encoding {
        Encoding::Native => {
            Arc::new(core.into_stable_container_transport(native.agreement()?.clone()))
        }
        Encoding::Protobuf => Arc::new(core.with_selected_wire(up_rust::ProtobufWire)),
        Encoding::Xcdrv2 => Arc::new(core.with_selected_wire(up_wire_xcdrv2::XcdrV2Wire)),
        Encoding::Arrow => Arc::new(core.with_selected_wire(up_wire_arrow::ArrowWire)),
        Encoding::Omgidl => Arc::new(core.with_selected_wire(up_wire_omgidl::OmgIdlWire)),
    };
    Ok(transport)
}
