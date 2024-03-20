use crate::utransport_builder::UTransportBuilder;
use async_std::channel::{bounded, Sender};
use async_std::task;
use async_trait::async_trait;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{mpsc, Arc};
use std::thread;
use up_rust::{UAuthority, UMessage, UStatus, UTransport, UUIDBuilder, UUri, UUID};

#[derive(Clone)]
pub(crate) struct SenderWrapper<T> {
    id: UUID,
    sender: Arc<Sender<T>>,
}

impl<T> SenderWrapper<T> {
    pub fn new(sender: Sender<T>) -> Self {
        let id = UUIDBuilder::new().build();
        let sender = Arc::new(sender);
        Self { id, sender }
    }
}

impl<T> Hash for SenderWrapper<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> PartialEq for SenderWrapper<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for SenderWrapper<T> {}

pub enum UTransportRouterCommand {
    Register,
    Unregister,
}

pub struct UTransportRouter {}

impl UTransportRouter {
    pub fn new<T>(utransport_builder: T) -> Result<UTransportRouterHandle, UStatus>
    where
        T: UTransportBuilder + 'static,
    {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            task::block_on(async {
                let result = UTransportRouterInner::new(utransport_builder).await;
                tx.send(result).unwrap();
            });
        });
        rx.recv().unwrap()
    }
}

struct UTransportRouterInner {
    utransport: Box<dyn UTransport>,
    listener_map: HashMap<(UAuthority, SenderWrapper<UMessage>), String>,
}

impl UTransportRouterInner {
    pub async fn new<T>(utransport_builder: T) -> Result<UTransportRouterHandle, UStatus>
    where
        T: UTransportBuilder,
    {
        let utransport = utransport_builder.build(); // TODO: May want to allow this to fail
        let (command_sender, command_receiver) = bounded(100);
        let (message_sender, message_receiver) = bounded(200);
        let message_sender = SenderWrapper::new(message_sender);

        // call non-pub fn to setup the inner loop

        Ok(UTransportRouterHandle {
            command_sender,
            message_sender,
        })
    }
}

pub struct UTransportRouterHandle {
    pub(crate) command_sender: Sender<UTransportRouterCommand>,
    pub(crate) message_sender: SenderWrapper<UMessage>,
}

mod tests {

    use crate::up_transport_router::UTransportRouter;
    use crate::utransport_builder::UTransportBuilder;
    use async_trait::async_trait;
    use up_rust::{UMessage, UStatus, UTransport, UUri};

    pub struct UPClientFoo;

    #[async_trait]
    impl UTransport for UPClientFoo {
        async fn send(&self, message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
        ) -> Result<String, UStatus> {
            todo!()
        }

        async fn unregister_listener(&self, topic: UUri, listener: &str) -> Result<(), UStatus> {
            todo!()
        }
    }

    impl UPClientFoo {
        pub fn new() -> Self {
            Self {}
        }
    }
    pub struct UTransportBuilderFoo;
    impl UTransportBuilder for UTransportBuilderFoo {
        fn build(&self) -> Box<dyn UTransport> {
            let utransport_foo: Box<dyn UTransport> = Box::new(UPClientFoo::new());
            utransport_foo
        }
    }

    impl UTransportBuilderFoo {
        pub fn new() -> Self {
            Self {}
        }
    }

    #[test]
    fn test_creating_utransport_router() {
        let utransport_builder_foo = UTransportBuilderFoo::new();
        let utransport_router_handle = UTransportRouter::new(utransport_builder_foo);
    }
}
