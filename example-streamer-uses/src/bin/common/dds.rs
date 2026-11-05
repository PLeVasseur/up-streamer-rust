// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Shared DDS endpoint construction and role flow from explicit inputs.

use std::sync::Arc;
use std::time::Duration;
use up_rust::UWithNativePrefixWire;
use up_transport_dds::{DdsConfig, Reliability};

use async_trait::async_trait;
use clap::ValueEnum;
use tokio::sync::mpsc;
use up_rust::frame::metadata::try_project_umessage_to_frame_metadata;
use up_rust::{
    PayloadCodecIdentity, PayloadEncoding, UCode, UFrameMetadata, UListener, UMessage,
    UMessageBuilder, UOwnedFrame, UOwnedListener, UOwnedTransport, UStatus, UTransport, UUri,
};
use up_transport_dds::encoded_owned::DdsEncodedOwnedCore;
use up_transport_dds::owned::UPTransportDdsOwned;
use up_transport_dds::UPTransportDds;

#[cfg(feature = "sustained-workload")]
use super::workload;
use super::{payloads, proof, startup};
pub use payloads::NativeContext;
use payloads::{
    arrow_payload_bytes, native_payload_bytes, omgidl_payload_bytes, xcdrv2_payload_bytes,
};

const ENTITY_ID: u32 = 0x0001_5BA0;
const ENTITY_VERSION: u8 = 1;
const NATIVE_PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"DDSR");

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub enum Role {
    Publisher,
    Subscriber,
    Notifier,
    Notifyee,
    Client,
    Server,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RouteFamily {
    Classic,
    OwnedFrame,
    EncodedOwned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Encoding {
    Native,
    Protobuf,
    Xcdrv2,
    Arrow,
    #[value(name = "omgidl")]
    OmgIdl,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ReliabilityArg {
    Reliable,
    BestEffort,
}

fn timeout(args: &DdsArgs) -> Duration {
    Duration::from_millis(args.timeout_ms)
}

fn encoding(args: &DdsArgs) -> Result<PayloadEncoding, UStatus> {
    Ok(match args.encoding {
        Encoding::Native => args.native.encoding()?,
        Encoding::Protobuf => up_rust::ProtobufWire::encoding(),
        Encoding::Xcdrv2 => up_wire_xcdrv2::XcdrV2Wire::encoding(),
        Encoding::Arrow => up_wire_arrow::ArrowWire::encoding(),
        Encoding::OmgIdl => up_wire_omgidl::OmgIdlWire::encoding(),
    })
}

fn topic(authority: &str, resource_id: u16) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, ENTITY_ID, ENTITY_VERSION, resource_id)
        .map_err(|error| invalid(format!("invalid topic URI: {error}")))
}

fn endpoint(authority: &str) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, ENTITY_ID, ENTITY_VERSION, 0)
        .map_err(|error| invalid(format!("invalid endpoint URI: {error}")))
}

fn method(authority: &str, resource_id: u16) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, ENTITY_ID, ENTITY_VERSION, resource_id)
        .map_err(|error| invalid(format!("invalid method URI: {error}")))
}

fn outbound_message(role: Role, args: &DdsArgs) -> Result<UMessage, UStatus> {
    outbound_message_sequence(role, args, 1)
}

fn outbound_message_sequence(
    role: Role,
    args: &DdsArgs,
    sequence: u32,
) -> Result<UMessage, UStatus> {
    let payload = payload_sequence(args, sequence)?;
    let result = match role {
        Role::Publisher => {
            UMessageBuilder::publish(topic(&args.local_authority, args.topic_resource_id)?)
                .build_with_payload(payload, encoding(args)?)
        }
        Role::Notifier => UMessageBuilder::notification(
            topic(&args.local_authority, args.topic_resource_id)?,
            endpoint(&args.peer_authority)?,
        )
        .build_with_payload(payload, encoding(args)?),
        Role::Client => UMessageBuilder::request(
            method(&args.peer_authority, args.method_resource_id)?,
            endpoint(&args.local_authority)?,
            u32::try_from(args.timeout_ms).unwrap_or(u32::MAX),
        )
        .build_with_payload(payload, encoding(args)?),
        Role::Subscriber | Role::Notifyee | Role::Server => {
            return Err(invalid("passive role has no outbound request"));
        }
    };
    result.map_err(|error| invalid(format!("build outbound message: {error}")))
}

