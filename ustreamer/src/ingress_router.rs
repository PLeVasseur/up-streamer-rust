use crate::streamer_router::StreamerRouter;
use async_std::channel::Receiver;
use std::error::Error;
use std::thread;
use up_rust::uprotocol::UMessage;

pub struct IngressRouter {}

pub struct IngressRouterStartArgs {
    pub(crate) ingress_receiver: Receiver<UMessage>,
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
    thread::spawn(move || {
        IngressRouterInner::start()
    });
}

struct IngressRouterInner;

impl IngressRouterInner {
    fn start() {
        todo!()
    }
}

async fn handle_ingress() {
    // TODO: Implement

    // TODO:
    //   1. Confirm that the message is intended for host device
    //      => Lookup in authority_routes and cross-reference against host_transport
    //      => Must have reference to authority_routes and host_transport
    //   2. We know it's for the host, so we forward it onto the host transport
    //      => Must have reference to the utransport_sender for host transport
}