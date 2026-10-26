// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual role CLI regression checks.
#![cfg(all(feature = "iceoryx2-selected-wire", feature = "sustained-workload"))]
mod support;

#[test]
fn client_help() {
    support::help(
        env!("CARGO_BIN_EXE_iceoryx2_client"),
        include_str!("role_cli_help/iceoryx2_client.json"),
    );
}

#[test]
fn server_help() {
    support::help(
        env!("CARGO_BIN_EXE_iceoryx2_server"),
        include_str!("role_cli_help/iceoryx2_server.json"),
    );
}

#[test]
fn publisher_help() {
    support::help(
        env!("CARGO_BIN_EXE_iceoryx2_publisher"),
        include_str!("role_cli_help/iceoryx2_publisher.json"),
    );
}

#[test]
fn subscriber_help() {
    support::help(
        env!("CARGO_BIN_EXE_iceoryx2_subscriber"),
        include_str!("role_cli_help/iceoryx2_subscriber.json"),
    );
}

#[test]
fn notifier_help() {
    support::help(
        env!("CARGO_BIN_EXE_iceoryx2_notifier"),
        include_str!("role_cli_help/iceoryx2_notifier.json"),
    );
}

#[test]
fn notifyee_help() {
    support::help(
        env!("CARGO_BIN_EXE_iceoryx2_notifyee"),
        include_str!("role_cli_help/iceoryx2_notifyee.json"),
    );
}

#[test]
fn rejects_unknown_encoding_before_setup() {
    support::rejects(
        env!("CARGO_BIN_EXE_iceoryx2_publisher"),
        &["--encoding", "unknown-wire"],
        "invalid value",
    );
}

#[test]
fn rejects_unknown_allocation_strategy_before_setup() {
    support::rejects(
        env!("CARGO_BIN_EXE_iceoryx2_publisher"),
        &[
            "--iceoryx2-publisher-allocation-strategy",
            "unknown-strategy",
        ],
        "invalid value",
    );
}
