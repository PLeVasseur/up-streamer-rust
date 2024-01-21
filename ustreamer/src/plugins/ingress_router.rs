#![recursion_limit = "256"]

use crate::plugins::types::QueueEntry;
use async_std::channel::{self, Receiver, Sender};
use async_std::task;
use futures::select;
use log::{debug, info};
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
pub struct IngressRouter {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
zenoh_plugin_trait::declare_plugin!(IngressRouter);

pub struct IngressRouterStartArgs {
    pub runtime: Runtime,
    // egress_queue_sender: Sender<QueueEntry>,
    // ingress_queue_sender: Sender<QueueEntry>,
    // ingress_queue_receiver: Receiver<QueueEntry>,
    pub transports: Vec<Arc<dyn UTransport>>,
}

// impl ZenohPlugin for IngressRouter {}
impl Plugin for IngressRouter {
    type StartArgs = IngressRouterStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "ingress_router";

    // The first operation called by zenohd on the plugin
    fn start(name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        let transports_clone = start_args.transports.clone();
        async_std::task::spawn(run(transports_clone));

        let transports_plugin_clone = start_args.transports.clone();
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                runtime: start_args.runtime.clone(),
                transports: transports_plugin_clone
            },
        )))))
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    runtime: Runtime,
    transports: Vec<Arc<dyn UTransport>>
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

async fn run(transports: Vec<Arc<dyn UTransport>>) {
    env_logger::init();

    let uuri_for_all_remote = UUri {
        authority: Some(UAuthority {
            remote: Some(Remote::Name("*".to_string())),
        }),
        entity: Some(UEntity {
            name: "*".to_string(),
            id: None,
            version_major: None,
            version_minor: None,
        }),
        resource: None,
    };
    for transport in &transports {

        let callback = move |result: Result<UMessage, UStatus>| {
            let Ok(msg) = result else {
                println!("Unable to retrieve src");
                return;
            };

            println!("Message source: {:?}", msg.source);
        };
        let transport_clone = transport.clone();
        let uuri_for_all_remote_clone = uuri_for_all_remote.clone();
        task::spawn(async move {
            // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
            let _registered_minute_timer_key = {
                match transport_clone
                    .register_listener(uuri_for_all_remote_clone, Box::new(callback))
                    .await
                {
                    Ok(registered_key) => registered_key,
                    Err(status) => {
                        println!(
                            "Failed to register timer_minute listener: {:?}",
                            status.get_code()
                        );
                        return;
                    }
                }
            };
        });
    }
}
