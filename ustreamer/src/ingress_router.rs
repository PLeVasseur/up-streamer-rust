use std::error::Error;
use async_std::channel::Receiver;
use up_rust::uprotocol::UMessage;
use crate::streamer_router::StreamerRouter;

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