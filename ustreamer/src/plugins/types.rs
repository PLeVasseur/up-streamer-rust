use crate::plugins::up_client_full::{UpClientFull, UpClientFullFactory};
use async_std::channel::{Receiver, Sender};
use async_std::sync::{Arc, Mutex};
use async_std::task;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use strum::EnumString;
use uprotocol_rust_transport_mqtt::UTransportMqtt;
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{u_authority, UAuthority, UMessage, UUri};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::config::WhatAmI;

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

pub struct ULinkZenohFactory {}
impl UpClientFullFactory for ULinkZenohFactory {
    fn transport_type(&self) -> &'static TransportType {
        &TransportType::UpClientZenoh
    }
    fn create_up_client(
        &self,
    ) -> Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Arc<Mutex<Box<dyn UpClientFull>>>> + Send>>>
    {
        Box::new(|| {
            Box::pin(async move {
                let mut up_client_config = zenoh::config::Config::default();
                up_client_config
                    .set_mode(Some(WhatAmI::Peer))
                    .expect("Unable to configure as Peer");
                let up_client: Arc<Mutex<Box<dyn UpClientFull>>> = Arc::new(Mutex::new(Box::new(
                    ULinkZenoh::new_from_config(up_client_config).await.unwrap(),
                )));
                up_client
            })
        })
    }
}

pub struct UTransportSommrFactory {}
impl UpClientFullFactory for UTransportSommrFactory {
    fn transport_type(&self) -> &'static TransportType {
        &TransportType::UpClientSommr
    }
    fn create_up_client(
        &self,
    ) -> Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Arc<Mutex<Box<dyn UpClientFull>>>> + Send>>>
    {
        Box::new(|| {
            Box::pin(async move {
                let mut up_client_config = zenoh::config::Config::default();
                up_client_config
                    .set_mode(Some(WhatAmI::Peer))
                    .expect("Unable to configure as Peer");
                let up_client: Arc<Mutex<Box<dyn UpClientFull>>> = Arc::new(Mutex::new(Box::new(
                    UTransportSommr::new_from_config(up_client_config)
                        .await
                        .unwrap(),
                )));
                up_client
            })
        })
    }
}

pub struct UTransportMqttFactory {}
impl UpClientFullFactory for UTransportMqttFactory {
    fn transport_type(&self) -> &'static TransportType {
        &TransportType::UpClientMqtt
    }
    fn create_up_client(
        &self,
    ) -> Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Arc<Mutex<Box<dyn UpClientFull>>>> + Send>>>
    {
        Box::new(|| {
            Box::pin(async move {
                let mut up_client_config = zenoh::config::Config::default();
                up_client_config
                    .set_mode(Some(WhatAmI::Peer))
                    .expect("Unable to configure as Peer");
                let up_client: Arc<Mutex<Box<dyn UpClientFull>>> = Arc::new(Mutex::new(Box::new(
                    UTransportMqtt::new_from_config(up_client_config)
                        .await
                        .unwrap(),
                )));
                up_client
            })
        })
    }
}
