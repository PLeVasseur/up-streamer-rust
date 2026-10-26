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

//! Immutable URI projection key used for runtime routing map/set identity.

use up_rust::UUri;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct UriIdentityKey {
    authority_name: String,
    ue_id: u32,
    ue_version_major: u8,
    resource_id: u16,
}

fn uentity_id(uri: &UUri) -> u32 {
    (u32::from(uri.uentity_instance_id()) << 16) | u32::from(uri.uentity_type_id())
}

impl From<UUri> for UriIdentityKey {
    fn from(uri: UUri) -> Self {
        let authority_name = uri.authority_name().to_string();
        let ue_id = uentity_id(&uri);
        let ue_version_major = uri.uentity_major_version();
        let resource_id = uri.resource_id();

        Self {
            authority_name,
            ue_id,
            ue_version_major,
            resource_id,
        }
    }
}

impl From<&UUri> for UriIdentityKey {
    fn from(uri: &UUri) -> Self {
        Self {
            authority_name: uri.authority_name().to_string(),
            ue_id: uentity_id(uri),
            ue_version_major: uri.uentity_major_version(),
            resource_id: uri.resource_id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UriIdentityKey;
    use std::collections::HashSet;
    use up_rust::UUri;

    #[test]
    fn owned_and_borrowed_projection_are_identical() {
        let uri =
            UUri::try_from_parts("authority-a", 0x5BA0, 0x01, 0x8001).expect("URI should build");

        let key_from_borrowed = UriIdentityKey::from(&uri);
        let key_from_owned = UriIdentityKey::from(uri.clone());

        assert_eq!(key_from_owned, key_from_borrowed);
    }

    #[test]
    fn projection_uses_canonical_major_and_resource_semantics() {
        let uri =
            UUri::try_from_parts("authority-a", 0x5BA0, 0xFF, 0x8001).expect("URI should build");

        let key = UriIdentityKey::from(&uri);

        assert_eq!(key.ue_version_major, uri.uentity_major_version());
        assert_eq!(key.resource_id, uri.resource_id());
        assert_eq!(key.ue_version_major, 0xFF);
        assert_eq!(key.resource_id, 0x8001);
    }

    #[test]
    fn projection_key_hashing_dedupes_equal_projected_uris() {
        let normalized =
            UUri::try_from_parts("authority-a", 0x5BA0, 0xFF, 0x8001).expect("URI should build");
        let parsed = "//authority-a/5BA0/FF/8001"
            .parse::<UUri>()
            .expect("URI should parse");

        let mut seen = HashSet::new();
        seen.insert(UriIdentityKey::from(&normalized));
        seen.insert(UriIdentityKey::from(parsed));

        assert_eq!(seen.len(), 1);
    }
}
