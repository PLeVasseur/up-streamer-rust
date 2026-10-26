// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual role CLI regression checks.
#![cfg(all(feature = "zenoh-transport", feature = "sustained-workload"))]
mod support;

#[test]
fn client_help() {
    support::help(
        env!("CARGO_BIN_EXE_zenoh_client"),
        include_str!("role_cli_help/zenoh_client.json"),
    );
}

#[test]
fn server_help() {
    support::help(
        env!("CARGO_BIN_EXE_zenoh_server"),
        include_str!("role_cli_help/zenoh_server.json"),
    );
}

#[test]
fn publisher_help() {
    support::help(
        env!("CARGO_BIN_EXE_zenoh_publisher"),
        include_str!("role_cli_help/zenoh_publisher.json"),
    );
}

#[test]
fn subscriber_help() {
    support::help(
        env!("CARGO_BIN_EXE_zenoh_subscriber"),
        include_str!("role_cli_help/zenoh_subscriber.json"),
    );
}

#[test]
fn notifier_help() {
    support::help(
        env!("CARGO_BIN_EXE_zenoh_notifier"),
        include_str!("role_cli_help/zenoh_notifier.json"),
    );
}

#[test]
fn notifyee_help() {
    support::help(
        env!("CARGO_BIN_EXE_zenoh_notifyee"),
        include_str!("role_cli_help/zenoh_notifyee.json"),
    );
}

#[test]
fn rejects_unknown_encoding_before_setup() {
    support::rejects(
        env!("CARGO_BIN_EXE_zenoh_publisher"),
        &["--encoding", "unknown-wire"],
        "invalid value",
    );
}
