// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Creation-reserved native mechanics for the compatibility payload-flow command.

use super::*;
pub(super) use configurable_streamer_wire_support::prepared::PreparedReceived as Received;
use configurable_streamer_wire_support::prepared::PreparedSession;
use std::sync::Mutex;
use up_rust::{PreparedTransportCore, RegistrationConfig, RegistrationInbox, TxHeaderParts};
use up_streamer::PreparedEndpoint;

type NativeRx<T> = <<T as PreparedEndpoint>::Core as PreparedTransportCore>::Rx;
pub(super) struct PreparedFlow<T: PreparedEndpoint> {
    session: PreparedSession<T>,
    inbox: Arc<RegistrationInbox<NativeRx<T>>>,
    subscription: Mutex<Option<<T::Core as PreparedTransportCore>::Subscription>>,
}
impl<T: PreparedEndpoint> PreparedFlow<T> {
    pub(super) async fn new(transport: Arc<T>, cli: &Cli) -> Result<Self, UStatus> {
        let definitions = catalog(cli)?;
        let session = PreparedSession::new(transport, &definitions)?;
        let inbox = Arc::new(
            RegistrationInbox::new(RegistrationConfig {
                capacity: 4,
                overflow: up_rust::OverflowPolicy::Block {
                    deadline: Duration::from_millis(cli.timeout_ms),
                },
            })
            .map_err(|error| invalid_config(error.to_string()))?,
        );
        let mut subscription = None;
        for definition in definitions.iter().filter(|entry| !entry.transmit) {
            let entry = definition.reservation()?;
            subscription = Some(
                session
                    .core()
                    .attach(&entry.source, entry.sink.as_ref(), inbox.clone())
                    .await
                    .map_err(|error| invalid_config(error.to_string()))?,
            );
        }
        Ok(Self {
            session,
            inbox,
            subscription: Mutex::new(subscription),
        })
    }
    pub(super) fn send(&self, metadata: &UFrameMetadata, payload: &[u8]) -> Result<(), UStatus> {
        self.session
            .send(TxHeaderParts::from(metadata), Some(payload))
            .map_err(|error| invalid_config(error.to_string()))
    }
    pub(super) async fn receive(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        timeout_ms: u64,
    ) -> Result<Received<NativeRx<T>>, UStatus> {
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .ok_or_else(|| invalid_config("receive deadline overflow"))?;
        loop {
            let raw = tokio::time::timeout_at(deadline, self.inbox.recv())
                .await
                .map_err(|_| {
                    UStatus::fail_with_code(UCode::DeadlineExceeded, "prepared receive timed out")
                })?
                .ok_or_else(|| {
                    UStatus::fail_with_code(UCode::Unavailable, "prepared receiver closed")
                })?;
            let frame = match self.session.admit(raw) {
                Ok(frame) => frame,
                Err(_) => continue,
            };
            let header = frame
                .header()
                .map_err(|error| invalid_config(error.to_string()))?;
            if !source.matches(&header.source)
                || !match (sink, header.sink.as_ref()) {
                    (None, None) => true,
                    (Some(filter), Some(actual)) => filter.matches(actual),
                    _ => false,
                }
            {
                continue;
            }
            let received = self.session.decode_compatibility_frame(frame)?;
            if let Some(agreement) = received.native_agreement() {
                NativeContext::from_agreement(agreement.clone())?.verify_view(&received)?;
            } else if received.metadata().native_type_token().is_some() {
                return Err(invalid_config("native receive has no configured agreement"));
            }
            return Ok(received);
        }
    }
}
impl<T: PreparedEndpoint> Drop for PreparedFlow<T> {
    fn drop(&mut self) {
        self.inbox.close();
        if let Some(mut subscription) = self.subscription.get_mut().unwrap().take() {
            let _ = self.session.core().detach(&mut subscription);
        }
    }
}

pub(super) fn catalog(
    cli: &Cli,
) -> Result<Vec<configurable_streamer_wire_support::prepared::PreparedAddress>, UStatus> {
    use configurable_streamer_wire_support::prepared::PreparedAddress;
    let entry = |source: UUri, sink: Option<UUri>, transmit| -> Result<PreparedAddress, UStatus> {
        Ok(PreparedAddress {
            source: source.to_uri(false),
            sink: sink.map(|uri| uri.to_uri(false)),
            max_metadata_len: 3072,
            max_payload_len: payload_bytes(cli)?.len(),
            payload_alignment: payload_alignment(cli),
            max_token_len: 2048,
            allow_traceparent: true,
            transmit,
        })
    };
    Ok(match cli.role {
        FlowRole::PubSender => vec![entry(
            topic_uri(&cli.local_authority, cli.topic_resource_id)?,
            None,
            true,
        )?],
        FlowRole::PubReceiver => vec![entry(
            topic_uri(&cli.peer_authority, cli.topic_resource_id)?,
            None,
            false,
        )?],
        FlowRole::NotifySender => vec![entry(
            topic_uri(&cli.local_authority, cli.topic_resource_id)?,
            Some(endpoint_uri(&cli.peer_authority)?),
            true,
        )?],
        FlowRole::NotifyReceiver => vec![entry(
            topic_uri(&cli.peer_authority, cli.topic_resource_id)?,
            Some(endpoint_uri(&cli.local_authority)?),
            false,
        )?],
        FlowRole::RpcClient => vec![
            entry(
                endpoint_uri(&cli.local_authority)?,
                Some(method_uri(&cli.peer_authority, cli.method_resource_id)?),
                true,
            )?,
            entry(
                method_uri(&cli.peer_authority, cli.method_resource_id)?,
                Some(endpoint_uri(&cli.local_authority)?),
                false,
            )?,
        ],
        FlowRole::RpcServer => vec![
            entry(
                endpoint_uri(&cli.peer_authority)?,
                Some(method_uri(&cli.local_authority, cli.method_resource_id)?),
                false,
            )?,
            entry(
                method_uri(&cli.local_authority, cli.method_resource_id)?,
                Some(endpoint_uri(&cli.peer_authority)?),
                true,
            )?,
        ],
    })
}
