use crate::streamer_router::StreamerRouter;
use crate::utransport_router::{
    UTransportRouter, UTransportRouterHandle, UTransportRouterStartArgs,
};
use crate::ustreamer_error::UStreamerError;
use async_std::channel::{self, Receiver, Sender};
use std::collections::HashMap;
use std::error::Error;
use std::hash::{Hash, Hasher};
use prost::bytes::BufMut;
use up_rust::uprotocol::{UAuthority, UMessage};
use crate::egress_router::{EgressRouter, EgressRouterHandle, EgressRouterStartArgs};
use crate::ingress_router::{IngressRouter, IngressRouterHandle, IngressRouterStartArgs};


struct HashableUAuthority(UAuthority);

impl PartialEq for HashableUAuthority {
    fn eq(&self, other: &HashableUAuthority) -> bool {
        self.0 == other.0
    }
}

impl Eq for HashableUAuthority {}

impl Hash for HashableUAuthority {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut bytes = Vec::new();
        if self.0.has_id() {
            bytes.put_u8(self.0.id().len() as u8);
            bytes.put(self.0.id());
        } else if self.0.has_ip() {
            bytes.put(self.0.ip());
        } else {
            // Should never happen, call hashable first!
            bytes.put_u8(42);
        }

        bytes.hash(state)
    }
}
impl HashableUAuthority {
    fn hashable(&self) -> bool {
        if self.0.has_id() || self.0.has_ip() {
            return true;
        }
        return false;
    }
}

// We use the concept of a TransportTag and not a concrete enum because we want to allow
// extensibility to use beyond the currently written `up-client-foo-rust` and support closed-source
// or vendor-specific implementations
pub type TransportTag = u8;
type TransportId = String;

struct Route {
    authority: UAuthority,
    transport: TransportTag,
}

struct TaggedTransportRouterStartArgs {
    tag: TransportTag,
    id: TransportId,
    queue_length: usize,
    start_args: UTransportRouterStartArgs,
}

struct IngressEgressQueueConfig {
    ingress_queue_length: usize,
    egress_queue_length: usize,
}

struct UStreamerConfig {
    transport_start_args: Vec<TaggedTransportRouterStartArgs>,
    ingress_egress_queue_config: IngressEgressQueueConfig,
    routes: Vec<Route>,
}

struct UStreamer {
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    utransport_router_handles: HashMap<TransportTag, UTransportRouterHandle>,
    utransport_senders: HashMap<TransportTag, Sender<UMessage>>,
    utransport_receivers: HashMap<TransportTag, Receiver<UMessage>>,
    ingress_sender: Sender<UMessage>,
    ingress_receiver: Receiver<UMessage>,
    ingress_handle: IngressRouterHandle,
    egress_sender: Sender<UMessage>,
    egress_receiver: Receiver<UMessage>,
    egress_handle: EgressRouterHandle,
}

impl UStreamer {
    pub fn start(config: &UStreamerConfig) -> Result<UStreamer, UStreamerError> {
        let (utransport_senders, utransport_receivers) =
            Self::assemble_utransport_senders_receivers(config)?;
        let (ingress_sender, ingress_receiver, egress_sender, egress_receiver) = Self::assemble_ingress_egress(config)?;
        let authority_routes = Self::assemble_authority_routes(config)?;

        let utransport_router_handles = Self::start_utransport_routers(config)?;
        let (ingress_handle, egress_handle) =  Self::start_ingress_egress_routers(config, ingress_receiver.clone(), egress_receiver.clone())?;

        Ok(Self {
            authority_routes,
            utransport_router_handles,
            utransport_senders,
            utransport_receivers,
            ingress_sender,
            ingress_receiver,
            ingress_handle,
            egress_sender,
            egress_receiver,
            egress_handle
        })
    }

    pub fn stop(&self) -> Result<(), Box<dyn Error>> {
        // TODO: Implement functionality so that we're able to gracefully wind down the transports
        //  and ingress, egress

        todo!()
    }

    fn assemble_utransport_senders_receivers(
        config: &UStreamerConfig,
    ) -> Result<
        (
            HashMap<TransportTag, Sender<UMessage>>,
            HashMap<TransportTag, Receiver<UMessage>>,
        ),
        UStreamerError,
    > {
        let mut utransport_senders = HashMap::new();
        let mut utransport_receivers = HashMap::new();

        for transport_start_args in &config.transport_start_args {
            let (utransport_sender, utransport_receiver) =
                channel::bounded::<UMessage>(transport_start_args.queue_length);
            utransport_senders.insert(transport_start_args.tag, utransport_sender);
            utransport_receivers.insert(transport_start_args.tag, utransport_receiver);
        }

        Ok((
            utransport_senders,
            utransport_receivers,
        ))
    }

    fn assemble_authority_routes(
        config: &UStreamerConfig,
    ) -> Result<HashMap<HashableUAuthority, TransportTag>, UStreamerError> {

        let mut authority_routes = HashMap::new();

        for route in &config.routes {
            let hashable_uauthority = HashableUAuthority(route.authority.clone());
            if !hashable_uauthority.hashable() {
                return Err(UStreamerError::UAuthorityNotHashable(route.authority.clone()));
            }

            let previously_inserted = authority_routes.insert(hashable_uauthority, route.transport.clone());
            if previously_inserted.is_some() {
                return Err(UStreamerError::DuplicateTransportTag(previously_inserted.unwrap()));
            }
        }

        Ok(authority_routes)
    }

    fn assemble_ingress_egress(
        config: &UStreamerConfig
    ) -> Result<(Sender<UMessage>, Receiver<UMessage>, Sender<UMessage>, Receiver<UMessage>), UStreamerError> {
        let (ingress_sender, ingress_receiver) =
            channel::bounded::<UMessage>(config.ingress_egress_queue_config.ingress_queue_length);
        let (egress_sender, egress_receiver) =
            channel::bounded::<UMessage>(config.ingress_egress_queue_config.egress_queue_length);
        Ok((ingress_sender, ingress_receiver, egress_sender, egress_receiver))
    }

    fn start_utransport_routers(
        config: &UStreamerConfig
    ) -> Result<HashMap<TransportTag, UTransportRouterHandle>, UStreamerError> {
        let mut utransport_router_handles = HashMap::new();

        for transport_start_args in &config.transport_start_args {
            let handle =
                UTransportRouter::start(&transport_start_args.id, &transport_start_args.start_args)
                    .expect(&*format!(
                        "Failed to start {} router",
                        &transport_start_args.id
                    ));
            utransport_router_handles.insert(transport_start_args.tag, handle);
        }

        return Ok(utransport_router_handles)
    }

    fn start_ingress_egress_routers(
        config: &UStreamerConfig,
        ingress_receiver: Receiver<UMessage>,
        egress_receiver: Receiver<UMessage>,
    ) -> Result<(IngressRouterHandle, EgressRouterHandle), UStreamerError> {
        let ingress_router_start_args = IngressRouterStartArgs {
            ingress_receiver,
        };
        let ingress_handle = IngressRouter::start("ingress_router", &ingress_router_start_args)
            .expect(&*"Failed to start ingress router".to_string());

        let egress_router_start_args = EgressRouterStartArgs {
            egress_receiver,
        };
        let egress_handle = EgressRouter::start("egress_router", &egress_router_start_args)
            .expect(&*"Failed to start ingress router".to_string());

        Ok((ingress_handle, egress_handle))
    }
}
