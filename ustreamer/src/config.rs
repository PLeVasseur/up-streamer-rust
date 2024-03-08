use crate::errors::UStreamerError;
use crate::transport_router::UTransportRouterConfig;
use up_rust::uprotocol::UAuthority;

// We use the concept of a TransportTag and not a concrete enum because we want to allow
// extensibility to use beyond the currently written `up-client-foo-rust` and support closed-source
// or vendor-specific implementations
pub type TransportTag = u8;
pub type TransportId = String;

pub struct Route {
    pub(crate) authority: UAuthority,
    pub(crate) transport: TransportTag,
}

pub struct TaggedTransportRouterConfig {
    pub(crate) tag: TransportTag,
    pub(crate) id: TransportId,
    pub(crate) queue_length: usize,
    pub(crate) config: UTransportRouterConfig,
}

pub struct IngressEgressQueueConfig {
    pub(crate) ingress_queue_length: usize,
    pub(crate) egress_queue_length: usize,
}

impl IngressEgressQueueConfig {
    pub fn new(
        ingress_queue_length: usize,
        egress_queue_length: usize,
    ) -> Result<Self, UStreamerError> {
        if ingress_queue_length < 1 || egress_queue_length < 1 {
            return Err(UStreamerError::GeneralError(
                "Must have queue lengths > 0".to_string(),
            ));
        }

        Ok(Self {
            ingress_queue_length,
            egress_queue_length,
        })
    }
}

pub struct BookkeepingConfig {
    pub(crate) transmit_cache_size: usize,
}

impl BookkeepingConfig {
    pub fn new(transmit_cache_size: usize) -> Result<Self, UStreamerError> {
        Ok(Self {
            transmit_cache_size,
        })
    }
}

pub struct UStreamerConfig {
    pub(crate) transport_router_configs: Vec<TaggedTransportRouterConfig>,
    pub(crate) ingress_egress_queue_config: IngressEgressQueueConfig,
    pub(crate) bookkeeping_config: BookkeepingConfig,
    pub(crate) routes: Vec<Route>,
}

impl UStreamerConfig {
    pub fn new(
        transport_router_configs: Vec<TaggedTransportRouterConfig>,
        ingress_egress_queue_config: IngressEgressQueueConfig,
        bookkeeping_config: BookkeepingConfig,
        routes: Vec<Route>,
    ) -> Result<Self, UStreamerError> {
        if transport_router_configs.is_empty() {
            return Err(UStreamerError::GeneralError(
                "No transport router configs provided".to_string(),
            ));
        }

        if routes.is_empty() {
            return Err(UStreamerError::GeneralError(
                "No routes provided".to_string(),
            ));
        }

        Ok(Self {
            transport_router_configs,
            ingress_egress_queue_config,
            bookkeeping_config,
            routes,
        })
    }
}
