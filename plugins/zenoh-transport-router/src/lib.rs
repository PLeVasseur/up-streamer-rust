/********************************************************************************
 * Copyright (c) 2023 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

#![recursion_limit = "256"]

use futures::select;
use log::{debug, info};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::{
    atomic::{AtomicBool, Ordering::Relaxed},
    Arc, Mutex,
};
use zenoh::plugins::{Plugin, RunningPluginTrait, ValidationFunction, ZenohPlugin};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_core::zlock;
use zenoh_result::{bail, ZResult};

// The struct implementing the ZenohPlugin and ZenohPlugin traits
pub struct ZenohTransportRouter {}

// declaration of the plugin's VTable for zenohd to find the plugin's functions to be called
zenoh_plugin_trait::declare_plugin!(ZenohTransportRouter);

// A default sniff-route for the zenoh-transport-router to monitor traffic on
// TODO: May be good to have the up-zenoh-* libraries prepend their key expressions with up/ to disambiguate
// const DEFAULT_SNIFF_ROUTE: &str = "up/**";
const DEFAULT_SNIFF_ROUTE: &str = "**";

impl ZenohPlugin for ZenohTransportRouter {}
impl Plugin for ZenohTransportRouter {
    type StartArgs = Runtime;
    type RunningPlugin = zenoh::plugins::RunningPlugin;

    // A mandatory const to define, in case of the plugin is built as a standalone executable
    const STATIC_NAME: &'static str = "zenoh_transport_router";

    // The first operation called by zenohd on the plugin
    fn start(name: &str, runtime: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        let config = runtime.config.lock();
        let self_cfg = config.plugin(name).unwrap().as_object().unwrap();
        // get the plugin's config details from self_cfg Map (here the "storage-selector" property)
        let default_sniff_route: KeyExpr = match self_cfg.get("default-sniff-route") {
            Some(serde_json::Value::String(s)) => KeyExpr::try_from(s)?,
            None => KeyExpr::try_from(DEFAULT_SNIFF_ROUTE).unwrap(),
            _ => {
                bail!("default-sniff-route is a mandatory option for {}", name)
            }
        }
        .clone()
        .into_owned();
        std::mem::drop(config);

        // a flag to end the plugin's loop when the plugin is removed from the config
        let flag = Arc::new(AtomicBool::new(true));
        // spawn the task running the plugin's loop
        async_std::task::spawn(run(runtime.clone(), default_sniff_route, flag.clone()));
        // return a RunningPlugin to zenohd
        Ok(Box::new(RunningPlugin(Arc::new(Mutex::new(
            RunningPluginInner {
                flag,
                name: name.into(),
                runtime: runtime.clone(),
            },
        )))))
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
impl RunningPluginTrait for RunningPlugin {
    // Operation returning a ValidationFunction(path, old, new)-> ZResult<Option<serde_json::Map<String, serde_json::Value>>>
    // this function will be called each time the plugin's config is changed via the zenohd admin space
    fn config_checker(&self) -> ValidationFunction {
        let guard = zlock!(&self.0);
        let name = guard.name.clone();
        std::mem::drop(guard);
        let plugin = self.clone();
        Arc::new(move |path, old, new| {
            const DEFAULT_SNIFF_ROUTE: &str = "default-sniff-route";
            if path == DEFAULT_SNIFF_ROUTE || path.is_empty() {
                match (old.get(DEFAULT_SNIFF_ROUTE), new.get(DEFAULT_SNIFF_ROUTE)) {
                    (Some(serde_json::Value::String(os)), Some(serde_json::Value::String(ns)))
                        if os == ns => {}
                    (_, Some(serde_json::Value::String(selector))) => {
                        let mut guard = zlock!(&plugin.0);
                        guard.flag.store(false, Relaxed);
                        guard.flag = Arc::new(AtomicBool::new(true));
                        match KeyExpr::try_from(selector.clone()) {
                            Err(e) => log::error!("{}", e),
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
                        let guard = zlock!(&plugin.0);
                        guard.flag.store(false, Relaxed);
                    }
                    _ => {
                        bail!("default-sniff-route for {} must be a string", &name)
                    }
                }
            }
            bail!("unknown option {} for {}", path, &name)
        })
    }

    // Function called on any query on admin space that matches this plugin's sub-part of the admin space.
    // Thus the plugin can reply its contribution to the global admin space of this zenohd.
    fn adminspace_getter<'a>(
        &'a self,
        _selector: &'a Selector<'a>,
        _plugin_status_key: &str,
    ) -> ZResult<Vec<zenoh::plugins::Response>> {
        Ok(Vec::new())
    }
}

// If the plugin is dropped, set the flag to false to end the loop
impl Drop for RunningPlugin {
    fn drop(&mut self) {
        zlock!(self.0).flag.store(false, Relaxed);
    }
}

async fn run(runtime: Runtime, sniff_route: KeyExpr<'_>, flag: Arc<AtomicBool>) {
    env_logger::init();

    // create a zenoh Session that shares the same Runtime as zenohd
    let session = zenoh::init(runtime).res().await.unwrap();

    // the HasMap used as a storage by this example of storage plugin
    let mut stored: HashMap<String, Sample> = HashMap::new();

    debug!(
        "Run zenoh-transport-router with sniff-route={}",
        sniff_route
    );

    // This storage plugin subscribes to the sniff_route and will store in HashMap the received samples
    debug!("Create Subscriber on {}", sniff_route);
    let sub = session
        .declare_subscriber(&sniff_route)
        .res()
        .await
        .unwrap();

    // This storage plugin declares a Queryable that will reply to queries with the samples stored in the HashMap
    debug!("Create Queryable on {}", sniff_route);
    let queryable = session.declare_queryable(&sniff_route).res().await.unwrap();

    // Plugin's event loop, while the flag is true
    while flag.load(Relaxed) {
        select!(
            // on sample received by the Subscriber
            sample = sub.recv_async() => {
                let sample = sample.unwrap();
                info!("Received data ('{}': '{}')", sample.key_expr, sample.value);
                stored.insert(sample.key_expr.to_string(), sample);
            },
            // on query received by the Queryable
            query = queryable.recv_async() => {
                let query = query.unwrap();
                info!("Handling query '{}'", query.selector());
                for (key_expr, sample) in stored.iter() {
                    if query.selector().key_expr.intersects(unsafe{keyexpr::from_str_unchecked(key_expr)}) {
                        query.reply(Ok(sample.clone())).res().await.unwrap();
                    }
                }
            }
        );
    }
}
