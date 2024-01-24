use std::sync::Arc;
use strum::EnumString;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::UMessage;

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
