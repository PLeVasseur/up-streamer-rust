#![recursion_limit = "256"]

use futures::select;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::env;
use std::fs::canonicalize;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, Mutex,
};
use std::time::Duration;
use tracing::{debug, info};
use zenoh::plugins::{RunningPluginTrait, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_core::zlock;
use zenoh_plugin_trait::{plugin_long_version, plugin_version, Plugin, PluginControl};
use zenoh_result::{bail, ZResult};
use tracing::{error, trace};
use async_std::task;
use up_rust::UTransport;
use up_transport_vsomeip::UPTransportVsomeip;
use up_transport_zenoh::UPClientZenoh;
use up_streamer::{Endpoint, UStreamer};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct ExamplePlugin {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
#[cfg(feature = "dynamic_plugin")]
zenoh_plugin_trait::declare_plugin!(ExamplePlugin);

// A default selector for this example of storage plugin (in case the config doesn't set it)
// This plugin will subscribe to this selector and declare a queryable with this selector
const DEFAULT_SELECTOR: &str = "demo/example/**";

impl ZenohPlugin for ExamplePlugin {}
impl Plugin for ExamplePlugin {
    type StartArgs = Runtime;
    type Instance = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const DEFAULT_NAME: &'static str = "up_linux_streamer";
    const PLUGIN_VERSION: &'static str = plugin_version!();
    const PLUGIN_LONG_VERSION: &'static str = plugin_long_version!();

    // The first operation called by zenohd on the plugin
    fn start(name: &str, runtime: &Self::StartArgs) -> ZResult<Self::Instance> {
        zenoh_util::try_init_log_from_env();
        trace!("up-linux-streamer-plugin: start");
        // let config = runtime.config().lock();
        // trace!("config: {config:?}");
        // let maybe_config = config.plugin(name);
        // trace!("maybe_config: {maybe_config:?}");
        // let selector = if let Some(config) = maybe_config {
        //     let maybe_config_object = config.as_object();
        //     if let Some(config_object) = maybe_config_object {
        //         // let self_cfg = config.plugin(name).unwrap().as_object().unwrap();
        //         // get the plugin's config details from self_cfg Map (here the "storage-selector" property)
        //         trace!("up-linux-streamer-plugin: got self_cfg");
        //         let selector: KeyExpr = match config_object.get("storage-selector") {
        //             Some(serde_json::Value::String(s)) => KeyExpr::try_from(s)?,
        //             None => KeyExpr::try_from(DEFAULT_SELECTOR).unwrap(),
        //             _ => {
        //                 bail!("storage-selector is a mandatory option for {}", name)
        //             }
        //         }
        //             .clone()
        //             .into_owned();
        //         trace!("up-linux-streamer-plugin: after selector set");
        //         trace!("up-linux-streamer-plugin: after dropping std::mem::drop(config)");
        //         selector
        //     } else {
        //         KeyExpr::try_from(DEFAULT_SELECTOR).unwrap()
        //     }
        // } else {
        //     KeyExpr::try_from(DEFAULT_SELECTOR).unwrap()
        // };

        let selector = KeyExpr::try_from(DEFAULT_SELECTOR).unwrap();

        // a flag to end the plugin's loop when the plugin is removed from the config
        let flag = Arc::new(AtomicBool::new(true));
        // spawn the task running the plugin's loop
        trace!("up-linux-streamer-plugin: before spawning run");
        async_std::task::spawn(run(runtime.clone(), selector, flag.clone()));
        trace!("up-linux-streamer-plugin: after spawning run");
        // return a RunningPlugin to zenohd
        trace!("up-linux-streamer-plugin: before creating RunningPlugin");
        let ret = Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                flag,
                name: name.into(),
                runtime: runtime.clone(),
            },
        ))));

        trace!("up-linux-streamer-plugin: after creating RunningPlugin");

        Ok(ret)
    }
}

