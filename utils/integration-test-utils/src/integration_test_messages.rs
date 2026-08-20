/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
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

use crate::local_client_uuri;
use up_rust::{UMessage, UMessageBuilder, UUri, UUID};

fn method_uri_from(endpoint: &UUri) -> UUri {
    endpoint.clone_with_resource_id(0)
}

pub fn publish_from_local_client_for_remote_client(local_id: u32) -> UMessage {
    UMessageBuilder::publish(local_client_uuri(local_id))
        .build()
        .expect("publish message should build")
}

pub fn notification_from_local_client_for_remote_client(
    local_id: u32,
    remote_uuri: UUri,
) -> UMessage {
    UMessageBuilder::notification(local_client_uuri(local_id), method_uri_from(&remote_uuri))
        .build()
        .expect("notification message should build")
}

pub fn request_from_local_client_for_remote_client(local_id: u32, remote_uuri: UUri) -> UMessage {
    UMessageBuilder::request(
        remote_uuri,
        method_uri_from(&local_client_uuri(local_id)),
        5_000,
    )
    .build()
    .expect("request message should build")
}

pub fn response_from_local_client_for_remote_client(local_id: u32, remote_uuri: UUri) -> UMessage {
    UMessageBuilder::response(
        method_uri_from(&remote_uuri),
        UUID::build(),
        local_client_uuri(local_id),
    )
    .build()
    .expect("response message should build")
}

pub fn publish_from_remote_client_for_local_client(remote_uuri: UUri) -> UMessage {
    UMessageBuilder::publish(remote_uuri)
        .build()
        .expect("publish message should build")
}

pub fn notification_from_remote_client_for_local_client(
    remote_uuri: UUri,
    local_id: u32,
) -> UMessage {
    UMessageBuilder::notification(remote_uuri, method_uri_from(&local_client_uuri(local_id)))
        .build()
        .expect("notification message should build")
}

pub fn request_from_remote_client_for_local_client(remote_uuri: UUri, local_id: u32) -> UMessage {
    UMessageBuilder::request(
        local_client_uuri(local_id),
        method_uri_from(&remote_uuri),
        5_000,
    )
    .build()
    .expect("request message should build")
}

pub fn response_from_remote_client_for_local_client(remote_uuri: UUri, local_id: u32) -> UMessage {
    UMessageBuilder::response(
        method_uri_from(&local_client_uuri(local_id)),
        UUID::build(),
        remote_uuri,
    )
    .build()
    .expect("response message should build")
}
