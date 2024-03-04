use std::error::Error;
use async_std::channel::Receiver;
use up_rust::uprotocol::UMessage;
use crate::streamer_router::StreamerRouter;

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