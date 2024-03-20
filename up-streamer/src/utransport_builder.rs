use up_rust::UTransport;

pub trait UTransportBuilder: Send + Sync {
    fn build(&self) -> Box<dyn UTransport>;
}