// An inner-state for the RunningPlugin
struct RunningPluginInner {
    flag: Arc<AtomicBool>,
    name: String,
    runtime: Runtime,
}
// The RunningPlugin struct implementing the RunningPluginTrait trait
#[derive(Clone)]
struct RunningPlugin(Arc<Mutex<RunningPluginInner>>);

impl PluginControl for RunningPlugin {}

impl RunningPluginTrait for RunningPlugin {
    fn config_checker(
        &self,
        path: &str,
        old: &serde_json::Map<String, serde_json::Value>,
        new: &serde_json::Map<String, serde_json::Value>,
    ) -> ZResult<Option<serde_json::Map<String, serde_json::Value>>> {
        let mut guard = zlock!(&self.0);
        const STORAGE_SELECTOR: &str = "storage-selector";
        if path == STORAGE_SELECTOR || path.is_empty() {
            match (old.get(STORAGE_SELECTOR), new.get(STORAGE_SELECTOR)) {
                (Some(serde_json::Value::String(os)), Some(serde_json::Value::String(ns)))
                if os == ns => {}
                (_, Some(serde_json::Value::String(selector))) => {
                    guard.flag.store(false, Relaxed);
                    guard.flag = Arc::new(AtomicBool::new(true));
                    match KeyExpr::try_from(selector.clone()) {
                        Err(e) => tracing::error!("{}", e),
                        Ok(selector) => {
                            async_std::task::spawn(run(
                                guard.runtime.clone(),
                                selector,
                                guard.flag.clone(),
                            ));
                        }
                    }
                    return Ok(None);
                }
                (_, None) => {
                    guard.flag.store(false, Relaxed);
                }
                _ => {
                    bail!("up-linux-streamer-plugin: storage-selector for {} must be a string", &guard.name)
                }
            }
        }
        bail!("up-linux-streamer-plugin: unknown option {} for {}", path, guard.name)
    }
}

// If the plugin is dropped, set the flag to false to end the loop
impl Drop for RunningPlugin {
    fn drop(&mut self) {
        zlock!(self.0).flag.store(false, Relaxed);
    }
}

