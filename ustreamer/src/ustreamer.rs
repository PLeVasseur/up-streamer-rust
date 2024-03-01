use async_std::channel::{self};
use std::collections::HashMap;
use up_rust::uprotocol::UAuthority;
use crate::plugins::utransport_plugin::{UTransportPlugin, UTransportPluginStartArgs};

#[derive(Debug)]
enum UStreamerConstructionError {
    DuplicateTransportTag,
}

impl std::fmt::Display for UStreamerConstructionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            UStreamerConstructionError::DuplicateTransportTag => write!(f, "Duplicate transport tags found"),
        }
    }
}

impl std::error::Error for UStreamerConstructionError {}

// We use the concept of a TransportTag and not a concrete enum because we want to allow
// extensibility to use beyond the currently written `up-client-foo-rust` and support closed-source
// or vendor-specific implementations
type TransportTag = u8;

struct Route {
    authority: UAuthority,
    transport: TransportTag
}

struct RoutingTable {
    routes: Vec<Route>
}

struct TaggedTransportPluginStartArgs(TransportTag, UTransportPluginStartArgs);

struct UStreamerConfig {
    host_transport: TaggedTransportPluginStartArgs,
    external_transports: Vec<TaggedTransportPluginStartArgs>,
    routing_table: RoutingTable
}

#[derive(Default)]
struct UStreamer {
    authority_routes: HashMap<UAuthority, TransportTag>,
    transport_builders: Vec<TaggedTransportPluginStartArgs>,
    utransport_senders: HashMap<TransportTag, channel::Sender<bool>>,
    utransport_receivers: HashMap<TransportTag, channel::Receiver<bool>>,
}

impl UStreamer {
    pub fn start(config: &UStreamerConfig) -> Result<UStreamer, UStreamerConstructionError> {
        // TODO: Perform validation here that we have a unique pairing between all `TaggedTransport`s
        //  and fail if we do not

        // TODO: Build authority_routes

        let transport_builders = &config.external_transports;
        let mut utransport_senders = HashMap::new();
        let mut utransport_receivers = HashMap::new();

        for transport_builder in transport_builders {
            const UTRANSPORT_QUEUE_CAPACITY: usize = 100;
            let (utransport_sender, utransport_receiver) =
                channel::bounded::<bool>(UTRANSPORT_QUEUE_CAPACITY);
            utransport_senders.insert(transport_builder.0, utransport_sender);
            utransport_receivers.insert(transport_builder.0, utransport_receiver);
            {
                use zenoh_plugin_trait::Plugin;
                UTransportPlugin::start("todo", &transport_builder.1)
                    .expect("Failed to start todo plugin");
            }

        }

        Ok(Self {
            authority_routes: Default::default(),
            transport_builders: Default::default(),
            utransport_senders,
            utransport_receivers,
        })
    }

    fn assemble_authority_routes(&self, config: &UStreamerConfig) -> Result<(), UStreamerConstructionError> {
        Ok(())
    }
}