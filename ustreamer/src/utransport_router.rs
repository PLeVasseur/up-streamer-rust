use crate::streamer_router::StreamerRouter;
use crate::transport_builder::UTransportBuilder;
use std::cell::RefCell;
use std::error::Error;
use std::io::ErrorKind;
use std::{io, thread};
use up_rust::uprotocol::UAuthority;

pub struct UTransportRouter {}

pub struct UTransportRouterStartArgs {
    transport_builder: RefCell<Option<Box<dyn UTransportBuilder>>>,
    host_transport: bool,
    authorities: Vec<UAuthority>,
}

pub struct UTransportRouterHandle;

impl StreamerRouter for UTransportRouter {
    type StartArgs = UTransportRouterStartArgs;
    type Instance = UTransportRouterHandle;

    fn start(name: &str, start_args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>> {
        if start_args.transport_builder.borrow_mut().is_none() {
            return Err("Transport is not available".into());
        }

        let transport_builder = start_args
            .transport_builder
            .borrow_mut()
            .take()
            .ok_or_else(|| {
                // TODO: Can replace with own custom error
                io::Error::new(ErrorKind::NotFound, "Transport is not available")
            })?;

        async_std::task::spawn(run(transport_builder, start_args.authorities.clone()));
        Ok(UTransportRouterHandle {})
    }
}

async fn run(transport_builder: Box<dyn UTransportBuilder>, authorities: Vec<UAuthority>) {
    thread::spawn(move || {
        transport_builder.start(authorities);
    });
}