fn payload_sequence(args: &DdsArgs, sequence: u32) -> Result<Vec<u8>, UStatus> {
    #[cfg(feature = "sustained-workload")]
    if args.workload_plan.is_some() && args.encoding == Encoding::Protobuf {
        use protobuf::Message;
        let mut payload = hello_world_protos::hello_world_service::HelloRequest::new();
        payload.name = format!("{}:{sequence}", args.payload);
        return payload
            .write_to_bytes()
            .map_err(|error| invalid(error.to_string()));
    }
    match args.encoding {
        Encoding::Native => native_payload_bytes(NATIVE_PAYLOAD_MAGIC, sequence, &args.payload),
        Encoding::Protobuf => Ok(args.payload.as_bytes().to_vec()),
        Encoding::Xcdrv2 => {
            xcdrv2_payload_bytes(sequence, args.local_authority.clone(), &args.payload)
        }
        Encoding::Arrow => arrow_payload_bytes(sequence, &args.payload),
        Encoding::OmgIdl => omgidl_payload_bytes(sequence, &args.payload),
    }
}

fn filters(role: Role, args: &DdsArgs) -> Result<(UUri, Option<UUri>), UStatus> {
    match role {
        Role::Subscriber => Ok((topic(&args.peer_authority, args.topic_resource_id)?, None)),
        Role::Notifyee => Ok((
            topic(&args.peer_authority, args.topic_resource_id)?,
            Some(endpoint(&args.local_authority)?),
        )),
        Role::Client => Ok((
            method(&args.peer_authority, args.method_resource_id)?,
            Some(endpoint(&args.local_authority)?),
        )),
        Role::Server => Ok((
            endpoint(&args.peer_authority)?,
            Some(method(&args.local_authority, args.method_resource_id)?),
        )),
        Role::Publisher | Role::Notifier => Err(invalid("active one-way role has no filter")),
    }
}

fn response_metadata(request: &UFrameMetadata, args: &DdsArgs) -> Result<UFrameMetadata, UStatus> {
    let source = request
        .sink()
        .cloned()
        .ok_or_else(|| invalid("request metadata has no sink"))?;
    UFrameMetadata::response(source, request.source().clone(), request.id().clone())
        .with_payload_encoding(encoding(args)?)
        .build()
        .map_err(|error| invalid(format!("build response metadata: {error}")))
        .and_then(|metadata| {
            if args.encoding == Encoding::Native {
                args.native.stamp(metadata)
            } else {
                Ok(metadata)
            }
        })
}

fn response_message(request: &UMessage, args: &DdsArgs) -> Result<UMessage, UStatus> {
    let request_id = request.id().clone();
    let invoked_method = request
        .sink()
        .cloned()
        .ok_or_else(|| invalid("request has no sink"))?;
    UMessageBuilder::response(request.source().clone(), request_id, invoked_method)
        .build_with_payload(
            request
                .payload()
                .map_or_else(Vec::new, |bytes| bytes.to_vec()),
            encoding(args)?,
        )
        .map_err(|error| invalid(format!("build response message: {error}")))
}

struct MessageListener(mpsc::UnboundedSender<UMessage>);

#[async_trait]
impl UListener for MessageListener {
    async fn on_receive(&self, message: UMessage) {
        proof::received(&message);
        let _ = self.0.send(message);
    }
}

