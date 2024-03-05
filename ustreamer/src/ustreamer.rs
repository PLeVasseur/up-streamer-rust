use crate::egress_router::{EgressRouter, EgressRouterHandle, EgressRouterStartArgs};
use crate::hashable_items::{HashableUAuthority, HashableUUID};
use crate::ingress_router::{IngressRouter, IngressRouterHandle, IngressRouterStartArgs};
use crate::streamer_router::StreamerRouter;
use crate::ustreamer_error::UStreamerError;
use crate::utransport_router::{
    UTransportRouter, UTransportRouterConfig, UTransportRouterHandle, UTransportRouterStartArgs,
};
use async_std::channel::{self, Receiver, Sender};
use async_std::sync::Mutex;
use lru::LruCache;
use std::collections::HashMap;
use std::error::Error;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Arc;
use up_rust::uprotocol::{UAuthority, UMessage, UUID};

// We use the concept of a TransportTag and not a concrete enum because we want to allow
// extensibility to use beyond the currently written `up-client-foo-rust` and support closed-source
// or vendor-specific implementations
pub type TransportTag = u8;
pub type TransportId = String;

pub struct Route {
    authority: UAuthority,
    transport: TransportTag,
}

pub struct TaggedTransportRouterConfig {
    tag: TransportTag,
    id: TransportId,
    queue_length: usize,
    config: UTransportRouterConfig,
}

pub struct IngressEgressQueueConfig {
    ingress_queue_length: usize,
    egress_queue_length: usize,
}

pub struct BookkeepingConfig {
    transmit_cache_size: usize,
}

pub struct UStreamerConfig {
    transport_router_configs: Vec<TaggedTransportRouterConfig>,
    ingress_egress_queue_config: IngressEgressQueueConfig,
    bookkeeping_config: BookkeepingConfig,
    routes: Vec<Route>,
}

