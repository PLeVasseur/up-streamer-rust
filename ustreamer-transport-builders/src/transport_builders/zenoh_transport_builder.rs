use async_std::task;
use up_client_zenoh_rust::UPClientZenoh;
use up_rust::transport::datamodel::UTransport;
use ustreamer::transport_builder::UTransportBuilder;
use zenoh::prelude::Config;

// TODO: Must get moved into another repo, too specific for up-streamer-rust

struct ZenohTransportBuilder {
    config: Option<Config>,
}

impl ZenohTransportBuilder {
    #[allow(dead_code)]
    fn new_with_config(config: Config) -> Self {
        Self {
            config: Some(config),
        }
    }

    #[allow(dead_code)]
    fn new() -> Self {
        Self { config: None }
    }
}

impl UTransportBuilder for ZenohTransportBuilder {
    fn build(&self) -> Box<dyn UTransport> {
        let config = self.config.clone().unwrap_or_else(Config::default);
        let utransport: Box<dyn UTransport> = Box::new(
            task::block_on(UPClientZenoh::new(config)).expect("Unable to create UPClientZenoh!"),
        );
        utransport
    }
}
