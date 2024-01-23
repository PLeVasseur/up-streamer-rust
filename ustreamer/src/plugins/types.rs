use std::sync::Arc;
use uprotocol_sdk::transport::datamodel::UTransport;

enum TransportType {
    UpClientZenoh,
    UpClientSommr,
    UpClientMqtt
}

type TransportVec = Vec<(TransportType, Arc<dyn UTransport>)>;