pub struct UStreamer {
    host_transport: Option<TransportTag>,
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
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
    pub fn start(config: UStreamerConfig) -> Result<UStreamer, UStreamerError> {
        let (utransport_senders, utransport_receivers) =
            Self::assemble_utransport_senders_receivers(&config)?;
        let (ingress_sender, ingress_receiver, egress_sender, egress_receiver) =
            Self::assemble_ingress_egress(&config)?;
        let authority_routes = Self::assemble_authority_routes(&config)?;

        let host_transport_tag = Self::find_host_transport(&config)?;
        let host_transport_sender: Option<Sender<UMessage>> = {
            if host_transport_tag.is_some() {
                let sender = utransport_senders.get(&host_transport_tag.unwrap());
                if sender.is_some() {
                    sender.clone().unwrap();
                }
            };
            None
        };

        let transmit_cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(config.bookkeeping_config.transmit_cache_size).unwrap(),
        )));

        let (ingress_handle, egress_handle) = Self::start_ingress_egress_routers(
            &config,
            ingress_receiver.clone(),
            egress_receiver.clone(),
            host_transport_tag,
            host_transport_sender,
            authority_routes.clone(),
        )?;
        let utransport_router_handles = Self::start_utransport_routers(
            config,
            &utransport_receivers,
            host_transport_tag,
            authority_routes.clone(),
            ingress_sender.clone(),
            egress_sender.clone(),
            transmit_cache.clone(),
        )?;

        Ok(Self {
            host_transport: host_transport_tag,
            authority_routes,
            transmit_cache,
            utransport_router_handles,
            utransport_senders,
            utransport_receivers,
            ingress_sender,
            ingress_receiver,
            ingress_handle,
            egress_sender,
            egress_receiver,
            egress_handle,
        })
    }

    pub fn stop(&self) -> Result<(), Box<dyn Error>> {
        // TODO: Implement functionality so that we're able to gracefully wind down the transports
        //  and ingress, egress

        todo!()
    }

    fn find_host_transport(
        config: &UStreamerConfig,
    ) -> Result<Option<TransportTag>, UStreamerError> {
        let mut host_transport = None;
        for transport_router_config in &config.transport_router_configs {
            let is_host_transport = transport_router_config.config.host_transport;

            if host_transport.is_some() && is_host_transport {
                return Err(UStreamerError::GeneralError(
                    "host_transport set true twice".to_string(),
                ));
            }

            if host_transport.is_none() {
                host_transport = Some(transport_router_config.tag);
            }
        }

        return Ok(host_transport);
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

        for transport_router_config in &config.transport_router_configs {
            let (utransport_sender, utransport_receiver) =
                channel::bounded::<UMessage>(transport_router_config.queue_length);
            utransport_senders.insert(transport_router_config.tag, utransport_sender);
            utransport_receivers.insert(transport_router_config.tag, utransport_receiver);
        }

        Ok((utransport_senders, utransport_receivers))
    }

    fn assemble_authority_routes(
        config: &UStreamerConfig,
    ) -> Result<HashMap<HashableUAuthority, TransportTag>, UStreamerError> {
        let mut authority_routes = HashMap::new();

        for route in &config.routes {
            let hashable_uauthority = HashableUAuthority(route.authority.clone());
            if !hashable_uauthority.hashable() {
                return Err(UStreamerError::UAuthorityNotHashable(
                    route.authority.clone(),
                ));
            }

            let previously_inserted =
                authority_routes.insert(hashable_uauthority, route.transport.clone());
            if previously_inserted.is_some() {
                return Err(UStreamerError::DuplicateTransportTag(
                    previously_inserted.unwrap(),
                ));
            }
        }

        Ok(authority_routes)
    }

    fn assemble_ingress_egress(
        config: &UStreamerConfig,
    ) -> Result<
        (
            Sender<UMessage>,
            Receiver<UMessage>,
            Sender<UMessage>,
            Receiver<UMessage>,
        ),
        UStreamerError,
    > {
        let (ingress_sender, ingress_receiver) =
            channel::bounded::<UMessage>(config.ingress_egress_queue_config.ingress_queue_length);
        let (egress_sender, egress_receiver) =
            channel::bounded::<UMessage>(config.ingress_egress_queue_config.egress_queue_length);
        Ok((
            ingress_sender,
            ingress_receiver,
            egress_sender,
            egress_receiver,
        ))
    }

    fn start_utransport_routers(
        config: UStreamerConfig,
        utransport_receivers: &HashMap<TransportTag, Receiver<UMessage>>,
        host_transport_tag: Option<TransportTag>,
        authority_routes: HashMap<HashableUAuthority, TransportTag>,
        ingress_sender: Sender<UMessage>,
        egress_sender: Sender<UMessage>,
        transmit_cache: Arc<Mutex<LruCache<HashableUUID, bool>>>,
    ) -> Result<HashMap<TransportTag, UTransportRouterHandle>, UStreamerError> {
        let mut utransport_router_handles = HashMap::new();

        for transport_router_config in config.transport_router_configs {
            let authorities_to_listen_on = {
                let mut authorities = Vec::new();
                for route in &config.routes {
                    if route.transport == transport_router_config.tag {
                        authorities.push(route.authority.clone());
                    }
                }
                authorities
            };

            let authority_routes = authority_routes.clone();
            let utransport_receiver = utransport_receivers
                .get(&transport_router_config.tag)
                .ok_or(UStreamerError::GeneralError(format!(
                    "Unable to find utransport_receiver for tag: {:?}",
                    &transport_router_config.tag
                )))?;
            let transport_router_start_args = UTransportRouterStartArgs {
                host_transport_tag,
                config: transport_router_config.config,
                authorities: authorities_to_listen_on,
                authority_routes,
                ingress_sender: ingress_sender.clone(),
                egress_sender: egress_sender.clone(),
                transmit_request_receiver: utransport_receiver.clone(),
                transmit_cache: transmit_cache.clone(),
            };

            let handle =
                UTransportRouter::start(&transport_router_config.id, &transport_router_start_args)
                    .expect(&*format!(
                        "Failed to start {} router",
                        &transport_router_config.id
                    ));
            utransport_router_handles.insert(transport_router_config.tag, handle);
        }

        return Ok(utransport_router_handles);
    }

    fn start_ingress_egress_routers(
        _config: &UStreamerConfig,
        ingress_receiver: Receiver<UMessage>,
        egress_receiver: Receiver<UMessage>,
        host_transport_tag: Option<TransportTag>,
        host_transport_sender: Option<Sender<UMessage>>,
        authority_routes: HashMap<HashableUAuthority, TransportTag>,
    ) -> Result<(IngressRouterHandle, EgressRouterHandle), UStreamerError> {
        let ingress_router_start_args = IngressRouterStartArgs {
            ingress_receiver,
            host_transport_tag,
            host_transport_sender,
            authority_routes,
        };
        let ingress_handle = IngressRouter::start("ingress_router", &ingress_router_start_args)
            .expect(&*"Failed to start ingress router".to_string());

        let egress_router_start_args = EgressRouterStartArgs { egress_receiver };
        let egress_handle = EgressRouter::start("egress_router", &egress_router_start_args)
            .expect(&*"Failed to start ingress router".to_string());

        Ok((ingress_handle, egress_handle))
    }
}
