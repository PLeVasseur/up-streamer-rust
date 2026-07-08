pub(crate) mod cli;
#[cfg(all(feature = "selected-wire-common", feature = "up-wire-xcdrv2"))]
pub(crate) mod payloads;

use async_trait::async_trait;
use hello_world_protos::{
    hello_world_service::{HelloRequest, HelloResponse},
    hello_world_topics::Timer,
};
use protobuf::Message;
use std::sync::Arc;
use tracing::{debug, error, info};
use up_rust::{
    PayloadEncoding, UCode, UListener, UMessage, UMessageBuilder, UPayloadFormat, UStatus,
    UTransport,
};

pub(crate) fn protobuf_payload(message: &impl Message) -> Vec<u8> {
    message.write_to_bytes().unwrap()
}

#[allow(dead_code)]
pub(crate) struct ServiceResponseListener;

#[async_trait]
impl UListener for ServiceResponseListener {
    async fn on_receive(&self, msg: UMessage) {
        info!("ServiceResponseListener: Received a message: {msg:?}");

        let Some(payload_bytes) = msg.payload() else {
            panic!("No payload bytes");
        };

        match HelloResponse::parse_from_bytes(&payload_bytes) {
            Ok(hello_response) => debug!("Here we received response: {hello_response:?}"),
            Err(err) => error!("Unable to parse into HelloResponse: {err:?}"),
        }
        info!(
            "FLOW observed_payload_bytes={} role=classic_response_listener",
            payload_bytes.len()
        );
    }
}

#[allow(dead_code)]
pub(crate) struct ServiceRequestResponder {
    client: Arc<dyn UTransport>,
}
impl ServiceRequestResponder {
    #[allow(dead_code)]
    pub(crate) fn new(client: Arc<dyn UTransport>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl UListener for ServiceRequestResponder {
    async fn on_receive(&self, msg: UMessage) {
        info!("ServiceResponseListener: Received a message: {msg:?}");

        let Some(payload_bytes) = msg.payload() else {
            panic!("No bytes available");
        };
        let (response_payload, response_encoding) =
            match HelloRequest::parse_from_bytes(&payload_bytes) {
                Ok(hello_request) => {
                    debug!("hello_request: {hello_request:?}");
                    let hello_response = HelloResponse {
                        message: format!("The response to the request: {}", hello_request.name),
                        ..Default::default()
                    };
                    (protobuf_payload(&hello_response), PayloadEncoding::PROTOBUF)
                }
                Err(err) => {
                    error!("Unable to parse HelloRequest: {err:?}");
                    let response_encoding = match message_payload_encoding(&msg) {
                        Ok(encoding) => encoding,
                        Err(error) => {
                            error!("Unable to preserve request payload encoding: {error:?}");
                            return;
                        }
                    };
                    (payload_bytes.to_vec(), response_encoding)
                }
            };

        let attributes = msg.attributes();

        let response_msg = UMessageBuilder::response_for_request(attributes)
            .build_with_payload_encoding(response_payload, response_encoding)
            .unwrap();
        info!("Sending Response message:\n{:?}", &response_msg);
        self.client.send(response_msg).await.unwrap();
    }
}

pub(crate) fn native_message_payload_parts(
    magic: u32,
    sequence: u32,
    payload: &str,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    native_message_payload_parts_impl(magic, sequence, payload)
}

#[cfg(all(feature = "selected-wire-common", feature = "up-wire-xcdrv2"))]
fn native_message_payload_parts_impl(
    magic: u32,
    sequence: u32,
    payload: &str,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    use up_rust::StableContainerPayload;

    Ok((
        payloads::native_payload_bytes(magic, sequence, payload)?,
        StableContainerPayload::<payloads::SelectedWireNativePayload>::encoding(),
    ))
}

#[cfg(not(all(feature = "selected-wire-common", feature = "up-wire-xcdrv2")))]
fn native_message_payload_parts_impl(
    _magic: u32,
    _sequence: u32,
    _payload: &str,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    Err(invalid_argument(
        "native selected-wire payload examples require selected-wire-common and up-wire-xcdrv2 features",
    ))
}

pub(crate) fn xcdrv2_message_payload_parts(
    sequence: u32,
    source: String,
    payload: &str,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    xcdrv2_message_payload_parts_impl(sequence, source, payload)
}

#[cfg(all(feature = "selected-wire-common", feature = "up-wire-xcdrv2"))]
fn xcdrv2_message_payload_parts_impl(
    sequence: u32,
    source: String,
    payload: &str,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    use up_rust::PayloadFormat;

    Ok((
        payloads::xcdrv2_payload_bytes(sequence, source, payload)?,
        up_wire_xcdrv2::XcdrV2Wire::encoding(),
    ))
}

#[cfg(not(all(feature = "selected-wire-common", feature = "up-wire-xcdrv2")))]
fn xcdrv2_message_payload_parts_impl(
    _sequence: u32,
    _source: String,
    _payload: &str,
) -> Result<(Vec<u8>, PayloadEncoding), UStatus> {
    Err(invalid_argument(
        "XCDRv2 selected-wire payload examples require selected-wire-common and up-wire-xcdrv2 features",
    ))
}

fn message_payload_encoding(msg: &UMessage) -> Result<PayloadEncoding, UStatus> {
    let attributes = msg.attributes();
    let (registry_id, literal_id, content_type) = attributes.open_payload_encoding_parts();
    if registry_id.is_some() || literal_id.is_some() || content_type.is_some() {
        return PayloadEncoding::from_parts(
            registry_id,
            literal_id.map(str::to_owned),
            content_type.map(str::to_owned),
        )
        .map_err(|error| invalid_argument(format!("invalid open payload encoding: {error}")));
    }

    let format = msg
        .payload_format()
        .filter(|format| *format != UPayloadFormat::Unspecified)
        .unwrap_or(UPayloadFormat::Protobuf);
    PayloadEncoding::try_from_legacy_format(format)
        .map_err(|error| invalid_argument(format!("invalid legacy payload format: {error}")))
}

fn invalid_argument(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

#[allow(dead_code)]
pub(crate) struct PublishReceiver;

#[async_trait]
impl UListener for PublishReceiver {
    async fn on_receive(&self, msg: UMessage) {
        info!("PublishReceiver: Received a message: {msg:?}");

        let Some(payload_bytes) = msg.payload() else {
            panic!("No bytes available");
        };
        match Timer::parse_from_bytes(&payload_bytes) {
            Ok(timer_message) => {
                debug!("timer: {timer_message:?}");
            }
            Err(err) => {
                error!("Unable to parse Timer Message: {err:?}");
            }
        };
    }
}
