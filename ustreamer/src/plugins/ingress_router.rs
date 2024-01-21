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
pub struct IngressRouter {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
zenoh_plugin_trait::declare_plugin!(IngressRouter);

pub struct IngressRouterStartArgs {
    pub runtime: Runtime,
    // egress_queue_sender: Sender<QueueEntry>,
    pub ingress_queue_sender: Sender<UMessage>,
    pub ingress_queue_receiver: Receiver<UMessage>,
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
        let ingress_queue_sender_clone = start_args.ingress_queue_sender.clone();
        let ingress_queue_receiver_clone = start_args.ingress_queue_receiver.clone();
        async_std::task::spawn(run(
            transports_clone,
            ingress_queue_sender_clone,
            ingress_queue_receiver_clone,
        ));

        let transports_plugin_clone = start_args.transports.clone();
        // let ingress_queue_sender_plugin_clone = start_args.ingress_queue_sender.clone();
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                runtime: start_args.runtime.clone(),
                transports: transports_plugin_clone,
            },
        )))))
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    runtime: Runtime,
    transports: Vec<Arc<dyn UTransport>>,
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

async fn ingress_queue_consumer(mut receiver: Receiver<UMessage>) {
    while let Ok(msg) = receiver.recv().await {
        trace!("Received msg: {:?}", msg);

        // TODO: Add dispatching logic to point internally

        // TODO: if Publish, then...
        //  => Need to consider how to get ahold of our up_client_zenoh as a UTransport
        //     so that we can call send() on it

        // TODO: if Request, then...
        //  => Need to consider how to get ahold of our up_client_zenoh as an RpcClient
        //     so that we can call invoke_method() on it

        // TODO: if Response, then...
        //  => Need to consider how to get ahold of our raw Zenoh session
        //     so that we can look up the Zenoh Query to reply back on
    }
}

fn transport_listener(result: Result<UMessage, UStatus>, sender: Sender<UMessage>) {
    match result {
        Ok(message) => {
            let payload = match message.payload {
                Some(ref payload) => payload,
                None => {
                    error!("No payload attached!");
                    return;
                }
            };

            let data = match &payload.data {
                Some(data) => data,
                None => {
                    error!("Empty data payload!");
                    return;
                }
            };

            trace!("Message source: {:?}", message.source);

            // TODO: Add logic here on _when_ to insert into the channel
            //  if UMessage.UAttributes.sink.UAuthority is Remote and Remote matches us,
            //  then send to the ingress_queue
            let sender_clone = sender.clone();
            task::spawn(async move {
                sender_clone.send(message).await.unwrap();
            });
        }
        Err(status) => {
            error!(
                "transport_listener returned UStatus: {:?} msg: {}",
                status.get_code(),
                status.message()
            );
        }
    }
}

async fn run(
    transports: Vec<Arc<dyn UTransport>>,
    ingress_queue_sender: Sender<UMessage>,
    ingress_queue_receiver: Receiver<UMessage>,
) {
    env_logger::init();

    let consumer_task = task::spawn(ingress_queue_consumer(ingress_queue_receiver));

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
        let transport_clone = transport.clone();
        let uuri_for_all_remote_clone = uuri_for_all_remote.clone();
        let ingress_queue_sender_clone = ingress_queue_sender.clone();
        task::spawn(async move {
            let listener_closure = move |result: Result<UMessage, UStatus>| {
                transport_listener(result, ingress_queue_sender_clone.clone());
            };

            // You might normally keep track of the registered listener's key so you can remove it later with unregister_listener
            let _registered_minute_timer_key = {
                match transport_clone
                    // .register_listener(uuri_for_all_remote_clone, Box::new(transport_listener))
                    .register_listener(uuri_for_all_remote_clone, Box::new(listener_closure))
                    .await
                {
                    Ok(registered_key) => registered_key,
                    Err(status) => {
                        error!("Failed to register listener: {:?}", status.get_code());
                        return;
                    }
                }
            };
        });
    }
}
