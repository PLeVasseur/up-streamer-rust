use std::sync::Mutex;
use async_trait::async_trait;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{UAttributes, UEntity, UMessage, UPayload, UStatus, UUri};
use crate::transport_builders::transport_builder::{UTransportBuilder, UTransportBuilderFunction};

// From here to below is all just mocking what a potential UpClientAndroid could look like
// and testing the hypothesis on whether we should be able to
trait Interface: Send + Sync {}

trait IUBus: Interface + Send {}

struct UpClientAndroid {
    ubus: Box<dyn IUBus>
}

impl UpClientAndroid {
    fn new(ubus: Box<dyn IUBus>) -> Self {
        Self {
            ubus
        }
    }
}

#[async_trait]
impl UTransport for UpClientAndroid {
    #[allow(dead_code)]
    async fn authenticate(&self, entity: UEntity) -> Result<(), UStatus> {
        todo!()
    }

    #[allow(dead_code)]
    async fn send(
        &self,
        topic: UUri,
        payload: UPayload,
        attributes: UAttributes,
    ) -> Result<(), UStatus> {
        todo!()
    }

    #[allow(dead_code)]
    async fn register_listener(
        &self,
        topic: UUri,
        listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
    ) -> Result<String, UStatus> {
        todo!()
    }

    #[allow(dead_code)]
    async fn unregister_listener(&self, topic: UUri, listener: &str) -> Result<(), UStatus> {
        todo!()
    }
}

struct AndroidTransportBuilder {
    ubus: Mutex<Option<Box<dyn IUBus>>>
}

impl AndroidTransportBuilder {
    fn obtain_binder_reference(ubus: Box<dyn IUBus>) -> Self {
        Self {
            ubus: Mutex::new(Some(ubus))
        }
    }
}

impl UTransportBuilder for AndroidTransportBuilder {
    fn create_up_client(&self) -> UTransportBuilderFunction {
        Box::new(|| {
            let mut ubus_lock = self.ubus.lock().unwrap();
            if ubus_lock.is_none() {
                // this is OK as it should only happen on initial setup / configuration,
                // not in run-time for the first time on a deployment
                panic!("UBus was not set!");
            }

            let ubus = ubus_lock.take().unwrap();

            let up_client: Box<dyn UTransport> =
                Box::new(UpClientAndroid::new(ubus));
            up_client
        })
    }
}