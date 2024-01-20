#![recursion_limit = "256"]

use crate::plugins::types::QueueEntry;
use async_std::channel::{self, Sender, Receiver};
use async_std::task;
use futures::select;
use log::{debug, info};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, Mutex,
};
use uprotocol_sdk::uprotocol::{UMessage, UStatus, UUri};
use uprotocol_sdk::transport::datamodel::UTransport;
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_core::zlock;
use zenoh_result::{bail, ZResult};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct IngressRouter {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
zenoh_plugin_trait::declare_plugin!(IngressRouter);

pub struct IngressRouterStartArgs {
    runtime: Runtime,
    egress_queue_sender: Sender<QueueEntry>,
    ingress_queue_sender: Sender<QueueEntry>,
    ingress_queue_receiver: Receiver<QueueEntry>,
    transports: Vec<Arc<Mutex<dyn UTransport>>>
}

// impl ZenohPlugin for IngressRouter {}
impl Plugin for IngressRouter {
    type StartArgs = IngressRouterStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "example";

    // The first operation called by zenohd on the plugin
    fn start(name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {

        for transport in start_args.transports {
            let uuri = UUri{
                authority: Default::default(),
                entity: Default::default(),
                resource: Default::default()
            };
            let callback = move |result: Result<UMessage, UStatus>| {

            };
            task::spawn(async move {
                transport.lock().unwrap().register_listener(uuri, Box::new(callback));
            });
        }

        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                runtime: start_args.runtime.clone(),
            },
        )))))

        // spawn the task running the plugin's loop
        // async_std::task::spawn(run(&start_args.transports));
        // async_std::task::spawn(run(start_args.runtime.clone(), start_args.transports));
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

async fn run(transports: &Vec<Arc<Mutex<dyn UTransport>>>) {
// async fn run(runtime: Runtime, transports: &Vec<Arc<Mutex<dyn UTransport>>>) {
    env_logger::init();

}