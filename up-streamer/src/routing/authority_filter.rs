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

//! Authority-level URI filter helpers shared across route and data-plane internals.

use up_rust::UUri;

pub(crate) fn authority_to_wildcard_filter(authority_name: &str) -> UUri {
    UUri::try_from_parts(authority_name, 0xFFFF_FFFF, 0xFF, 0xFFFF)
        .expect("route authority wildcard filter should build")
}
