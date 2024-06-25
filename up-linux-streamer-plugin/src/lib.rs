#![recursion_limit = "256"]

use async_std::task;
use futures::select;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::env;
use std::fs::canonicalize;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, Mutex,
};
use std::time::Duration;
use tracing::{debug, error, info, trace};
use zenoh::plugins::{RunningPluginTrait, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_backend_traits::config::PluginConfig;
use zenoh_core::zlock;
use zenoh_plugin_trait::{plugin_long_version, plugin_version, Plugin, PluginControl};
use zenoh_result::{bail, ZResult};

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
        trace!("name: {name}");
        // @CY / Luca -- Doing the same as in the example plugin here leads to a runtime panic
        //  due to config.plugin(name) being None
        let config = runtime.config().lock();
        let self_cfg = config.plugin(name).unwrap().as_object().unwrap();
        trace!("self_cfg: {self_cfg:?}");

        let selector = KeyExpr::try_from(DEFAULT_SELECTOR).unwrap();

        // a flag to end the plugin's loop when the plugin is removed from the config
        let flag = Arc::new(AtomicBool::new(true));
        // spawn the task running the plugin's loop
        trace!("up-linux-streamer-plugin: before spawning run");
        async_std::task::spawn(run(runtime.clone(), selector, flag.clone()));
        trace!("up-linux-streamer-plugin: after spawning run");
        // return a RunningPlugin to zenohd
        trace!("up-linux-streamer-plugin: before creating RunningPlugin");
        let ret = Box::new(RunningPlugin(Arc::new(Mutex::new(RunningPluginInner {
            flag,
            name: name.into(),
            runtime: runtime.clone(),
        }))));

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
                    bail!(
                        "up-linux-streamer-plugin: storage-selector for {} must be a string",
                        &guard.name
                    )
                }
            }
        }
        bail!(
            "up-linux-streamer-plugin: unknown option {} for {}",
            path,
            guard.name
        )
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

    env_logger::init();

    // Plugin's event loop, while the flag is true
    let mut counter = 1;
    while flag.load(Relaxed) {
        // TODO: Need to implement signalling to stop uStreamer

        task::sleep(Duration::from_millis(1000)).await;
        trace!("counter: {counter}");

        counter += 1;
    }
}
