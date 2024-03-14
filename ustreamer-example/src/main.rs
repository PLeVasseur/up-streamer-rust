mod someipstanding_transport_builder;
mod zenoh_transport_builder;

use crate::someipstanding_transport_builder::SomeipstandinTransportBuilder;
use crate::zenoh_transport_builder::ZenohTransportBuilder;
use std::string::ToString;
use ustreamer::config::{
    BookkeepingConfig, IngressEgressQueueConfig, TaggedTransportRouterConfig, UStreamerConfig,
};
use ustreamer::transport_router::UTransportRouterConfig;
use ustreamer::ustreamer::UStreamer;
use ustreamer::utransport_builder::UTransportBuilder;

const ZENOH_TRANSPORT_TAG: u8 = 0;
const ZENOH_TRANSPORT_ID: &str = "Zenoh";

const SOMEIPSTANDIN_TRANSPORT_TAG: u8 = 1;
const SOMEIPSTANDIN_TRANSPORT_ID: &str = "Someipstandin";

fn main() {
    let bookkeeping_config = BookkeepingConfig::new(100);
    let ingress_egress_queue_config = IngressEgressQueueConfig::new(100, 100);

    let zenoh_transport_builder: Box<dyn UTransportBuilder> =
        Box::new(ZenohTransportBuilder::new());
    let someipstandin_transport_builder: Box<dyn UTransportBuilder> =
        Box::new(SomeipstandinTransportBuilder::new());

    let zenoh_router_config = UTransportRouterConfig::new(zenoh_transport_builder, true).unwrap();
    let zenoh_tagged_transport_router_config = TaggedTransportRouterConfig::new(
        ZENOH_TRANSPORT_TAG,
        ZENOH_TRANSPORT_ID.to_string(),
        100,
        zenoh_router_config,
    )
    .unwrap();
    let someipstandin_router_config =
        UTransportRouterConfig::new(someipstandin_transport_builder, false).unwrap();
    let someipstandin_tagged_transport_router_config = TaggedTransportRouterConfig::new(
        SOMEIPSTANDIN_TRANSPORT_TAG,
        SOMEIPSTANDIN_TRANSPORT_ID.to_string(),
        100,
        someipstandin_router_config,
    )
    .unwrap();
    let transport_router_configs = vec![
        zenoh_tagged_transport_router_config,
        someipstandin_tagged_transport_router_config,
    ];
    let config = UStreamerConfig::new(
        transport_router_configs,
        ingress_egress_queue_config.unwrap(),
        bookkeeping_config.unwrap(),
        vec![],
    );
    let _ustreamer = UStreamer::start(config.unwrap()).unwrap();
}
