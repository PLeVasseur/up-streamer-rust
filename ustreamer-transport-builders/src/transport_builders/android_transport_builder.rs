use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use up_rust::transport::datamodel::UTransport;
use up_rust::uprotocol::{UMessage, UStatus, UUri};
use ustreamer::transport_builder::UTransportBuilder;

// From here to below is all just mocking what a potential UpClientAndroid could look like
// and testing the hypothesis on whether we should be able to bring a Box / Strong of dyn IUBus
trait Interface: Send + Sync {}

trait IUBus: Interface + Send {}

struct UpClientAndroid {
    #[allow(dead_code)]
    ubus: Box<dyn IUBus>,
}

impl UpClientAndroid {
    fn new(ubus: Box<dyn IUBus>) -> Self {
        Self { ubus }
    }
}

#[async_trait]
impl UTransport for UpClientAndroid {
    #[allow(dead_code)]
    async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
        todo!()
    }

    #[allow(dead_code)]
    async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
        todo!()
    }

    #[allow(dead_code)]
    async fn register_listener(
        &self,
        _topic: UUri,
        _listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
    ) -> Result<String, UStatus> {
        todo!()
    }

    #[allow(dead_code)]
    async fn unregister_listener(&self, _topic: UUri, _listener: &str) -> Result<(), UStatus> {
        todo!()
    }
}

struct AndroidTransportBuilder {
    ubus: Arc<Mutex<Option<Box<dyn IUBus>>>>,
}

impl AndroidTransportBuilder {
    #[allow(dead_code)]
    fn obtain_binder_reference(ubus: Box<dyn IUBus>) -> Self {
        Self {
            ubus: Arc::new(Mutex::new(Some(ubus))),
        }
    }
}

impl UTransportBuilder for AndroidTransportBuilder {
    fn build(&self) -> Box<dyn UTransport> {
        let ubus_clone = Arc::clone(&self.ubus);
        let mut ubus_lock = ubus_clone.lock().unwrap();
        if ubus_lock.is_none() {
            panic!("UBus was not set!");
        }

        let ubus = ubus_lock.take().unwrap();
        let up_client: Box<dyn UTransport> = Box::new(UpClientAndroid::new(ubus));
        up_client
    }
}