async fn run(runtime: Runtime, selector: KeyExpr<'_>, flag: Arc<AtomicBool>) {
    trace!("up-linux-streamer-plugin: inside of run");
    zenoh_util::try_init_log_from_env();
    trace!("up-linux-streamer-plugin: after try_init_log_from_env()");

    trace!("attempt to call something on the runtime");
    let timestamp_res = runtime.new_timestamp();
    trace!("called function on runtime: {timestamp_res:?}");

    // // create a zenoh Session that shares the same Runtime than zenohd
    // TODO: For some reason we crash out here... should ask @CY
    let session_res = zenoh::init(runtime.clone()).res().await;
    if let Err(err) = session_res {
        // TODO: Just the act of passing in the runtime causes the core dump
        //  so we cannot see any kind of error here
        error!("Unable to initialize session from passed in runtime: {err:?}");
    }
    trace!("up-linux-streamer-plugin: after initiating session");

    // // the HasMap used as a storage by this example of storage plugin
    // let mut stored: HashMap<String, Sample> = HashMap::new();
    //
    // debug!("up-linux-streamer-plugin: Run example-plugin with storage-selector={}", selector);
    //
    // // This storage plugin subscribes to the selector and will store in HashMap the received samples
    // debug!("up-linux-streamer-plugin: Create Subscriber on {}", selector);
    // let sub = session.declare_subscriber(&selector).res().await.unwrap();
    //
    // // This storage plugin declares a Queryable that will reply to queries with the samples stored in the HashMap
    // debug!("up-linux-streamer-plugin: Create Queryable on {}", selector);
    // let queryable = session.declare_queryable(&selector).res().await.unwrap();

    env_logger::init();

    let mut streamer = UStreamer::new("up-linux-streamer", 10000);

    let exe_path = match env::current_exe() {
        Ok(exe_path) => {
            if let Some(exe_dir) = exe_path.parent() {
                println!("The binary is located in: {}", exe_dir.display());
                exe_path
            } else {
                panic!("Failed to determine the directory of the executable.");
            }
        }
        Err(e) => {
            panic!("Failed to get the executable path: {}", e);
        }
    };
    tracing::log::trace!("exe_path: {exe_path:?}");
    let exe_path_parent = exe_path.parent();
    let Some(exe_path_parent) = exe_path_parent else {
        panic!("Unable to get parent path");
    };
    tracing::log::trace!("exe_path_parent: {exe_path_parent:?}");

    // let crate_dir = env!("CARGO_MANIFEST_DIR");
    // // TODO: Make configurable to pass the path to the vsomeip config as a command line argument
    let vsomeip_config = PathBuf::from(exe_path_parent).join("vsomeip-configs/point_to_point.json");
    tracing::log::trace!("vsomeip_config: {vsomeip_config:?}");
    let vsomeip_config = canonicalize(vsomeip_config).ok();
    tracing::log::trace!("vsomeip_config: {vsomeip_config:?}");

    // There will be a single vsomeip_transport, as there is a connection into device and a streamer
    // TODO: Add error handling if we fail to create a UPTransportVsomeip
    let vsomeip_transport: Arc<dyn UTransport> = Arc::new(
        UPTransportVsomeip::new_with_config(&"linux".to_string(), 10, &vsomeip_config.unwrap())
            .unwrap(),
    );

    // TODO: Probably make somewhat configurable?
    let zenoh_config = Config::default();
    // TODO: Add error handling if we fail to create a UPClientZenoh
    let zenoh_transport: Arc<dyn UTransport> = Arc::new(
        UPClientZenoh::new_with_runtime(runtime.clone(), "linux".to_string())
            .await
            .unwrap(),
    );
    //     UPClientZenoh::new(zenoh_config, "linux".to_string())
    //         .await
    //         .unwrap(),
    // );
    // TODO: Make configurable to pass the name of the mE authority as a  command line argument
    let vsomeip_endpoint = Endpoint::new(
        "vsomeip_endpoint",
        "me_authority",
        vsomeip_transport.clone(),
    );

    // TODO: Make configurable the ability to have perhaps a config file we pass in that has all the
    //  relevant authorities over Zenoh that should be forwarded
    let zenoh_transport_endpoint_a = Endpoint::new(
        "zenoh_transport_endpoint_a",
        "linux", // simple initial case of streamer + intended high compute destination on same device
        zenoh_transport.clone(),
    );

    // TODO: Per Zenoh endpoint configured, run these two rules
    let forwarding_res = streamer
        .add_forwarding_rule(vsomeip_endpoint.clone(), zenoh_transport_endpoint_a.clone())
        .await;

    if let Err(err) = forwarding_res {
        error!("Unable to add forwarding result: {err:?}");
    }

    let forwarding_res =streamer
        .add_forwarding_rule(zenoh_transport_endpoint_a.clone(), vsomeip_endpoint.clone())
        .await;

    if let Err(err) = forwarding_res {
        error!("Unable to add forwarding result: {err:?}");
    }

    // Plugin's event loop, while the flag is true
    let mut counter = 1;
    while flag.load(Relaxed) {

        // TODO: Need to implement signalling to stop uStreamer

        task::sleep(Duration::from_millis(1000)).await;
        trace!("counter: {counter}");

        // select!(
        //     // on sample received by the Subscriber
        //     sample = sub.recv_async() => {
        //         let sample = sample.unwrap();
        //         info!("up-linux-streamer-plugin: Received data ('{}': '{}')", sample.key_expr, sample.value);
        //         stored.insert(sample.key_expr.to_string(), sample);
        //     },
        //     // on query received by the Queryable
        //     query = queryable.recv_async() => {
        //         let query = query.unwrap();
        //         info!("up-linux-streamer-plugin: Handling query '{}'", query.selector());
        //         for (key_expr, sample) in stored.iter() {
        //             if query.selector().key_expr.intersects(unsafe{keyexpr::from_str_unchecked(key_expr)}) {
        //                 query.reply(Ok(sample.clone())).res().await.unwrap();
        //             }
        //         }
        //     }
        // );

        counter += 1;
    }
}