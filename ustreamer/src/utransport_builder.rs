use up_rust::transport::datamodel::UTransport;

const TAG: &str = "UTransportBuilder";

pub trait UTransportBuilder: Send + Sync {
    fn build(&self) -> Box<dyn UTransport>;
}
