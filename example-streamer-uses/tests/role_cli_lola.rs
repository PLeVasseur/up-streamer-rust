// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual role CLI regression checks.
#![cfg(all(feature = "lola-selected-wire", feature = "sustained-workload"))]
mod support;

#[test]
fn client_help() {
    support::help(
        env!("CARGO_BIN_EXE_lola_client"),
        include_str!("role_cli_help/lola_client.json"),
    );
}

#[test]
fn server_help() {
    support::help(
        env!("CARGO_BIN_EXE_lola_server"),
        include_str!("role_cli_help/lola_server.json"),
    );
}

#[test]
fn publisher_help() {
    support::help(
        env!("CARGO_BIN_EXE_lola_publisher"),
        include_str!("role_cli_help/lola_publisher.json"),
    );
}

#[test]
fn subscriber_help() {
    support::help(
        env!("CARGO_BIN_EXE_lola_subscriber"),
        include_str!("role_cli_help/lola_subscriber.json"),
    );
}

#[test]
fn notifier_help() {
    support::help(
        env!("CARGO_BIN_EXE_lola_notifier"),
        include_str!("role_cli_help/lola_notifier.json"),
    );
}

#[test]
fn notifyee_help() {
    support::help(
        env!("CARGO_BIN_EXE_lola_notifyee"),
        include_str!("role_cli_help/lola_notifyee.json"),
    );
}

#[test]
fn rejects_unknown_encoding_before_setup() {
    support::rejects(
        env!("CARGO_BIN_EXE_lola_publisher"),
        &["--encoding", "unknown-wire"],
        "invalid value",
    );
}
