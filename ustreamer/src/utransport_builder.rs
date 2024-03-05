use std::collections::HashMap;
use crate::hashable_items::{HashableUAuthority, HashableUUID};
use async_std::channel::{Receiver, SendError, Sender};
use async_std::sync::Mutex;
use async_std::task;
use async_trait::async_trait;
use log::{error, trace};
use lru::LruCache;
use std::sync::Arc;
use up_rust::transport::datamodel::UTransport;
use up_rust::uprotocol::{UAttributes, UAuthority, UMessage, UMessageType, UStatus, UUri};
use crate::ustreamer::TransportTag;

const TAG: &str = "UTransportBuilder";

async fn transport_listener(
    result: Result<UMessage, UStatus>,
    host_transport: bool,
    host_transport_tag: Option<TransportTag>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    ingress_sender: Sender<UMessage>,
    egress_sender: Sender<UMessage>,
    transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
) {
    // TODO: Consider how to gracefully shut this down

    // Check if we have seen this message already based on UMessage.attributes.id
    //  => If we have, we have already transmitted this message once, we can drop it
    let Ok(message) = result else {
        error!(
            "{TAG}: Received an erroneous UStatus: {:?}",
            &result.err().unwrap()
        );
        return;
    };
    let UMessage { attributes, .. } = &message;
    let attr = attributes.clone();
    let Some(attr) = attr.as_ref() else {
        error!("{TAG}: Received a UMessage without UAttributes");
        return;
    };
    let Some(id) = attr.id.as_ref() else {
        error!("{TAG}: Received a UMessage whose UAttributes does not have an id");
        return;
    };
    if transmit_cache
        .lock()
        .await
        .contains(&HashableUUID(id.clone()))
    {
        trace!(
            "{TAG}: Received a UMessage we have already forwarded: {}",
            String::from(id)
        );
        return;
    }

    // If this message is intended for our device, then place it into the ingress queue
    // otherwise place it into the egress queue
    let Ok(message_type) = attr.type_.enum_value() else {
        error!("{TAG}: No message type supplied");
        return;
    };

    match message_type {
        UMessageType::UMESSAGE_TYPE_UNSPECIFIED => {
            error!("{TAG}: UMessageType is UMESSAGE_TYPE_UNSPECIFIED. Cannot route.");
            return;
        }
        UMessageType::UMESSAGE_TYPE_PUBLISH => {
            if attr.sink.is_some() {
                // if a Notification with a sink
                send_to_ingress_or_egress(&host_transport_tag, authority_routes, ingress_sender, egress_sender, message, attr).await;
            } else {
                // if a Publish with only a source
                if attr.source.is_some() {
                    send_to_ingress_and_egress(&host_transport_tag, authority_routes, ingress_sender, egress_sender, message, attr).await;
                } else {
                    error!("{TAG}: UMessageType is UMESSAGE_TYPE_PUBLISH, but no sink or source UUri. Unable to route.");
                    return;
                }
            }
        }
        UMessageType::UMESSAGE_TYPE_REQUEST => {
            if attr.sink.is_some() {
                send_to_ingress_or_egress(&host_transport_tag, authority_routes, ingress_sender, egress_sender, message, attr).await;
            } else {
                error!("{TAG}: UMessageType is UMESSAGE_TYPE_REQUEST, but no sink UUri. Unable to route.");
                return;
            }
        }
        UMessageType::UMESSAGE_TYPE_RESPONSE => {
            if attr.sink.is_some() {
                send_to_ingress_or_egress(&host_transport_tag, authority_routes, ingress_sender, egress_sender, message, attr).await;
            } else {
                error!("{TAG}: UMessageType is UMESSAGE_TYPE_RESPONSE, but no sink UUri. Unable to route.");
                return;
            }
        }
    }
}

async fn send_to_ingress_and_egress(host_transport_tag: &Option<TransportTag>, authority_routes: HashMap<HashableUAuthority, TransportTag>, ingress_sender: Sender<UMessage>, egress_sender: Sender<UMessage>, message: UMessage, attr: &UAttributes) {
    let Some(authority) = attr.sink.authority.as_ref() else {
        error!("{TAG}: UAuthority missing from UUri. Unable to route.");
        return;
    };

    let transport_tag = authority_routes.get(&HashableUAuthority(authority.clone()));
    if transport_tag.is_none() {
        error!("{TAG}: Route not setup for UAuthority: {:?}", &authority);
        return;
    }
    let host_routing_possible = host_transport_tag.is_some() && transport_tag.is_some();
    if host_routing_possible && &host_transport_tag.unwrap() == transport_tag.unwrap() {
        match ingress_sender.send(message.clone()).await {
            Ok(_) => {}
            Err(e) => {
                error!("{TAG}: Unable to send UMessage to ingress: {:?}", e);
                return;
            }
        }
    }
    match egress_sender.send(message).await {
        Ok(_) => {}
        Err(e) => {
            error!("{TAG}: Unable to send UMessage to egress: {:?}", e);
            return;
        }
    }

}

