use crate::hashable_items::{HashableUAuthority, HashableUUID};
use crate::router::Router;
use crate::ustreamer::TransportTag;
use crate::utransport_builder::UTransportBuilder;
use async_std::channel::{Receiver, Sender};
use async_std::sync::Mutex;
use lru::LruCache;
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::io::ErrorKind;
use std::sync::Arc;
use std::{io, thread};
use up_rust::uprotocol::{UAuthority, UMessage};

pub struct UTransportRouter {}

pub struct UTransportRouterConfig {
    transport_builder: RefCell<Option<Box<dyn UTransportBuilder>>>,
    pub(crate) host_transport: bool,
}

pub struct UTransportRouterStartArgs {
    pub(crate) host_transport_tag: Option<TransportTag>,
    pub(crate) config: UTransportRouterConfig,
    pub(crate) authorities: Vec<UAuthority>,
    pub(crate) authority_routes: HashMap<HashableUAuthority, TransportTag>,
    pub(crate) ingress_sender: Sender<UMessage>,
    pub(crate) egress_sender: Sender<UMessage>,
    pub(crate) transmit_request_receiver: Receiver<UMessage>,
    pub(crate) transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
}

pub struct UTransportRouterHandle;

impl Router for UTransportRouter {
    type StartArgs = UTransportRouterStartArgs;
    type Instance = UTransportRouterHandle;

    fn start(name: &str, start_args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>> {
        if start_args.config.transport_builder.borrow_mut().is_none() {
            return Err("Transport is not available".into());
        }

        let transport_builder = start_args
            .config
            .transport_builder
            .borrow_mut()
            .take()
            .ok_or_else(|| {
                // TODO: Can replace with own custom error
                io::Error::new(ErrorKind::NotFound, "Transport is not available")
            })?;

        async_std::task::spawn(run(
            transport_builder,
            start_args.authorities.clone(),
            start_args.config.host_transport.clone(),
            start_args.host_transport_tag.clone(),
            start_args.authority_routes.clone(),
            start_args.ingress_sender.clone(),
            start_args.egress_sender.clone(),
            start_args.transmit_request_receiver.clone(),
            start_args.transmit_cache.clone(),
        ));
        Ok(UTransportRouterHandle {})
    }
}

async fn run(
    transport_builder: Box<dyn UTransportBuilder>,
    authorities: Vec<UAuthority>,
    host_transport: bool,
    host_transport_tag: Option<TransportTag>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    ingress_sender: Sender<UMessage>,
    egress_sender: Sender<UMessage>,
    transmit_request_receiver: Receiver<UMessage>,
    transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
) {
    thread::spawn(move || {
        transport_builder.start(
            authorities,
            host_transport,
            host_transport_tag,
            authority_routes,
            ingress_sender,
            egress_sender,
            transmit_request_receiver,
            transmit_cache,
        );
    });
}