pub async fn run_classic(
    role: Role,
    args: &DdsArgs,
    transport: Arc<UPTransportDds>,
) -> Result<(), UStatus> {
    #[cfg(feature = "sustained-workload")]
    if let Some(path) = &args.workload_plan {
        let plan = workload::WorkerPlan::load(path)?;
        let source = matches!(role, Role::Publisher | Role::Client);
        let expected_role = if matches!(role, Role::Client | Role::Server) {
            streamer_soak::Role::ClientServerRpc
        } else {
            streamer_soak::Role::PublisherSubscriber
        };
        if plan.replay.role != expected_role {
            return Err(invalid("workload role differs from binary role"));
        }
        transport.wait_ready(2, timeout(args))?;
        let filter = if matches!(role, Role::Publisher) {
            None
        } else {
            Some(filters(role, args)?)
        };
        let health = transport.health();
        let final_transport = transport.clone();
        return workload::classic(transport, args.native.clone(), plan, source, filter,
            |sequence| outbound_message_sequence(role, args, sequence),
            move || {
                let acknowledged = final_transport.wait_acknowledged(timeout(args));
                drop(final_transport);
                let health = health.snapshot();
                let counters = serde_json::json!({"dds_errors": health.dds_errors,
                    "malformed_samples": health.malformed_samples, "dispatch_drops": health.dispatch_drops,
                    "callback_panics": health.callback_panics, "worker_panics": health.worker_panics,
                    "teardown_errors": health.teardown_errors});
                streamer_flow_evidence::emit_json_or_fail("OP_TRANSPORT_HEALTH ", &counters);
                acknowledged?;
                if counters.as_object().expect("health object").values().any(|value| value.as_u64() != Some(0)) {
                    return Err(invalid("binding health reported a sustained-workload error"));
                }
                Ok(())
            }).await;
    }
    if matches!(role, Role::Publisher | Role::Notifier) {
        transport.wait_ready(2, timeout(args))?;
        startup::wait_for_start().await?;
        let message = outbound_message(role, args)?;
        proof::send(transport.as_ref(), message.clone()).await?;
        transport.wait_acknowledged(timeout(args))?;
        print_sent(role, message.payload().map_or(0, |payload| payload.len()));
        return Ok(());
    }

    let (source, sink) = filters(role, args)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport
        .register_listener(&source, sink.as_ref(), Arc::new(MessageListener(tx)))
        .await?;
    println!("READY listener_registered");
    if matches!(role, Role::Client) {
        transport.wait_ready(2, timeout(args))?;
        startup::wait_for_start().await?;
        proof::send(transport.as_ref(), outbound_message(role, args)?).await?;
    }
    let message = receive(&mut rx, timeout(args), "classic message").await?;
    if args.encoding == Encoding::Native {
        args.native.verify_classic(&message)?;
    }
    if matches!(role, Role::Server) {
        transport.wait_ready(2, timeout(args))?;
        proof::send(transport.as_ref(), response_message(&message, args)?).await?;
    }
    transport.wait_acknowledged(timeout(args))?;
    print_observed(role, message.payload().map_or(0, |payload| payload.len()));
    Ok(())
}

struct OwnedListener(mpsc::UnboundedSender<UOwnedFrame>);

#[async_trait]
impl UOwnedListener for OwnedListener {
    async fn on_receive_owned(&self, frame: UOwnedFrame) {
        proof::received_frame(&frame);
        let _ = self.0.send(frame);
    }
}

