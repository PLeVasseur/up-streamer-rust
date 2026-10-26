// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Shared role behavior over configured transports. Entrypoints own CLI, binding
//! construction and concrete wire selection; passive readiness, payload identity,
//! RPC correlation and evidence stay on the same flow for owned and native APIs.
#![allow(dead_code)]

use std::{sync::Arc, time::Duration};

use clap::ValueEnum;
use up_rust::{
    PayloadCodecIdentity, PayloadEncoding, UCode, UFrameMetadata, UFrameView, UOwnedFrame,
    UOwnedTransport, UStatus, UTxBuffer, UTxLoanSpec, UUri, UZeroCopyRxLease, UZeroCopyTransport,
};
#[cfg(feature = "sustained-workload")]
use up_rust::{UMessage, UMessageBuilder};

use super::payloads::{
    arrow_payload_bytes, native_payload_alignment, native_payload_bytes, omgidl_payload_bytes,
    xcdrv2_payload_bytes, NativeContext,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum FlowMode {
    CopyMinimized,
    OwnedFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum FlowTransport {
    Zenoh,
    Iceoryx2,
    Lola,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum FlowWireFormat {
    Native,
    Protobuf,
    Xcdrv2,
    Arrow,
    Omgidl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum FlowRole {
    PubSender,
    PubReceiver,
    NotifySender,
    NotifyReceiver,
    RpcClient,
    RpcServer,
}

/// Repetition policy for finite connectivity sends of one prepared message.
#[derive(Clone, Copy)]
pub(crate) struct SendOptions {
    pub count: usize,
    pub interval: Option<Duration>,
}

impl From<&FlowConfig> for SendOptions {
    fn from(config: &FlowConfig) -> Self {
        Self {
            count: config.send_count,
            interval: Some(Duration::from_millis(config.send_interval_ms)),
        }
    }
}

#[derive(Clone)]
pub(crate) struct FlowConfig {
    pub native: NativeContext,
    pub wire_format: FlowWireFormat,
    pub role: FlowRole,
    pub local_authority: String,
    pub peer_authority: String,
    pub ue_id: u32,
    pub ue_version_major: u8,
    pub topic_resource_id: u16,
    pub method_resource_id: u16,
    pub payload: String,
    pub payload_alignment: usize,
    pub send_count: usize,
    pub send_interval_ms: u64,
    pub timeout_ms: u64,
    pub rpc_response_delay_ms: u64,
    #[cfg(feature = "sustained-workload")]
    pub workload_plan: Option<std::path::PathBuf>,
}

impl FlowConfig {
    pub(crate) fn local_endpoint(&self) -> Result<UUri, UStatus> {
        UUri::try_from_parts(&self.local_authority, self.ue_id, self.ue_version_major, 0)
            .map_err(|error| invalid(error.to_string()))
    }

    // The existing examples use one fixture entity independently of the backend's
    // local registration URI. Keep that distinction when sharing their flow.
    pub(crate) fn uri(&self, authority: &str, resource: u16) -> Result<UUri, UStatus> {
        UUri::try_from_parts(authority, 0x5BA0, 1, resource)
            .map_err(|error| invalid(error.to_string()))
    }

    fn filters(&self) -> Result<(UUri, Option<UUri>), UStatus> {
        Ok(match self.role {
            FlowRole::PubReceiver => (
                self.uri(&self.peer_authority, self.topic_resource_id)?,
                None,
            ),
            FlowRole::NotifyReceiver => (
                self.uri(&self.peer_authority, self.topic_resource_id)?,
                Some(self.uri(&self.local_authority, 0)?),
            ),
            FlowRole::RpcClient => (
                self.uri(&self.peer_authority, self.method_resource_id)?,
                Some(self.uri(&self.local_authority, 0)?),
            ),
            FlowRole::RpcServer => (
                UUri::try_from_parts(&self.peer_authority, u32::MAX, u8::MAX, u16::MAX)
                    .map_err(|error| invalid(error.to_string()))?,
                Some(self.uri(&self.local_authority, self.method_resource_id)?),
            ),
            _ => return Err(invalid("sender-only role has no receive filter")),
        })
    }

    pub(crate) fn encoding(&self) -> Result<PayloadEncoding, UStatus> {
        Ok(match self.wire_format {
            FlowWireFormat::Native => self.native.encoding()?,
            FlowWireFormat::Protobuf => up_rust::ProtobufWire::encoding(),
            FlowWireFormat::Xcdrv2 => up_wire_xcdrv2::XcdrV2Wire::encoding(),
            FlowWireFormat::Arrow => up_wire_arrow::ArrowWire::encoding(),
            FlowWireFormat::Omgidl => up_wire_omgidl::OmgIdlWire::encoding(),
        })
    }

    pub(crate) fn payload(&self, sequence: u32, workload: bool) -> Result<Vec<u8>, UStatus> {
        #[cfg(feature = "payload-contract-large-fixtures")]
        if let Some(fixture) = self.native.fixture() {
            if !workload || self.wire_format != FlowWireFormat::Native {
                return Err(invalid("sensor fixture requires a native workload"));
            }
            return fixture.bytes(sequence, self.native.agreement()?);
        }
        match self.wire_format {
            FlowWireFormat::Native => {
                native_payload_bytes(u32::from_le_bytes(*b"UPNF"), sequence, &self.payload)
            }
            FlowWireFormat::Xcdrv2 => {
                xcdrv2_payload_bytes(sequence, self.local_authority.clone(), &self.payload)
            }
            FlowWireFormat::Arrow => arrow_payload_bytes(sequence, &self.payload),
            FlowWireFormat::Omgidl => omgidl_payload_bytes(sequence, &self.payload),
            FlowWireFormat::Protobuf if workload => {
                use protobuf::Message;
                let mut value = hello_world_protos::hello_world_service::HelloRequest::new();
                value.name = format!("{}:{sequence}", self.payload);
                value
                    .write_to_bytes()
                    .map_err(|error| invalid(error.to_string()))
            }
            FlowWireFormat::Protobuf => Ok(self.payload.as_bytes().to_vec()),
        }
    }

    fn stamp(&self, metadata: UFrameMetadata) -> Result<UFrameMetadata, UStatus> {
        if self.wire_format == FlowWireFormat::Native {
            self.native.stamp(metadata)
        } else {
            Ok(metadata)
        }
    }

    fn outbound_metadata(&self) -> Result<UFrameMetadata, UStatus> {
        let builder = match self.role {
            FlowRole::PubSender => {
                UFrameMetadata::publish(self.uri(&self.local_authority, self.topic_resource_id)?)
            }
            FlowRole::NotifySender => UFrameMetadata::notification(
                self.uri(&self.local_authority, self.topic_resource_id)?,
                self.uri(&self.peer_authority, 0)?,
            ),
            FlowRole::RpcClient => UFrameMetadata::request(
                self.uri(&self.peer_authority, self.method_resource_id)?,
                self.uri(&self.local_authority, 0)?,
                Duration::from_millis(self.timeout_ms.min(u64::from(u32::MAX))),
            ),
            _ => return Err(invalid("passive role has no outbound request")),
        };
        self.stamp(
            builder
                .with_payload_encoding(self.encoding()?)
                .build()
                .map_err(|error| invalid(error.to_string()))?,
        )
    }

    fn response_metadata(&self, request: &UFrameMetadata) -> Result<UFrameMetadata, UStatus> {
        let source = request
            .sink()
            .cloned()
            .ok_or_else(|| invalid("request metadata missing sink"))?;
        let mut builder =
            UFrameMetadata::response(source, request.source().clone(), request.id().clone())
                .with_payload_encoding(self.encoding()?);
        if let Some(priority) = request.priority() {
            builder = builder.with_priority(priority);
        }
        if let Some(ttl) = request.ttl() {
            builder = builder.with_ttl(ttl);
        }
        self.stamp(
            builder
                .build()
                .map_err(|error| invalid(error.to_string()))?,
        )
    }

    fn alignment(&self) -> usize {
        if self.wire_format == FlowWireFormat::Native {
            native_payload_alignment()
        } else {
            self.payload_alignment
        }
    }

    #[cfg(feature = "sustained-workload")]
    fn workload_message(&self, sequence: u32) -> Result<UMessage, UStatus> {
        let payload = self.payload(sequence, true)?;
        let result = if self.role == FlowRole::PubSender {
            UMessageBuilder::publish(self.uri(&self.local_authority, self.topic_resource_id)?)
                .build_with_payload(payload, self.encoding()?)
        } else {
            UMessageBuilder::request(
                self.uri(&self.peer_authority, self.method_resource_id)?,
                self.uri(&self.local_authority, 0)?,
                60_000,
            )
            .build_with_payload(payload, self.encoding()?)
        };
        result.map_err(|error| invalid(error.to_string()))
    }
}

pub(crate) async fn run_owned(
    transport: Arc<dyn UOwnedTransport>,
    config: &FlowConfig,
) -> Result<(), UStatus> {
    run_owned_with_ready(transport, config, super::receive::listener_ready).await
}

pub(crate) async fn run_owned_with_ready(
    transport: Arc<dyn UOwnedTransport>,
    config: &FlowConfig,
    ready: fn(),
) -> Result<(), UStatus> {
    match config.role {
        FlowRole::PubSender | FlowRole::NotifySender => {
            let payload = config.payload(1, false)?;
            send_owned_repeated(
                &transport,
                config.outbound_metadata()?,
                &payload,
                config.into(),
            )
            .await?;
            observed_send(config.role, payload.len());
        }
        FlowRole::PubReceiver | FlowRole::NotifyReceiver => {
            let (source, sink) = config.filters()?;
            let frame = super::receive::owned_with_ready(
                &transport,
                &source,
                sink.as_ref(),
                config.timeout_ms,
                (config.wire_format == FlowWireFormat::Native).then_some(&config.native),
                ready,
            )
            .await?;
            observed_receive(config.role, frame.payload_bytes().len());
        }
        FlowRole::RpcClient => {
            let payload = config.payload(1, false)?;
            let metadata = config.outbound_metadata()?;
            let (source, sink) = config.filters()?;
            let response = if config.send_count <= 1 {
                send_owned(&transport, metadata, &payload).await?;
                receive_owned_pull(
                    &transport,
                    &source,
                    sink.as_ref(),
                    config.timeout_ms,
                    (config.wire_format == FlowWireFormat::Native).then_some(&config.native),
                )
                .await?
            } else {
                let sender = Arc::clone(&transport);
                let bytes = payload.clone();
                let settings = config.clone();
                let task = tokio::spawn(async move {
                    send_owned_repeated(&sender, metadata, &bytes, (&settings).into()).await
                });
                let response = receive_owned_pull(
                    &transport,
                    &source,
                    sink.as_ref(),
                    config.timeout_ms,
                    (config.wire_format == FlowWireFormat::Native).then_some(&config.native),
                )
                .await;
                task.abort();
                let _ = task.await;
                response?
            };
            ensure_payload_matches(&payload, response.payload_bytes())?;
            observed_receive(config.role, response.payload_bytes().len());
        }
        FlowRole::RpcServer => {
            let (source, sink) = config.filters()?;
            let request = super::receive::owned_with_ready(
                &transport,
                &source,
                sink.as_ref(),
                config.timeout_ms,
                (config.wire_format == FlowWireFormat::Native).then_some(&config.native),
                ready,
            )
            .await?;
            let payload = request.payload_bytes().to_vec();
            let metadata = config.response_metadata(request.metadata())?;
            tokio::time::sleep(Duration::from_millis(config.rpc_response_delay_ms)).await;
            send_owned_repeated(&transport, metadata, &payload, config.into()).await?;
            observed_receive(config.role, payload.len());
        }
    }
    Ok(())
}

pub(crate) async fn run_native<T>(transport: Arc<T>, config: &FlowConfig) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + up_rust::UHasWire + Send + Sync + 'static,
    T::Tx: UTxBuffer,
    T::Rx: UZeroCopyRxLease + Send + 'static,
{
    run_native_with_ready(transport, config, super::receive::listener_ready).await
}

pub(crate) async fn run_native_with_ready<T>(
    transport: Arc<T>,
    config: &FlowConfig,
    ready: fn(),
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + up_rust::UHasWire + Send + Sync + 'static,
    T::Tx: UTxBuffer,
    T::Rx: UZeroCopyRxLease + Send + 'static,
{
    #[cfg(feature = "sustained-workload")]
    if let Some(path) = &config.workload_plan {
        let plan = super::workload::WorkerPlan::load(path)?;
        let source = matches!(config.role, FlowRole::PubSender | FlowRole::RpcClient);
        let expected = if matches!(config.role, FlowRole::RpcClient | FlowRole::RpcServer) {
            streamer_soak::Role::ClientServerRpc
        } else {
            streamer_soak::Role::PublisherSubscriber
        };
        if plan.replay.role != expected {
            return Err(invalid("workload role differs from configured role"));
        }
        if matches!(
            config.role,
            FlowRole::NotifySender | FlowRole::NotifyReceiver
        ) {
            return Err(invalid("unsupported sustained role"));
        }
        let filter = if config.role == FlowRole::PubSender {
            None
        } else {
            Some(config.filters()?)
        };
        return super::workload::native(
            transport,
            config.native.clone(),
            plan,
            source,
            filter,
            |sequence| config.workload_message(sequence),
        )
        .await;
    }
    match config.role {
        FlowRole::PubSender | FlowRole::NotifySender => {
            let payload = config.payload(1, false)?;
            send_native_repeated(
                &transport,
                config.outbound_metadata()?,
                &payload,
                config.alignment(),
                config.into(),
            )
            .await?;
            observed_send(config.role, payload.len());
        }
        FlowRole::PubReceiver | FlowRole::NotifyReceiver => {
            let (source, sink) = config.filters()?;
            let frame = super::receive::zero_copy_with_ready(
                &transport,
                &source,
                sink.as_ref(),
                config.timeout_ms,
                ready,
            )
            .await?;
            let payload = super::payloads::copy_payload_bytes(&frame)?;
            observed_receive(config.role, payload.len());
        }
        FlowRole::RpcClient => {
            let payload = config.payload(1, false)?;
            let metadata = config.outbound_metadata()?;
            let (source, sink) = config.filters()?;
            let response = if config.send_count <= 1 {
                send_native(&transport, metadata, &payload, config.alignment()).await?;
                receive_native_pull(&transport, &source, sink.as_ref(), config.timeout_ms).await?
            } else {
                let sender = Arc::clone(&transport);
                let bytes = payload.clone();
                let settings = config.clone();
                let task = tokio::spawn(async move {
                    send_native_repeated(
                        &sender,
                        metadata,
                        &bytes,
                        settings.alignment(),
                        (&settings).into(),
                    )
                    .await
                });
                let response =
                    receive_native_pull(&transport, &source, sink.as_ref(), config.timeout_ms)
                        .await;
                task.abort();
                let _ = task.await;
                response?
            };
            let bytes = super::payloads::copy_payload_bytes(&response)?;
            ensure_payload_matches(&payload, &bytes)?;
            observed_receive(config.role, bytes.len());
        }
        FlowRole::RpcServer => {
            let (source, sink) = config.filters()?;
            let request = super::receive::zero_copy_with_ready(
                &transport,
                &source,
                sink.as_ref(),
                config.timeout_ms,
                ready,
            )
            .await?;
            let payload = super::payloads::copy_payload_bytes(&request)?;
            let metadata = config.response_metadata(request.metadata())?;
            tokio::time::sleep(Duration::from_millis(config.rpc_response_delay_ms)).await;
            send_native_repeated(
                &transport,
                metadata,
                &payload,
                config.alignment(),
                config.into(),
            )
            .await?;
            observed_receive(config.role, payload.len());
        }
    }
    Ok(())
}

async fn send_owned(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
) -> Result<(), UStatus> {
    super::proof::send_owned(
        transport.as_ref(),
        UOwnedFrame::with_payload(metadata, payload.to_vec())
            .map_err(|error| invalid(error.to_string()))?,
    )
    .await
}

pub(crate) async fn send_owned_repeated(
    transport: &Arc<dyn UOwnedTransport>,
    metadata: UFrameMetadata,
    payload: &[u8],
    options: SendOptions,
) -> Result<(), UStatus> {
    for attempt in 0..options.count.max(1) {
        send_owned(transport, metadata.clone(), payload).await?;
        if let Some(interval) = options
            .interval
            .filter(|_| attempt + 1 < options.count.max(1))
        {
            tokio::time::sleep(interval).await;
        }
    }
    Ok(())
}

async fn send_native<T>(
    transport: &Arc<T>,
    metadata: UFrameMetadata,
    payload: &[u8],
    alignment: usize,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
{
    let mut loan = transport
        .loan_validated_tx(UTxLoanSpec::payload(metadata, payload.len(), alignment)?)
        .await?;
    loan.payload_mut().copy_from_slice(payload);
    super::proof::send_loan(transport.as_ref(), loan).await
}

pub(crate) async fn send_native_repeated<T>(
    transport: &Arc<T>,
    metadata: UFrameMetadata,
    payload: &[u8],
    alignment: usize,
    options: SendOptions,
) -> Result<(), UStatus>
where
    T: UZeroCopyTransport + Send + Sync + 'static,
    T::Tx: UTxBuffer,
{
    for attempt in 0..options.count.max(1) {
        send_native(transport, metadata.clone(), payload, alignment).await?;
        if let Some(interval) = options
            .interval
            .filter(|_| attempt + 1 < options.count.max(1))
        {
            tokio::time::sleep(interval).await;
        }
    }
    Ok(())
}

pub(crate) async fn receive_owned_pull(
    transport: &Arc<dyn UOwnedTransport>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout_ms: u64,
    native: Option<&NativeContext>,
) -> Result<UOwnedFrame, UStatus> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for owned frame",
            ));
        }
        let result = tokio::time::timeout_at(deadline, transport.receive_owned(source, sink))
            .await
            .map_err(|_| {
                UStatus::fail_with_code(
                    UCode::DeadlineExceeded,
                    "timed out waiting for owned frame",
                )
            })?;
        match result {
            Ok(frame) => {
                super::proof::received_frame(&frame);
                if let Some(native) = native {
                    native.verify_owned(frame.metadata(), frame.payload_bytes())?;
                }
                return Ok(frame);
            }
            Err(error) if error.code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) async fn receive_native_pull<T>(
    transport: &Arc<T>,
    source: &UUri,
    sink: Option<&UUri>,
    timeout: u64,
) -> Result<T::Rx, UStatus>
where
    T: UZeroCopyTransport + up_rust::UHasWire + Send + Sync + 'static,
    T::Rx: UZeroCopyRxLease,
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for zero-copy frame",
            ));
        }
        let result = tokio::time::timeout_at(
            deadline,
            transport.receive_validated_zero_copy(source, sink),
        )
        .await
        .map_err(|_| {
            UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "timed out waiting for zero-copy frame",
            )
        })?;
        match result {
            Ok(frame) => {
                super::proof::received_frame(&frame);
                super::payloads::verify_received_frame(transport.as_ref(), &frame)?;
                return Ok(frame);
            }
            Err(error) if error.code() == UCode::NotFound => {
                tokio::time::sleep(Duration::from_millis(10)).await
            }
            Err(error) => return Err(error),
        }
    }
}

