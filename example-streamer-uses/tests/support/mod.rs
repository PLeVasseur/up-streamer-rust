// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Exercise the real role binaries without starting native endpoints.
use std::process::Command;
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
    assert_eq!(
        canonical(std::str::from_utf8(&output.stdout).unwrap()),
        canonical(&expected)
    );
}
pub fn rejects(binary: &str, arguments: &[&str], expected: &str) {
    let output = Command::new(binary).args(arguments).output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains(expected), "{text}");
}