pub async fn run_owned(
    role: Role,
    args: &DdsArgs,
    transport: Arc<UPTransportDdsOwned>,
) -> Result<(), UStatus> {
    if matches!(role, Role::Publisher | Role::Notifier) {
        transport.wait_ready(2, timeout(args))?;
        startup::wait_for_start().await?;
        let frame = frame_from_message(&outbound_message(role, args)?, args)?;
        let len = frame.payload_bytes().len();
        proof::send_owned(transport.as_ref(), frame).await?;
        transport.wait_acknowledged(timeout(args))?;
        print_sent(role, len);
        return Ok(());
    }

    let (source, sink) = filters(role, args)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport
        .register_owned_listener(&source, sink.as_ref(), Arc::new(OwnedListener(tx)))
        .await?;
    println!("READY listener_registered");
    if matches!(role, Role::Client) {
        transport.wait_ready(2, timeout(args))?;
        startup::wait_for_start().await?;
        proof::send_owned(
            transport.as_ref(),
            frame_from_message(&outbound_message(role, args)?, args)?,
        )
        .await?;
    }
    let frame = receive(&mut rx, timeout(args), "owned frame").await?;
    if args.encoding == Encoding::Native {
        args.native
            .verify_owned(frame.metadata(), frame.payload_bytes())?;
    }
    if matches!(role, Role::Server) {
        transport.wait_ready(2, timeout(args))?;
        proof::send_owned(
            transport.as_ref(),
            UOwnedFrame::with_payload(
                response_metadata(frame.metadata(), args)?,
                frame.payload_bytes().to_vec(),
            )
            .map_err(|error| invalid(format!("build owned response: {error}")))?,
        )
        .await?;
    }
    transport.wait_acknowledged(timeout(args))?;
    print_observed(role, frame.payload_bytes().len());
    Ok(())
}

fn frame_from_message(message: &UMessage, args: &DdsArgs) -> Result<UOwnedFrame, UStatus> {
    let metadata = if args.encoding == Encoding::Native {
        args.native.project_classic(message)?
    } else {
        try_project_umessage_to_frame_metadata(message)
            .map_err(|error| invalid(format!("project frame metadata: {error}")))?
    };
    match message.payload() {
        Some(payload) => UOwnedFrame::with_payload(metadata, payload.to_vec()),
        None => UOwnedFrame::without_payload(metadata),
    }
    .map_err(|error| invalid(format!("build owned frame: {error}")))
}

pub async fn run_encoded_owned<W>(
    role: Role,
    args: &DdsArgs,
    core: DdsEncodedOwnedCore,
    configure: impl FnOnce(
        DdsEncodedOwnedCore,
    ) -> up_rust::UNativePrefixWireTransport<DdsEncodedOwnedCore, W>,
) -> Result<(), UStatus>
where
    W: up_rust::UWire + Copy + Send + Sync + 'static,
    up_rust::UFrameFieldsMetadataCodec: up_rust::UWireMetadataCodecFor<W>,
{
    let required_matches = usize::from(matches!(
        role,
        Role::Publisher | Role::Notifier | Role::Client
    )) + 1;
    core.wait_ready(required_matches, timeout(args))?;
    let transport = Arc::new(configure(core));
    if matches!(role, Role::Publisher | Role::Notifier) {
        startup::wait_for_start().await?;
        let frame = frame_from_message(&outbound_message(role, args)?, args)?;
        let payload_len = frame.payload_bytes().len();
        proof::send_owned(transport.as_ref(), frame).await?;
        transport.core().wait_acknowledged(timeout(args))?;
        print_sent(role, payload_len);
        return Ok(());
    }

    let (source, sink) = filters(role, args)?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    transport
        .register_owned_listener(&source, sink.as_ref(), Arc::new(OwnedListener(tx)))
        .await?;
    println!("READY listener_registered");
    if matches!(role, Role::Client) {
        startup::wait_for_start().await?;
        proof::send_owned(
            transport.as_ref(),
            frame_from_message(&outbound_message(role, args)?, args)?,
        )
        .await?;
    }
    let frame = receive(&mut rx, timeout(args), "encoded-owned frame").await?;
    if args.encoding == Encoding::Native {
        // DDS exposes an owned receive view, not a native borrowing capability.
        // Validate the full representation before copying its bytes for the echo.
        args.native.verify_view(&frame)?;
    }
    let payload = payloads::copy_payload_bytes(&frame)?;
    if matches!(role, Role::Server) {
        let response =
            UOwnedFrame::with_payload(response_metadata(frame.metadata(), args)?, payload.clone())
                .map_err(|error| invalid(format!("build encoded-owned response: {error}")))?;
        proof::send_owned(transport.as_ref(), response).await?;
    }
    transport.core().wait_acknowledged(timeout(args))?;
    print_observed(role, payload.len());
    Ok(())
}

