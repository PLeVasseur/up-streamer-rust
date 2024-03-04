use crate::streamer_router::StreamerRouter;
use async_std::channel::Receiver;
use std::error::Error;
use std::thread;
use up_rust::uprotocol::UMessage;

pub struct EgressRouter {}

pub struct EgressRouterStartArgs {
    pub(crate) egress_receiver: Receiver<UMessage>,
}

pub struct EgressRouterHandle;

impl StreamerRouter for EgressRouter {
    type StartArgs = EgressRouterStartArgs;
    type Instance = EgressRouterHandle;

    fn start(name: &str, start_args: &Self::StartArgs) -> Result<Self::Instance, Box<dyn Error>> {
        todo!()
    }
}

async fn run(/* necessary params */) {
    thread::spawn(move || EgressRouterInner::start());
}

struct EgressRouterInner;

impl EgressRouterInner {
    fn start() {
        todo!()
    }
}

async fn handle_egress() {
    // TODO: Implement

    // TODO:
    //  Handle based on UMessageType
    //    1. Publish + no sink UUri [Publish]
    //       Forward onto all other transports than originating one
    //        -> Need a means to know the originating transport here then
    //        -> We can use the source UUri's UAuthority to lookup in uauthority_routes to understand
    //           which transport to not send over
    //      => Must have reference to uauthority_routes
    //      => Must have reference to utransport_senders hashmap
    //   2. Publish + sink UUri [Notification] (same as Request and Response)
    //      Forward onto the transport corresponding to the sink UUri
    //        -> We can use the source UUri's UAuthority to lookup in uauthority_routes to understand
    //           which transport to send over
    //      => Must have reference to uauthority_routes
    //      => Must have reference to utransport_senders hashmap
}