fn ensure_payload_matches(expected: &[u8], actual: &[u8]) -> Result<(), UStatus> {
    if expected == actual {
        Ok(())
    } else {
        Err(UStatus::fail_with_code(
            UCode::Internal,
            format!(
                "response payload mismatch: expected {} bytes, observed {} bytes",
                expected.len(),
                actual.len()
            ),
        ))
    }
}

fn role_name(role: FlowRole) -> &'static str {
    match role {
        FlowRole::PubSender => "pub_sender",
        FlowRole::PubReceiver => "pub_receiver",
        FlowRole::NotifySender => "notify_sender",
        FlowRole::NotifyReceiver => "notify_receiver",
        FlowRole::RpcClient => "rpc_client",
        FlowRole::RpcServer => "rpc_server",
    }
}

fn observed_send(role: FlowRole, size: usize) {
    println!("FLOW sent_payload_bytes={size} role={}", role_name(role));
}
fn observed_receive(role: FlowRole, size: usize) {
    println!(
        "FLOW observed_payload_bytes={size} role={}",
        role_name(role)
    );
}
pub(crate) fn invalid(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };
    use up_rust::{UOwnedListener, UOwnedTransportImpl, UPriority};

    fn config() -> FlowConfig {
        FlowConfig {
            native: NativeContext::default(),
            wire_format: FlowWireFormat::Protobuf,
            role: FlowRole::PubSender,
            local_authority: "source".into(),
            peer_authority: "sink".into(),
            ue_id: 0x2_4210,
            ue_version_major: 3,
            topic_resource_id: 0x9001,
            method_resource_id: 0x1234,
            payload: "sample".into(),
            payload_alignment: 1,
            send_count: 1,
            send_interval_ms: 200,
            timeout_ms: 5000,
            rpc_response_delay_ms: 250,
            #[cfg(feature = "sustained-workload")]
            workload_plan: None,
        }
    }

    #[test]
    fn backend_identity_override_preserves_paired_fixture_addresses() {
        let mut settings = config();
        assert_eq!(
            settings.local_endpoint().unwrap(),
            UUri::try_from_parts("source", 0x2_4210, 3, 0).unwrap()
        );
        let metadata = settings.outbound_metadata().unwrap();
        assert_eq!(
            metadata.source(),
            &UUri::try_from_parts("source", 0x5BA0, 1, 0x9001).unwrap()
        );
        settings.role = FlowRole::PubReceiver;
        settings.local_authority = "sink".into();
        settings.peer_authority = "source".into();
        assert_eq!(
            settings.filters().unwrap(),
            (metadata.source().clone(), None)
        );
    }

    #[test]
    fn native_selection_requires_agreement_while_serialized_selection_does_not() {
        let mut settings = config();
        assert_eq!(settings.encoding().unwrap(), PayloadEncoding::PROTOBUF);
        settings.wire_format = FlowWireFormat::Native;
        assert!(settings.encoding().is_err());
    }

    #[derive(Default)]
    struct RecordingEndpoint {
        sent: Mutex<Vec<UOwnedFrame>>,
        request: Mutex<Option<UOwnedFrame>>,
        unregistered: AtomicUsize,
    }

    #[async_trait]
    impl UOwnedTransportImpl for RecordingEndpoint {
        async fn send_validated_owned(&self, frame: UOwnedFrame) -> Result<(), UStatus> {
            self.sent.lock().unwrap().push(frame);
            Ok(())
        }

        async fn register_validated_owned_listener(
            &self,
            source: &UUri,
            sink: Option<&UUri>,
            listener: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            let request = self.request.lock().unwrap().take().expect("RPC request");
            assert!(source.matches(request.metadata().source()));
            assert_eq!(sink, request.metadata().sink());
            listener.on_receive_owned(request).await;
            Ok(())
        }

        async fn unregister_validated_owned_listener(
            &self,
            _: &UUri,
            _: Option<&UUri>,
            _: Arc<dyn UOwnedListener>,
        ) -> Result<(), UStatus> {
            self.unregistered.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn repeated_connectivity_send_preserves_identity_and_bytes() {
        let endpoint = Arc::new(RecordingEndpoint::default());
        let transport: Arc<dyn UOwnedTransport> = endpoint.clone();
        let mut settings = config();
        settings.role = FlowRole::NotifySender;
        settings.send_count = 3;
        settings.send_interval_ms = 0;
        run_owned(transport, &settings).await.unwrap();
        let sent = endpoint.sent.lock().unwrap();
        assert_eq!(sent.len(), 3);
        for frame in sent.iter() {
            assert_eq!(frame.metadata(), sent[0].metadata());
            assert_eq!(frame.payload_bytes(), b"sample");
            assert_eq!(
                frame.metadata().payload_encoding(),
                Some(&PayloadEncoding::PROTOBUF)
            );
            assert_eq!(
                frame.metadata().sink(),
                Some(&UUri::try_from_parts("sink", 0x5BA0, 1, 0).unwrap())
            );
        }
    }

    #[tokio::test]
    async fn rpc_server_preserves_correlation_payload_and_cleans_listener() {
        let mut settings = config();
        settings.role = FlowRole::RpcClient;
        let request = settings.outbound_metadata().unwrap();
        let endpoint = Arc::new(RecordingEndpoint {
            request: Mutex::new(Some(
                UOwnedFrame::with_payload(request.clone(), b"request bytes".to_vec()).unwrap(),
            )),
            ..Default::default()
        });
        let transport: Arc<dyn UOwnedTransport> = endpoint.clone();
        settings.role = FlowRole::RpcServer;
        settings.local_authority = "sink".into();
        settings.peer_authority = "source".into();
        settings.rpc_response_delay_ms = 0;
        run_owned(transport, &settings).await.unwrap();
        let sent = endpoint.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let response = &sent[0];
        assert_eq!(response.metadata().source(), request.sink().unwrap());
        assert_eq!(response.metadata().sink(), Some(request.source()));
        assert_eq!(response.metadata().reqid(), Some(request.id()));
        assert_eq!(response.metadata().priority(), Some(UPriority::CS4));
        assert_eq!(response.metadata().ttl(), request.ttl());
        assert_eq!(response.payload_bytes(), b"request bytes");
        assert_eq!(endpoint.unregistered.load(Ordering::SeqCst), 1);
    }
}