async fn receive<T>(
    receiver: &mut mpsc::UnboundedReceiver<T>,
    duration: Duration,
    what: &str,
) -> Result<T, UStatus> {
    // Passive roles: the window opens at the controller's source release.
    startup::receive_window().await?;
    tokio::time::timeout(duration, receiver.recv())
        .await
        .map_err(|_| {
            UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                format!("timed out waiting for {what}"),
            )
        })?
        .ok_or_else(|| {
            UStatus::fail_with_code(UCode::Unavailable, format!("{what} channel closed"))
        })
}

fn print_sent(role: Role, payload_len: usize) {
    println!(
        "FLOW sent_payload_bytes={payload_len} role={}",
        role_name(role)
    );
}

fn print_observed(role: Role, payload_len: usize) {
    println!(
        "FLOW observed_payload_bytes={payload_len} role={}",
        role_name(role)
    );
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Publisher => "publisher",
        Role::Subscriber => "subscriber",
        Role::Notifier => "notifier",
        Role::Notifyee => "notifyee",
        Role::Client => "rpc_client",
        Role::Server => "rpc_server",
    }
}

pub fn invalid(detail: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, detail.into())
}

/// Explicit DDS endpoint and role inputs, independent of any CLI parser.
pub struct DdsArgs {
    pub native: NativeContext,
    pub domain_id: i32,
    pub origin_id: Option<String>,
    pub reliability: ReliabilityArg,
    pub history_depth: u32,
    pub route_family: RouteFamily,
    pub encoding: Encoding,
    pub local_authority: String,
    pub peer_authority: String,
    pub topic_resource_id: u16,
    pub method_resource_id: u16,
    pub payload: String,
    pub timeout_ms: u64,
    #[cfg(feature = "sustained-workload")]
    pub workload_plan: Option<std::path::PathBuf>,
}

/// Construct the configured DDS family and run the selected role.
pub async fn run(role: Role, args: &DdsArgs) -> Result<(), UStatus> {
    let mut config = DdsConfig::new(args.domain_id);
    if let Some(origin_id) = &args.origin_id {
        config.origin_id.clone_from(origin_id);
    }
    config.qos.reliability = match args.reliability {
        ReliabilityArg::Reliable => Reliability::Reliable,
        ReliabilityArg::BestEffort => Reliability::BestEffort,
    };
    config.qos.history_depth = args.history_depth;
    let runtime = tokio::runtime::Handle::current();
    match args.route_family {
        RouteFamily::Classic => {
            let transport = Arc::new(UPTransportDds::with_config(config, runtime)?);
            run_classic(role, args, transport).await
        }
        RouteFamily::OwnedFrame => {
            let transport = Arc::new(UPTransportDdsOwned::with_config(config, runtime)?);
            run_owned(role, args, transport).await
        }
        RouteFamily::EncodedOwned => {
            let core = DdsEncodedOwnedCore::with_config(config, runtime)?;
            match args.encoding {
                Encoding::Native => {
                    let profile = args.native.agreement()?.clone();
                    run_encoded_owned(role, args, core, |core| {
                        core.into_stable_container_transport(profile)
                    })
                    .await
                }
                Encoding::Protobuf => {
                    run_encoded_owned(role, args, core, |core| core.into_protobuf_transport()).await
                }
                Encoding::Xcdrv2 => {
                    run_encoded_owned(role, args, core, |core| {
                        core.into_native_prefix_wire_transport(up_wire_xcdrv2::XcdrV2Wire)
                    })
                    .await
                }
                Encoding::Arrow => {
                    run_encoded_owned(role, args, core, |core| {
                        core.into_native_prefix_wire_transport(up_wire_arrow::ArrowWire)
                    })
                    .await
                }
                Encoding::OmgIdl => {
                    run_encoded_owned(role, args, core, |core| {
                        core.into_native_prefix_wire_transport(up_wire_omgidl::OmgIdlWire)
                    })
                    .await
                }
            }
        }
    }
}
