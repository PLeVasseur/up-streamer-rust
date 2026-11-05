// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Exercise the real role binaries without starting native endpoints.
use std::process::Command;

fn validate_entity_defaults(text: &str) -> Result<usize, String> {
    let flags = [
        "--uentity",
        "--source-uentity",
        "--target-uentity",
        "--sink-uentity",
        "--ue-id",
    ];
    let mut pending_flag = None;
    let mut checked = 0;
    for line in text.lines() {
        if line.trim_start().starts_with('-') {
            if let Some(flag) = pending_flag {
                return Err(format!("missing default for {flag}"));
            }
            pending_flag = line
                .split_whitespace()
                .map(|token| token.trim_end_matches(','))
                .find(|token| flags.contains(token));
        }
        let Some(flag) = pending_flag else {
            continue;
        };
        let Some(default) = line
            .split_once("[default: ")
            .and_then(|(_, remainder)| remainder.split_once(']'))
            .map(|(value, _)| value)
        else {
            continue;
        };
        let entity = if let Some(hex) = default.strip_prefix("0x") {
            u32::from_str_radix(hex, 16)
        } else {
            default.parse::<u32>()
        }
        .map_err(|error| format!("invalid {flag} default {default}: {error}"))?;
        up_rust::UUri::try_from_parts("cli-defaults", entity, 1, 0)
            .map_err(|error| format!("invalid {flag} default {default}: {error}"))?;
        pending_flag = None;
        checked += 1;
    }
    if let Some(flag) = pending_flag {
        return Err(format!("missing default for {flag}"));
    }
    Ok(checked)
}

#[test]
fn entity_defaults_accept_packed_hex_and_decimal() {
    let help = "--uentity <UENTITY>\n    [default: 0x15BA0]\n--ue-id <UE_ID> [default: 88992]";
    assert_eq!(validate_entity_defaults(help).unwrap(), 2);
}

#[test]
fn entity_defaults_reject_zero_instances_for_every_entity_flag() {
    for flag in [
        "--uentity",
        "--source-uentity",
        "--target-uentity",
        "--sink-uentity",
        "--ue-id",
    ] {
        for default in ["0x5BA0", "23456"] {
            let help = format!("{flag} <ENTITY>\n    [default: {default}]");
            let error = validate_entity_defaults(&help).unwrap_err();
            assert!(error.contains(flag), "{error}");
            assert!(
                error.contains("entity instance ID must not be zero"),
                "{error}"
            );
        }
    }
}

#[test]
fn entity_defaults_reject_missing_or_malformed_defaults() {
    for help in [
        "--uentity <UENTITY>",
        "--uentity <UENTITY>\n--resource <RESOURCE> [default: 1]",
        "--uentity <UENTITY> [default: 0x1_5BA0]",
    ] {
        assert!(validate_entity_defaults(help).is_err(), "{help}");
    }
}

fn canonical(text: &str) -> String {
    text.lines()
        .map(|line| {
            let Some(start) = line.find("[default: /") else {
                return line.to_owned();
            };
            let Some(offset) = line[start..].find("/example-streamer-uses/") else {
                return line.to_owned();
            };
            format!(
                "{}[default: <example-dir>/{}",
                &line[..start],
                &line[start + offset + "/example-streamer-uses/".len()..]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn help(binary: &str, expected: &str) {
    let expected: String = serde_json::from_str(expected).expect("saved CLI help JSON string");
    let output = Command::new(binary).arg("--help").output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let actual = std::str::from_utf8(&output.stdout).unwrap();
    validate_entity_defaults(actual).unwrap();
    assert_eq!(canonical(actual), canonical(&expected));
}
pub fn rejects(binary: &str, arguments: &[&str], expected: &str) {
    let output = Command::new(binary).args(arguments).output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains(expected), "{text}");
}
