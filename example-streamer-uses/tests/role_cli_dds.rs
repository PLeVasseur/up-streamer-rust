// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Actual role CLI regression checks.
#![cfg(all(feature = "dds-transport", feature = "sustained-workload"))]
mod support;

#[test]
fn client_help() {
    support::help(
        env!("CARGO_BIN_EXE_dds_client"),
        include_str!("role_cli_help/dds_client.json"),
    );
}

#[test]
fn server_help() {
    support::help(
        env!("CARGO_BIN_EXE_dds_server"),
        include_str!("role_cli_help/dds_server.json"),
    );
}

#[test]
fn publisher_help() {
    support::help(
        env!("CARGO_BIN_EXE_dds_publisher"),
        include_str!("role_cli_help/dds_publisher.json"),
    );
}

#[test]
fn subscriber_help() {
    support::help(
        env!("CARGO_BIN_EXE_dds_subscriber"),
        include_str!("role_cli_help/dds_subscriber.json"),
    );
}

#[test]
fn notifier_help() {
    support::help(
        env!("CARGO_BIN_EXE_dds_notifier"),
        include_str!("role_cli_help/dds_notifier.json"),
    );
}

#[test]
fn notifyee_help() {
    support::help(
        env!("CARGO_BIN_EXE_dds_notifyee"),
        include_str!("role_cli_help/dds_notifyee.json"),
    );
}

#[test]
fn rejects_unknown_encoding_before_setup() {
    support::rejects(
        env!("CARGO_BIN_EXE_dds_publisher"),
        &["--encoding", "unknown-wire"],
        "invalid value",
    );
}

#[test]
fn rejects_retired_loan_family_and_foreign_role_options() {
    support::rejects(
        env!("CARGO_BIN_EXE_dds_publisher"),
        &["--route-family", "copy-minimized"],
        "invalid value",
    );
    support::rejects(
        env!("CARGO_BIN_EXE_dds_publisher"),
        &["--payload-alignment", "8"],
        "unexpected argument",
    );
}
