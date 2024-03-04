use crate::streamer_plugin::StreamerPlugin;
use crate::utransport_plugin::{
    UTransportPlugin, UTransportPluginHandle, UTransportPluginStartArgs,
};
use async_std::channel::{self, Receiver, Sender};
use std::collections::HashMap;
use std::error::Error;
use std::hash::{Hash, Hasher};
use prost::bytes::BufMut;
use up_rust::uprotocol::UAuthority;

#[derive(Debug)]
enum UStreamerConstructionError {
    DuplicateTransportTag(TransportTag),
    UAuthorityNotHashable(UAuthority)
}

impl std::fmt::Display for UStreamerConstructionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            UStreamerConstructionError::DuplicateTransportTag(transport_tag) => {
                write!(f, "Duplicate transport tag found: {}", transport_tag)
            },
            UStreamerConstructionError::UAuthorityNotHashable(uauthority) => {
                write!(f, "Unable to has UAuthority: {:?}", uauthority)
            }
        }
    }
}

impl std::error::Error for UStreamerConstructionError {}

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
type TransportTag = u8;
type TransportId = String;

struct Route {
    authority: UAuthority,
    transport: TransportTag,
}

struct TaggedTransportPluginStartArgs {
    tag: TransportTag,
    id: TransportId,
    start_args: UTransportPluginStartArgs,
}

struct UStreamerConfig {
    transport_start_args: Vec<TaggedTransportPluginStartArgs>,
    routes: Vec<Route>,
}

#[derive(Default)]
struct UStreamer {
    authority_routes: HashMap<HashableUAuthority, TransportTag>,
    utransport_plugin_handles: HashMap<TransportTag, UTransportPluginHandle>,
    utransport_senders: HashMap<TransportTag, channel::Sender<bool>>,
    utransport_receivers: HashMap<TransportTag, channel::Receiver<bool>>,
}

impl UStreamer {
    pub fn start(config: &UStreamerConfig) -> Result<UStreamer, UStreamerConstructionError> {
        let (utransport_senders, utransport_receivers, utransport_plugin_handles) =
            Self::assemble_senders_receivers_handles(config)?;
        let authority_routes = Self::assemble_authority_routes(config)?;

        Ok(Self {
            authority_routes,
            utransport_plugin_handles,
            utransport_senders,
            utransport_receivers,
        })
    }

    pub fn stop(&self) -> Result<(), Box<dyn Error>> {
        // TODO: Implement functionality so that we're able to gracefully wind down the transports
        //  and ingress, egress

        todo!()
    }

    fn assemble_senders_receivers_handles(
        config: &UStreamerConfig,
    ) -> Result<
        (
            HashMap<TransportTag, Sender<bool>>,
            HashMap<TransportTag, Receiver<bool>>,
            HashMap<TransportTag, UTransportPluginHandle>,
        ),
        UStreamerConstructionError,
    > {
        let mut utransport_senders = HashMap::new();
        let mut utransport_receivers = HashMap::new();
        let mut utransport_plugin_handles = HashMap::new();

        for transport_builder in &config.transport_start_args {
            const UTRANSPORT_QUEUE_CAPACITY: usize = 100;
            let (utransport_sender, utransport_receiver) =
                channel::bounded::<bool>(UTRANSPORT_QUEUE_CAPACITY);
            utransport_senders.insert(transport_builder.tag, utransport_sender);
            utransport_receivers.insert(transport_builder.tag, utransport_receiver);
            let result =
                UTransportPlugin::start(&transport_builder.id, &transport_builder.start_args)
                    .expect(&*format!(
                        "Failed to start {} plugin",
                        &transport_builder.id
                    ));
            utransport_plugin_handles.insert(transport_builder.tag, result);
        }

        Ok((
            utransport_senders,
            utransport_receivers,
            utransport_plugin_handles,
        ))
    }

    fn assemble_authority_routes(
        config: &UStreamerConfig,
    ) -> Result<HashMap<HashableUAuthority, TransportTag>, UStreamerConstructionError> {

        let mut authority_routes = HashMap::new();

        for route in &config.routes {
            let hashable_uauthority = HashableUAuthority(route.authority.clone());
            if !hashable_uauthority.hashable() {
                return Err(UStreamerConstructionError::UAuthorityNotHashable(route.authority.clone()));
            }

            let previously_inserted = authority_routes.insert(hashable_uauthority, route.transport.clone());
            if previously_inserted.is_some() {
                return Err(UStreamerConstructionError::DuplicateTransportTag(previously_inserted.unwrap()));
            }
        }

        Ok(authority_routes)
    }
}
