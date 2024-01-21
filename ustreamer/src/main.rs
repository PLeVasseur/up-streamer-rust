mod plugins;

use crate::plugins::egress_router::EgressRouter;
use crate::plugins::ingress_router::{IngressRouter, IngressRouterStartArgs};

use std::sync::Arc;
use uprotocol_rust_transport_mqtt::UTransportMqtt;
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::scouting::WhatAmI;

#[async_std::main]
async fn main() {
    println!("Starting uStreamer!");

    // TODO: Add configuration of local UAuthority

    let mut config = zenoh::config::Config::default();
    config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");

    let runtime = zenoh::runtime::Runtime::new(config).await.unwrap();

    // TODO: Add configuration of which up-clients to start
    let mut up_client_zenoh_config = zenoh::config::Config::default();
    up_client_zenoh_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");
    let up_client_zenoh = Arc::new(
        ULinkZenoh::new_from_config(up_client_zenoh_config)
            .await
            .unwrap(),
    );

    let mut up_client_sommr_config = zenoh::config::Config::default();
    up_client_sommr_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");
    let up_client_sommr = Arc::new(
        UTransportSommr::new_from_config(up_client_sommr_config)
            .await
            .unwrap(),
    );

    let mut up_client_mqtt_config = zenoh::config::Config::default();
    up_client_mqtt_config
        .set_mode(Some(WhatAmI::Peer))
        .expect("Unable to configure as Peer");
    let up_client_mqtt = Arc::new(
        UTransportMqtt::new_from_config(up_client_mqtt_config)
            .await
            .unwrap(),
    );

    let up_client_zenoh_transport: Arc<dyn UTransport> =
        up_client_zenoh.clone() as Arc<dyn UTransport>;
    let up_client_sommr_transport: Arc<dyn UTransport> =
        up_client_sommr.clone() as Arc<dyn UTransport>;
    let up_client_mqtt_transport: Arc<dyn UTransport> =
        up_client_mqtt.clone() as Arc<dyn UTransport>;

    let up_clients: Vec<Arc<dyn UTransport>> = vec![
        up_client_zenoh_transport,
        up_client_sommr_transport,
        up_client_mqtt_transport,
    ];

    let start_args = IngressRouterStartArgs {
        runtime: runtime,
        transports: up_clients.clone(),
    };

    use zenoh_plugin_trait::Plugin;
    IngressRouter::start("ingress_router", &start_args).unwrap();

    async_std::future::pending::<()>().await;
}
