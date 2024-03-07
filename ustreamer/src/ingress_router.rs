#![allow(clippy::mutable_key_type)]
// TODO: Consider again, if we want to upstream the ability to hash and compare UAuthority & UUri
use crate::hashable_items::HashableUAuthority;
use crate::router::Router;
use crate::ustreamer::TransportTag;
use async_std::channel::{Receiver, Sender};
use async_std::task;
use log::{error, warn};
use std::collections::HashMap;
use std::error::Error;
use std::thread;
use up_rust::uprotocol::{UMessage, UUri};

const TAG: &str = "IngressRouter";

pub struct IngressRouter {}

pub struct IngressRouterStartArgs {
    pub(crate) ingress_receiver: Receiver<UMessage>,
    pub(crate) host_transport_tag: Option<TransportTag>,
    pub(crate) host_transport_sender: Option<Sender<UMessage>>,
    pub(crate) authority_routes: HashMap<HashableUAuthority, TransportTag>,
}

pub struct IngressRouterHandle;

impl Router for IngressRouter {
    type StartArgs = IngressRouterStartArgs;
    type Instance = IngressRouterHandle;

    fn start(_name: &str, start_args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>> {
        task::spawn(run(
            start_args.ingress_receiver.clone(),
            start_args.host_transport_tag,
            start_args.host_transport_sender.clone(),
            start_args.authority_routes.clone(),
        ));

        Ok(IngressRouterHandle)
    }
}
async fn run(
    ingress_receiver: Receiver<UMessage>,
    host_transport_tag: Option<TransportTag>,
    host_transport_sender: Option<Sender<UMessage>>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
) {
    thread::spawn(move || {
        IngressRouterInner::start(
            ingress_receiver,
            host_transport_tag,
            host_transport_sender,
            authority_routes,
        );
    });
}

struct IngressRouterInner;

impl IngressRouterInner {
    fn start(
        ingress_receiver: Receiver<UMessage>,
        host_transport_tag: Option<TransportTag>,
        host_transport_sender: Option<Sender<UMessage>>,
        authority_routes: HashMap<HashableUAuthority, TransportTag>,
    ) {
        task::spawn_local(handle_ingress(
            ingress_receiver,
            host_transport_tag,
            host_transport_sender,
            authority_routes,
        ));
    }
}

async fn handle_ingress(
    ingress_receiver: Receiver<UMessage>,
    host_transport_tag: Option<TransportTag>,
    host_transport_sender: Option<Sender<UMessage>>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
) {
    // Loop over and consume from ingress queue
    while let Ok(message) = ingress_receiver.recv().await {
        if host_transport_tag.is_none() || host_transport_sender.is_none() {
            warn!("{TAG}: UMessage cannot be directed to host, as host is not defined");
            continue;
        }

        // Confirm that the message is intended for host device
        let UMessage { attributes, .. } = &message;
        let attr = attributes.as_ref();
        let Some(attr) = attr else {
            error!("{TAG}: UMessage doesn't have UAttributes filled out");
            continue;
        };
        let Some(UUri { authority, .. }) = &attr.sink.as_ref() else {
            error!("{TAG}: No sink");
            continue;
        };
        let Some(auth) = authority.as_ref() else {
            error!("{TAG}: UUri doesn't have UAuthority filled out");
            continue;
        };
        match authority_routes.get(&HashableUAuthority(auth.clone())) {
            None => {
                error!("{TAG}: UMessage received with no match to known route");
                continue;
            }
            Some(transport_tag) => {
                if *transport_tag != host_transport_tag.unwrap() {
                    error!("{TAG}: UMessage intended for host doesn't match host_transport_tag");
                    continue;
                }

                if let Some(sender) = host_transport_sender.as_ref() {
                    if let Err(e) = sender.send(message.clone()).await {
                        error!(
                            "{TAG}: Unable to send to host transport transmit queue: {:?}",
                            e
                        );
                    }
                }
            }
        }
    }
}
