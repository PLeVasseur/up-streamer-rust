use std::cell::RefCell;
use std::{io, thread};
use std::io::ErrorKind;
use std::sync::{
    Arc, Mutex,
};
use async_std::task;
use uprotocol_sdk::uprotocol::{UAuthority, UMessage, UStatus, UUri};
use zenoh::plugins::{Response, RunningPluginTrait, ValidationFunction};
use zenoh::prelude::r#async::*;
use zenoh::runtime::Runtime;
use zenoh_plugin_trait::Plugin;
use zenoh_result::ZResult;
use crate::transport_builders::transport_builder::UTransportBuilder;

pub struct UTransportPlugin {}

pub struct UTransportPluginStartArgs {
    runtime: Runtime,
    transport_builder: RefCell<Option<Box<dyn UTransportBuilder>>>,
    authorities: Vec<UAuthority>,
}

impl Plugin for UTransportPlugin {
    type StartArgs = UTransportPluginStartArgs;
    type RunningPlugin = zenoh::plugins::RunningPlugin;
    const STATIC_NAME: &'static str = "utransport_plugin";

    fn start(name: &str, start_args: &Self::StartArgs) -> ZResult<Self::RunningPlugin> {
        if start_args.transport_builder.borrow_mut().is_none() {
            return Err("Transport is not available".into());
        }

        let transport_builder = start_args.transport_builder.borrow_mut().take().ok_or_else(|| {
            // TODO: Can replace with own custom error
            io::Error::new(ErrorKind::NotFound, "Transport is not available")
        })?;

        async_std::task::spawn(run(start_args.runtime.clone(), transport_builder, start_args.authorities.clone()));
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
    fn config_checker(&self) -> ValidationFunction {
        todo!()
    }

    fn adminspace_getter<'a>(&'a self, selector: &'a Selector<'a>, plugin_status_key: &str) -> ZResult<Vec<Response>> {
        todo!()
    }
}

// If the plugin is dropped, set the flag to false to end the loop
impl Drop for RunningPlugin {
    fn drop(&mut self) {

    }
}

async fn run(runtime: Runtime, transport_builder: Box<dyn UTransportBuilder>, authorities: Vec<UAuthority>) {
    thread::spawn(move || {
        task::block_on(async {
            transport_builder.create_and_setup(authorities).await;
        });
    });
}
