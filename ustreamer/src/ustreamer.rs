use crate::streamer_plugin::StreamerPlugin;
use crate::utransport_plugin::{
    UTransportPlugin, UTransportPluginHandle, UTransportPluginStartArgs,
};
use async_std::channel::{self};
use std::collections::HashMap;
use std::error::Error;
use up_rust::uprotocol::UAuthority;

#[derive(Debug)]
enum UStreamerConstructionError {
    DuplicateTransportTag,
}

impl std::fmt::Display for UStreamerConstructionError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match *self {
            UStreamerConstructionError::DuplicateTransportTag => {
                write!(f, "Duplicate transport tags found")
            }
        }
    }
}

impl std::error::Error for UStreamerConstructionError {}

// We use the concept of a TransportTag and not a concrete enum because we want to allow
// extensibility to use beyond the currently written `up-client-foo-rust` and support closed-source
// or vendor-specific implementations
type TransportTag = u8;
type TransportId = String;

struct Route {
    authority: UAuthority,
    transport: TransportTag,
}

struct RoutingTable {
    routes: Vec<Route>,
}

struct TaggedTransportPluginStartArgs {
    tag: TransportTag,
    id: TransportId,
    start_args: UTransportPluginStartArgs,
}

struct UStreamerConfig {
    transport_start_args: Vec<TaggedTransportPluginStartArgs>,
    routing_table: RoutingTable,
}

#[derive(Default)]
struct UStreamer {
    authority_routes: HashMap<UAuthority, TransportTag>,
    utransport_plugin_handles: HashMap<TransportTag, UTransportPluginHandle>,
    utransport_senders: HashMap<TransportTag, channel::Sender<bool>>,
    utransport_receivers: HashMap<TransportTag, channel::Receiver<bool>>,
}

impl UStreamer {
    pub fn start(config: &UStreamerConfig) -> Result<UStreamer, UStreamerConstructionError> {
        let transport_builders = &config.transport_start_args;
        let mut utransport_senders = HashMap::new();
        let mut utransport_receivers = HashMap::new();

        let mut utransport_plugin_handles = HashMap::new();

        let authority_routes = Self::assemble_authority_routes(config)?;

        for transport_builder in transport_builders {
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

        Ok(Self {
            authority_routes,
            utransport_plugin_handles,
            utransport_senders,
            utransport_receivers,
        })
    }

    pub fn stop(&self) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn assemble_authority_routes(
        config: &UStreamerConfig,
    ) -> Result<HashMap<UAuthority, TransportTag>, UStreamerConstructionError> {
        // TODO: Implement

        // TODO: Perform validation here that we have a unique pairing between all `TaggedTransport`s
        //  and fail if we do not

        todo!()
    }
}
