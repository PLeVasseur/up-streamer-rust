use up_rust::{Number, UAuthority, UEntity, UUri};

pub fn local_authority() -> UAuthority {
    UAuthority {
        name: Some("local_authority".to_string()),
        number: Number::Ip(vec![192, 168, 1, 100]).into(),
        ..Default::default()
    }
}

pub fn remote_authority_a() -> UAuthority {
    UAuthority {
        name: Some("remote_authority_a".to_string()),
        number: Number::Ip(vec![192, 168, 1, 200]).into(),
        ..Default::default()
    }
}

pub fn remote_authority_b() -> UAuthority {
    UAuthority {
        name: Some("remote_authority_b".to_string()),
        number: Number::Ip(vec![192, 168, 1, 201]).into(),
        ..Default::default()
    }
}

pub fn local_client_uuri(id: u32) -> UUri {
    UUri {
        authority: Some(local_authority()).into(),
        entity: Some(UEntity {
            name: format!("local_entity_{id}").to_string(),
            id: Some(id),
            version_major: Some(1),
            ..Default::default()
        })
        .into(),
        ..Default::default()
    }
}

pub fn remote_client_uuri(authority: UAuthority, id: u32) -> UUri {
    UUri {
        authority: Some(authority).into(),
        entity: Some(UEntity {
            name: format!("remote_entity_{id}").to_string(),
            id: Some(id),
            version_major: Some(1),
            ..Default::default()
        })
        .into(),
        ..Default::default()
    }
}
