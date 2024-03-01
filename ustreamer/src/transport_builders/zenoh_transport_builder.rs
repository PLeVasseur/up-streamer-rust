use async_std::task;
use up_rust::transport::datamodel::UTransport;
use up_client_zenoh_rust::UPClientZenoh;
use zenoh::prelude::Config;
use crate::transport_builders::transport_builder::UTransportBuilder;

struct ZenohTransportBuilder {
    config: Option<Config>
}

impl ZenohTransportBuilder {
    fn new_with_config(config: Config) -> Self {
        Self {
            config: Some(config)
        }
    }

    fn new() -> Self {
        Self {
            config: None
        }
    }
}

impl UTransportBuilder for ZenohTransportBuilder {
    fn build(&self) -> Box<dyn UTransport> {
        let config = self.config.clone().unwrap_or_else(Config::default);
        let utransport: Box<dyn UTransport> = Box::new(task::block_on(UPClientZenoh::new(config)).expect("Unable to create UPClientZenoh!"));
        utransport
    }
}