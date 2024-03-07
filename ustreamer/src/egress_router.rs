#![allow(clippy::mutable_key_type)]
// TODO: Consider again, if we want to upstream the ability to hash and compare UAuthority & UUri
use crate::hashable_items::HashableUAuthority;
use crate::router::Router;
use crate::ustreamer::TransportTag;
use async_std::channel::{Receiver, Sender};
use async_std::task;
use log::{error, trace};
use std::collections::HashMap;
use std::error::Error;
use std::thread;
use up_rust::uprotocol::{UAttributes, UMessage, UMessageType};

const TAG: &str = "EgressRouter";

pub struct EgressRouter {}

pub struct EgressRouterStartArgs {
    pub(crate) egress_receiver: Receiver<UMessage>,
    pub(crate) authority_routes: HashMap<HashableUAuthority, TransportTag>,
    pub(crate) utransport_senders: HashMap<TransportTag, Sender<UMessage>>,
}

pub struct EgressRouterHandle;

impl Router for EgressRouter {
    type StartArgs = EgressRouterStartArgs;
    type Instance = EgressRouterHandle;

    fn start(_name: &str, start_args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>> {
        task::spawn(run(
            start_args.egress_receiver.clone(),
            start_args.authority_routes.clone(),
            start_args.utransport_senders.clone(),
        ));

        Ok(EgressRouterHandle)
    }
}

async fn run(
    egress_receiver: Receiver<UMessage>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    utransport_senders: HashMap<TransportTag, Sender<UMessage>>,
) {
    thread::spawn(move || {
        EgressRouterInner::start(egress_receiver, authority_routes, utransport_senders)
    });
}

struct EgressRouterInner;

impl EgressRouterInner {
    fn start(
        egress_receiver: Receiver<UMessage>,
        authority_routes: HashMap<HashableUAuthority, TransportTag>,
        utransport_senders: HashMap<TransportTag, Sender<UMessage>>,
    ) {
        task::spawn_local(handle_egress(
            egress_receiver,
            authority_routes,
            utransport_senders,
        ));
    }
}

async fn handle_egress(
    egress_receiver: Receiver<UMessage>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    utransport_senders: HashMap<TransportTag, Sender<UMessage>>,
) {
    // Loop over and consume from egress queue
    while let Ok(mut message) = egress_receiver.recv().await {
        let UMessage { attributes, .. } = &message;
        let attr = attributes.clone();
        let Some(attr) = attr.as_ref() else {
            error!("{TAG}: Received a UMessage without UAttributes");
            continue;
        };
        let Ok(message_type) = attr.type_.enum_value() else {
            error!("{TAG}: No message type supplied");
            continue;
        };

        match message_type {
            UMessageType::UMESSAGE_TYPE_UNSPECIFIED => {
                error!("{TAG}: UMessageType is UMESSAGE_TYPE_UNSPECIFIED. Cannot route.");
                continue;
            }
            UMessageType::UMESSAGE_TYPE_PUBLISH => {
                if attr.sink.is_some() {
                    send_on_one_transport(
                        &authority_routes,
                        utransport_senders.clone(),
                        message,
                        attr,
                    )
                    .await;
                } else {
                    send_over_all_but_originating(
                        &authority_routes,
                        &utransport_senders,
                        &mut message,
                        attr,
                    )
                    .await;
                }
            }
            UMessageType::UMESSAGE_TYPE_REQUEST => {
                send_on_one_transport(&authority_routes, utransport_senders.clone(), message, attr)
                    .await;
            }
            UMessageType::UMESSAGE_TYPE_RESPONSE => {
                send_on_one_transport(&authority_routes, utransport_senders.clone(), message, attr)
                    .await;
            }
        }
    }
}

async fn send_over_all_but_originating(
    authority_routes: &HashMap<HashableUAuthority, TransportTag>,
    utransport_senders: &HashMap<TransportTag, Sender<UMessage>>,
    message: &mut UMessage,
    attr: &UAttributes,
) -> bool {
    let Some(authority) = attr.source.authority.as_ref() else {
        error!("{TAG}: UAuthority missing from UUri. Unable to route.");
        return true;
    };

    let transport_tag = authority_routes.get(&HashableUAuthority(authority.clone()));
    if transport_tag.is_none() {
        error!("{TAG}: Route not setup for UAuthority: {:?}", &authority);
        return true;
    }

    let exclude_transport_tag = transport_tag.unwrap();
    let senders_except_for_origin: Vec<&Sender<UMessage>> = utransport_senders
        .iter()
        .filter_map(|(key, value)| {
            if key != exclude_transport_tag {
                Some(value)
            } else {
                None
            }
        })
        .collect();

    for sender in senders_except_for_origin {
        match sender.send(message.clone()).await {
            Ok(_) => {
                // TODO: Need to specify which transport sent over / failed on
                trace!("{TAG}: Successfully sent message to transmit queue <FOO>");
            }
            Err(e) => {
                // TODO: Need to specify which transport sent over / failed on
                error!("{TAG}: Failed to send message to transmit queue for transport <FOO>: {e}");
            }
        }
    }
    false
}

async fn send_on_one_transport(
    authority_routes: &HashMap<HashableUAuthority, TransportTag>,
    utransport_senders: HashMap<TransportTag, Sender<UMessage>>,
    message: UMessage,
    attr: &UAttributes,
) {
    let Some(authority) = attr.sink.authority.as_ref() else {
        error!("{TAG}: UAuthority missing from UUri. Unable to route.");
        return;
    };

    let transport_tag = authority_routes.get(&HashableUAuthority(authority.clone()));
    if transport_tag.is_none() {
        error!("{TAG}: Route not setup for UAuthority: {:?}", &authority);
        return;
    }

    let utransport_sender = utransport_senders.get(transport_tag.unwrap());
    if utransport_sender.is_none() {
        error!(
            "{TAG}: No transport configured for this UAuthority: {:?}",
            &authority
        );
        return;
    }

    if let Some(sender) = utransport_sender {
        match sender.send(message).await {
            Ok(_) => {
                trace!("{TAG}: Successfully sent message to transmit_queue");
            }
            Err(e) => {
                error!(
                    "{TAG}: Unable to send message over to transmit_queue: {:?}",
                    e
                );
            }
        }
    }
}
