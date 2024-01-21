#![recursion_limit = "256"]

use async_std::channel::{self, Receiver, Sender};
use async_std::task;
use futures::select;
use log::{debug, error, info, trace};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, Mutex,
};
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{Remote, UAuthority, UEntity, UMessage, UStatus, UUri};
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_core::zlock;
use zenoh_result::{bail, ZResult};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct EgressRouter {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
// zenoh_plugin_trait::declare_plugin!(EgressRouter);

pub struct EgressRouterStartArgs {
    pub runtime: Runtime,
    pub egress_queue_sender: Sender<UMessage>,
    pub egress_queue_receiver: Receiver<UMessage>,
    // pub transports: Vec<Arc<dyn UTransport>>,
}

impl Plugin for EgressRouter {
    type StartArgs = EgressRouterStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "egress_router";

    // The first operation called by zenohd on the plugin
    fn start(name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        let egress_queue_sender_clone = start_args.egress_queue_sender.clone();
        let egress_queue_receiver_clone = start_args.egress_queue_receiver.clone();
        async_std::task::spawn(run(
            egress_queue_sender_clone,
            egress_queue_receiver_clone,
        ));

        // let ingress_queue_sender_plugin_clone = start_args.ingress_queue_sender.clone();
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                runtime: start_args.runtime.clone(),
            },
        )))))
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    runtime: Runtime,
}
// The RunningPlugin struct implementing the RunningPluginTrait trait
#[derive(Clone)]
struct RunningPlugin(Arc<Mutex<RunningPluginInner>>);
impl RunningPluginTrait for RunningPlugin {
    // Operation returning a ValidationFunction(path, old, new)-> ZResult<Option<serde_json::Map<String, serde_json::Value>>>
    // this function will be called each time the plugin's config is changed via the zenohd admin space
    fn config_checker(&self) -> ValidationFunction {
        todo!()
    }

    // Function called on any query on admin space that matches this plugin's sub-part of the admin space.
    // Thus the plugin can reply its contribution to the global admin space of this zenohd.
    fn adminspace_getter<'a>(
        &'a self,
        _selector: &'a Selector<'a>,
        _plugin_status_key: &str,
    ) -> ZResult<Vec<zenoh::plugins::Response>> {
        todo!()
    }
}

async fn egress_queue_consumer(mut receiver: Receiver<UMessage>) {
    while let Ok(msg) = receiver.recv().await {
        trace!("Egress Queue: Received msg: {:?}", msg);

        // TODO: Add dispatching logic to point externally
    }
}

async fn run(
    egress_queue_sender: Sender<UMessage>,
    egress_queue_receiver: Receiver<UMessage>,
) {
    let _ = env_logger::try_init();

    let consumer_task = task::spawn(egress_queue_consumer(egress_queue_receiver));
}
