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

use up_rust::UUri;

/// Returns the logical authority label for local test clients.
pub fn local_authority() -> String {
    "local_authority".to_string()
}

/// Returns the first logical remote authority label.
pub fn remote_authority_a() -> String {
    "remote_authority_a".to_string()
}

/// Returns the second logical remote authority label.
pub fn remote_authority_b() -> String {
    "remote_authority_b".to_string()
}

/// Builds a local fixture URI with the given entity ID, major version 1 and resource 2.
pub fn local_client_uuri(id: u32) -> UUri {
    UUri::try_from_parts(&local_authority(), id, 1, 2).expect("local client URI should build")
}

/// Builds a remote fixture URI with the supplied authority/entity, major version 1 and resource 2.
pub fn remote_client_uuri(authority: String, id: u32) -> UUri {
    UUri::try_from_parts(&authority, id, 1, 2).expect("remote client URI should build")
}
