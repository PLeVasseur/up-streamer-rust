mod plugins;

// use egress_router::{EgressRouter, EgressRouterStartArgs};
// use ingress_router::{IngressRouter, IngressRouterStartArgs};

use crate::plugins::egress_router::{EgressRouter, EgressRouterStartArgs};
use crate::plugins::ingress_router::{IngressRouter, IngressRouterStartArgs};

use async_std::channel::{self, Receiver, Sender};
use log::{debug, error, info, trace, warn};
use std::sync::Arc;
use uprotocol_rust_transport_mqtt::UTransportMqtt;
use uprotocol_rust_transport_sommr::UTransportSommr;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{Remote, UAuthority, UMessage};
use uprotocol_zenoh_rust::ULinkZenoh;
use zenoh::scouting::WhatAmI;

#[async_std::main]
async fn main() {
    env_logger::try_init().unwrap_or_default();

    println!("Starting uStreamer!");

    // TODO: Add configuration of local UAuthority
    let ustreamer_device_ip: Vec<u8> = vec![192, 168, 3, 100];
    let ustreamer_device_authority: UAuthority = UAuthority {
        remote: Some(Remote::Ip(ustreamer_device_ip)),
    };

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

    // TODO: Add ability to configure this
    const INGRESS_QUEUE_CAPACITY: usize = 5;
    let (ingress_queue_sender, ingress_queue_receiver) =
        channel::bounded::<UMessage>(INGRESS_QUEUE_CAPACITY);

    // TODO: Add ability to configure this
    const EGRESS_QUEUE_CAPACITY: usize = 5;
    let (egress_queue_sender, egress_queue_receiver) =
        channel::bounded::<UMessage>(EGRESS_QUEUE_CAPACITY);

    let ingress_queue_start_args = IngressRouterStartArgs {
        runtime: runtime.clone(),
        udevice_authority: ustreamer_device_authority.clone(),
        ingress_queue_sender: ingress_queue_sender.clone(),
        ingress_queue_receiver: ingress_queue_receiver.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        transports: up_clients.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        IngressRouter::start("ingress_router", &ingress_queue_start_args)
            .expect("Failed to start IngressRouter");
    }

    trace!("uStreamer: started IngressRouter");

    let egress_queue_start_args = EgressRouterStartArgs {
        runtime: runtime.clone(),
        egress_queue_sender: egress_queue_sender.clone(),
        egress_queue_receiver: egress_queue_receiver.clone(),
    };

    {
        use zenoh_plugin_trait::Plugin;
        EgressRouter::start("egress_router", &egress_queue_start_args).unwrap();
    }

    trace!("uStreamer: started EgressRouter");

    async_std::future::pending::<()>().await;
}
