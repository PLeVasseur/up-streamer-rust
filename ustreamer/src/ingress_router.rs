use std::collections::HashMap;
use crate::streamer_router::StreamerRouter;
use async_std::channel::{Receiver, Sender, SendError};
use std::error::Error;
use std::thread;
use log::{error, info, warn};
use up_rust::uprotocol::{UMessage, UUri};
use crate::hashable_items::HashableUAuthority;
use crate::ustreamer::TransportTag;

const TAG: &str = "IngressRouter";

pub struct IngressRouter {}

pub struct IngressRouterStartArgs {
    pub(crate) ingress_receiver: Receiver<UMessage>,
    pub(crate) host_transport_tag: Option<TransportTag>,
    pub(crate) host_transport_sender: Option<Sender<UMessage>>,
    pub(crate) authority_routes: HashMap<HashableUAuthority, TransportTag>,
}

pub struct IngressRouterHandle;

impl StreamerRouter for IngressRouter {
    type StartArgs = IngressRouterStartArgs;
    type Instance = IngressRouterHandle;

    fn start(name: &str, start_args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>> {
        todo!()
    }
}
async fn run(/* necessary params */) {
    thread::spawn(move || IngressRouterInner::start());
}

struct IngressRouterInner;

impl IngressRouterInner {
    fn start() {
        todo!()
    }
}

async fn handle_ingress(
    ingress_receiver: Receiver<UMessage>,
    host_transport_tag: Option<TransportTag>,
    host_transport_sender: Option<Sender<UMessage>>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
) {
    // Loop over and consume from transmit_queue
    while let Ok(mut message) = ingress_receiver.recv().await {
        if host_transport_tag.is_none() || host_transport_sender.is_none() {
            warn!("{TAG}: UMessage cannot be directed to host, as host is not defined");
            continue;
        }

        // Confirm that the message is intended for host device
        //  => Lookup in authority_routes and cross-reference against host_transport
        //  => Must have reference to authority_routes and host_transport
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
                        error!("{TAG}: Unable to send to host transport transmit queue: {:?}", e);
                    }
                }
            }
        }
    }


    //   2. We know it's for the host, so we forward it onto the host transport
    //      => Must have reference to the utransport_sender for host transport
}
