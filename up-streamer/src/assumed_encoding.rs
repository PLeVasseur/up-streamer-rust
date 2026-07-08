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

//! R3A: assumed payload-encoding decorator for classic transports that have
//! no payload-format side-channel (per the SOME/IP binding spec,
//! `payload_format` is "fixed per topic" and travels as convention, not
//! data). The decorator makes that convention explicit and auditable:
//!
//! - **receive**: messages arriving with a payload but no declared encoding
//!   are stamped with the configured assumption before reaching listeners, so
//!   downstream projections (the classic bridge) are total.
//! - **send**: outgoing messages must MATCH the configured assumption; a
//!   mismatch is a loud error, never a silent relabel — the same discipline
//!   as `to_legacy_format`'s "never as Raw" rule.

use std::sync::Arc;

use async_trait::async_trait;
use up_rust::{PayloadEncoding, UCode, UListener, UMessage, UStatus, UTransport, UUri};

/// Wraps a classic transport whose wire cannot carry payload-encoding
/// metadata, applying a per-endpoint "fixed per topic" encoding convention.
pub struct AssumedEncodingTransport {
    inner: Arc<dyn UTransport>,
    assumed: PayloadEncoding,
}

impl AssumedEncodingTransport {
    pub fn new(inner: Arc<dyn UTransport>, assumed: PayloadEncoding) -> Self {
        Self { inner, assumed }
    }

    fn message_encoding_matches(&self, message: &UMessage) -> Result<(), UStatus> {
        let attributes = message.attributes();
        let legacy = attributes.payload_format();
        let (registry_id, literal, content_type) = attributes.open_payload_encoding_parts();
        let declared = match (legacy, registry_id, literal, content_type) {
            (None | Some(up_rust::UPayloadFormat::Unspecified), None, None, None) => {
                // Undeclared: acceptable, the wire cannot carry it anyway and
                // the peer applies the same per-topic convention.
                return Ok(());
            }
            (Some(format), None, None, None) => PayloadEncoding::try_from_legacy_format(format)
                .map_err(|e| UStatus::fail_with_code(UCode::InvalidArgument, e.to_string()))?,
            (None | Some(up_rust::UPayloadFormat::Unspecified), id, lit, ct) => {
                PayloadEncoding::from_parts(id, lit.map(str::to_owned), ct.map(str::to_owned))
                    .map_err(|e| UStatus::fail_with_code(UCode::InvalidArgument, e.to_string()))?
            }
            _ => {
                return Err(UStatus::fail_with_code(
                    UCode::InvalidArgument,
                    "message declares both a concrete payload format and open \
                     payload-encoding fields",
                ));
            }
        };
        if declared != self.assumed {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                format!(
                    "message payload encoding `{}` does not match this endpoint's \
                     assumed per-topic encoding `{}`; refusing to send unlabeled",
                    declared.describe(),
                    self.assumed.describe()
                ),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl UTransport for AssumedEncodingTransport {
    async fn send(&self, message: UMessage) -> Result<(), UStatus> {
        self.message_encoding_matches(&message)?;
        self.inner.send(message).await
    }

    async fn register_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        listener: Arc<dyn UListener>,
    ) -> Result<(), UStatus> {
        let stamping = Arc::new(StampingListener {
            inner: listener,
            assumed: self.assumed.clone(),
        });
        self.inner
            .register_listener(source_filter, sink_filter, stamping)
            .await
    }

    async fn unregister_listener(
        &self,
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
        _listener: Arc<dyn UListener>,
    ) -> Result<(), UStatus> {
        // Note for productionization: unregistration by identity requires a
        // wrapper registry keyed by the inner listener; the classic bridge
        // holds routes for the process lifetime, so lookup-on-unregister is
        // an R3A-T-VSOMEIP completion item, tracked in the plan.
        self.inner
            .unregister_listener(source_filter, sink_filter, _listener)
            .await
    }
}

struct StampingListener {
    inner: Arc<dyn UListener>,
    assumed: PayloadEncoding,
}

#[async_trait]
impl UListener for StampingListener {
    async fn on_receive(&self, msg: UMessage) {
        let msg = if msg.payload().is_some()
            && matches!(
                msg.attributes().payload_format(),
                None | Some(up_rust::UPayloadFormat::Unspecified)
            )
            && msg.attributes().open_payload_encoding_parts() == (None, None, None)
        {
            msg.with_assumed_payload_encoding(&self.assumed)
        } else {
            msg
        };
        self.inner.on_receive(msg).await;
    }
}
