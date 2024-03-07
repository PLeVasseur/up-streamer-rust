use up_rust::transport::datamodel::UTransport;

pub trait UTransportBuilder: Send + Sync {
    fn build(&self) -> Box<dyn UTransport>;
}
