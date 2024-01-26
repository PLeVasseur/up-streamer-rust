/********************************************************************************
 * Copyright (c) 2023 Contributors to the Eclipse Foundation
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

use async_std::sync::Arc;
use std::hash::{Hash, Hasher};
use strum::EnumString;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{u_authority, UAuthority, UMessage};

#[derive(Clone, Debug, Eq, Hash, PartialEq, EnumString, strum::Display)]
pub enum TransportType {
    Multicast,
    UpClientZenoh,
    UpClientSommr,
    UpClientMqtt,
}

#[derive(Clone)]
pub struct TaggedTransport {
    pub up_client: Arc<dyn UTransport>,
    pub tag: TransportType,
}

pub type TransportVec = Vec<TaggedTransport>;

// options: 1. struct containing type + UMessage 2. enum where each element contains the UMessage
#[derive(Clone, Debug, EnumString, strum::Display)]
pub enum TaggedUMessage {
    UpClientZenoh(UMessage),
    UpClientSommr(UMessage),
    UpClientMqtt(UMessage),
}

#[derive(Clone, Debug)]
pub struct UMessageWithRouting {
    pub msg: UMessage,
    pub src: TransportType,
    pub dst: TransportType,
}

pub struct HashableAuthority(pub UAuthority);

impl PartialEq for HashableAuthority {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0.remote, &other.0.remote) {
            (Some(u_authority::Remote::Name(name1)), Some(u_authority::Remote::Name(name2))) => {
                name1 == name2
            }
            (Some(u_authority::Remote::Ip(ip1)), Some(u_authority::Remote::Ip(ip2))) => ip1 == ip2,
            (Some(u_authority::Remote::Id(id1)), Some(u_authority::Remote::Id(id2))) => id1 == id2,
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for HashableAuthority {}

impl Hash for HashableAuthority {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match &self.0.remote {
            Some(u_authority::Remote::Name(name)) => {
                1.hash(state); // Discriminant for the Name variant
                name.hash(state);
            }
            Some(u_authority::Remote::Ip(ip)) => {
                2.hash(state); // Discriminant for the Ip variant
                ip.hash(state);
            }
            Some(u_authority::Remote::Id(id)) => {
                3.hash(state); // Discriminant for the Id variant
                id.hash(state);
            }
            None => {
                0.hash(state); // Discriminant for None
            }
        }
    }
}
