use crate::utransport_router::UTransportRouterHandle;
use std::sync::Arc;
use up_rust::UAuthority;

#[derive(Clone)]
pub struct Route {
    authority: UAuthority,
    transport_router_handle: Arc<UTransportRouterHandle>,
}

impl Route {
    pub fn new(
        authority: &UAuthority,
        transport_router_handle: &Arc<UTransportRouterHandle>,
    ) -> Self {
        Self {
            authority: authority.clone(),
            transport_router_handle: transport_router_handle.clone(),
        }
    }

    pub fn get_authority(&self) -> UAuthority {
        self.authority.clone()
    }

    pub fn get_transport_router_handle(&self) -> Arc<UTransportRouterHandle> {
        self.transport_router_handle.clone()
    }
}
