use std::sync::Arc;
use strum::EnumString;
use uprotocol_sdk::transport::datamodel::UTransport;

#[derive(Clone, Debug, EnumString, strum::Display)]
pub enum TransportType {
    UpClientZenoh,
    UpClientSommr,
    UpClientMqtt
}

#[derive(Clone)]
pub struct TaggedTransport {
    pub up_client: Arc<dyn UTransport>,
    pub tag: TransportType,
}

pub type TransportVec = Vec<TaggedTransport>;