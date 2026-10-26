// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Classic message flow over endpoints constructed by each role entrypoint.
#![allow(dead_code)]

use chrono::{Local, Timelike};
use std::{sync::Arc, time::Duration};
use up_rust::{UCode, UListener, UMessage, UStatus, UTransport, UUri};

/// Produce one message per iteration after the configured delay. None is an
/// unbounded run; Some(0) sends nothing. Role frontends select their count policy.
pub(crate) async fn send(
    transport: &dyn UTransport,
    count: Option<u64>,
    interval: Duration,
    mut message: impl FnMut(u64) -> Result<UMessage, UStatus>,
) -> Result<u64, UStatus> {
    let mut sent = 0_u64;
    while count.is_none_or(|limit| sent < limit) {
        tokio::time::sleep(interval).await;
        super::proof::send(transport, message(sent)?).await?;
        sent = sent.checked_add(1).ok_or_else(|| {
            UStatus::fail_with_code(UCode::OutOfRange, "classic send counter exhausted")
        })?;
    }
    Ok(sent)
}

/// Admit the listener before announcing readiness and wait for process shutdown.
pub(crate) async fn listen(
    transport: &dyn UTransport,
    source: &UUri,
    sink: Option<&UUri>,
    listener: Arc<dyn UListener>,
) -> Result<(), UStatus> {
    transport.register_listener(source, sink, listener).await?;
    println!("READY listener_registered");
    std::future::pending().await
}

/// Existing classic timer example payload, sampled when its send iteration runs.
pub(crate) fn timer_payload() -> Vec<u8> {
    use hello_world_protos::{hello_world_topics::Timer, timeofday::TimeOfDay};
    let now = Local::now();
    super::protobuf_payload(&Timer {
        time: Some(TimeOfDay {
            hours: now.hour() as i32,
            minutes: now.minute() as i32,
            seconds: now.second() as i32,
            nanos: now.nanosecond() as i32,
            ..Default::default()
        })
        .into(),
        ..Default::default()
    })
}
