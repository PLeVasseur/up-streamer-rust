mod plugins;

use crate::plugins::egress_router::EgressRouter;
use crate::plugins::ingress_router::{IngressRouter, IngressRouterStartArgs};

use std::sync::Arc;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::scouting::WhatAmI;

#[async_std::main]
async fn main() {
    println!("Starting uStreamer!");

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");

    let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();

    let mut up_client_zenoh_config = zenoh::config::Config::default();
    up_client_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");

    let up_client_zenoh = Arc::new(ULinkZenoh::new_from_config(up_client_zenoh_config).await.unwrap());

    let up_clients = vec![up_client_zenoh];

    let up_clients: Vec<Arc<dyn UTransport>> = up_clients
        .into_iter()
        .map(|client| client as Arc<dyn UTransport>)
        .collect();

    let start_args = IngressRouterStartArgs{runtime: runtime, transports: up_clients.clone() };

    use zenoh_plugin_trait::Plugin;
    IngressRouter::start("ingress_router", &start_args)
        .unwrap();

    async_std::future::pending::<()>().await;
}
