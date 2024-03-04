use crate::hashable_items::HashableUUID;
use async_std::channel::{Receiver, SendError, Sender};
use async_std::sync::Mutex;
use async_std::task;
use async_trait::async_trait;
use log::{error, trace};
use lru::LruCache;
use std::sync::Arc;
use up_rust::transport::datamodel::UTransport;
use up_rust::uprotocol::{UAuthority, UMessage, UStatus, UUri};

const TAG: &str = "UTransportBuilder";

async fn transport_listener(
    result: Result<UMessage, UStatus>,
    host_transport: bool,
    ingress_sender: Sender<UMessage>,
    egress_sender: Sender<UMessage>,
    transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
) {
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
    if host_transport {
        match ingress_sender.send(message).await {
            Ok(_) => {}
            Err(e) => {
                error!("{TAG}: Unable to send message to ingress queue: {:?}", e);
            }
        }
    } else {
        match egress_sender.send(message).await {
            Ok(_) => {}
            Err(e) => {
                error!("{TAG}: Unable to send message to egress queue: {:?}", e);
            }
        }
    }
}

async fn transmit_loop(
    transmit_request_receiver: Receiver<UMessage>,
    utransport: Box<dyn UTransport>,
) {
    // Loop over and consume from transmit_queue
    while let Ok(mut message) = transmit_request_receiver.recv().await {
        // Send out over transport
        match utransport.send(message).await {
            Ok(_) => {}
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
                let ingress_sender = ingress_sender.clone();
                let egress_sender = egress_sender.clone();
                let transmit_cache = transmit_cache.clone();
                let closure_listener = move |result: Result<UMessage, UStatus>| {
                    task::spawn_local(transport_listener(
                        result,
                        host_transport.clone(),
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

            transmit_loop(transmit_request_receiver.clone(), utransport).await;
        });
    }
}