async fn send_to_ingress_or_egress(host_transport_tag: &Option<TransportTag>, authority_routes: HashMap<HashableUAuthority, TransportTag>, ingress_sender: Sender<UMessage>, egress_sender: Sender<UMessage>, message: UMessage, attr: &UAttributes) {
    let Some(authority) = attr.sink.authority.as_ref() else {
        error!("{TAG}: UAuthority missing from UUri. Unable to route.");
        return;
    };

    let transport_tag = authority_routes.get(&HashableUAuthority(authority.clone()));
    if transport_tag.is_none() {
        error!("{TAG}: Route not setup for UAuthority: {:?}", &authority);
        return;
    }
    let host_routing_possible = host_transport_tag.is_some() && transport_tag.is_some();
    if host_routing_possible && &host_transport_tag.unwrap() == transport_tag.unwrap() {
        match ingress_sender.send(message).await {
            Ok(_) => {}
            Err(e) => {
                error!("{TAG}: Unable to send UMessage to ingress: {:?}", e);
                return;
            }
        }
    } else {
        match egress_sender.send(message).await {
            Ok(_) => {}
            Err(e) => {
                error!("{TAG}: Unable to send UMessage to egress: {:?}", e);
                return;
            }
        }
    }
}

async fn transmit_loop(
    transmit_request_receiver: Receiver<UMessage>,
    utransport: Box<dyn UTransport>,
    transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
) {
    // Loop over and consume from transmit_queue
    while let Ok(mut message) = transmit_request_receiver.recv().await {
        // TODO: Consider how to gracefully shut this down

        let hashed_id = {
            if let Some(id) = message.attributes.id.as_ref() {
                HashableUUID(id.clone())
            } else {
                error!("{TAG}: Received UMessage to transmit, but no UUID id attached.");
                continue;
            }
        };

        // TODO: Save the message we received here
        //  => Probably also wanna save some metadata, such as the transport received over / sent on

        // Send out over transport
        match utransport.send(message).await {
            Ok(_) => {
                transmit_cache.lock().await.put(hashed_id, true);
            }
            Err(e) => {
                error!(
                    "{TAG}: Unable to transmit message over utransport, error: {:?}",
                    e
                )
            }
        }
    }
}

fn streamer_uuri_from_uauthority(authority: &UAuthority) -> UUri {
    UUri {
        authority: Some(authority.clone()).into(),
        ..Default::default()
    }
}

pub trait UTransportBuilder: Send + Sync {
    fn build(&self) -> Box<dyn UTransport>;

    fn start(
        &self,
        authorities: Vec<UAuthority>,
        host_transport: bool,
        host_transport_tag: Option<TransportTag>,
        authority_routes: HashMap<HashableUAuthority, TransportTag>,
        ingress_sender: Sender<UMessage>,
        egress_sender: Sender<UMessage>,
        transmit_request_receiver: Receiver<UMessage>,
        transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
    ) {
        trace!("entered create_and_setup");

        let utransport = self.build();

        task::spawn_local(async move {
            for authority in &authorities {
                let host_transport = host_transport.clone();
                let host_transport_tag = host_transport_tag.clone();
                let ingress_sender = ingress_sender.clone();
                let egress_sender = egress_sender.clone();
                let transmit_cache = transmit_cache.clone();
                let authority_routes = authority_routes.clone();
                let closure_listener = move |result: Result<UMessage, UStatus>| {
                    task::spawn_local(transport_listener(
                        result,
                        host_transport.clone(),
                        host_transport_tag.clone(),
                        authority_routes.clone(),
                        ingress_sender.clone(),
                        egress_sender.clone(),
                        transmit_cache.clone(),
                    ));
                };

                let register_success = utransport
                    .register_listener(
                        streamer_uuri_from_uauthority(authority),
                        Box::new(closure_listener),
                    )
                    .await;
                match register_success {
                    Ok(_) => {}
                    Err(e) => {
                        error!(
                            "{TAG}: Unable to register authority: {:?}, error: {:?}",
                            &authority, e
                        );
                        // TODO: Consider on whether to fail out immediately here
                    }
                }
            }

            let transmit_cache = transmit_cache.clone();
            transmit_loop(transmit_request_receiver.clone(), utransport, transmit_cache).await;
        });
    }
}
