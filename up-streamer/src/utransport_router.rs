use crate::utransport_builder::UTransportBuilder;
use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use async_trait::async_trait;
use futures::select;
use futures::FutureExt;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use up_rust::{UAuthority, UCode, UMessage, UStatus, UTransport, UUIDBuilder, UUri, UUID};

fn uauthority_to_uuri(authority: UAuthority) -> UUri {
    UUri {
        authority: Some(authority).into(),
        ..Default::default()
    }
}

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

impl<T> Deref for SenderWrapper<T> {
    type Target = Sender<T>;

    fn deref(&self) -> &Self::Target {
        &*self.sender
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
    Register(UAuthority, SenderWrapper<UMessage>),
    Unregister(UAuthority, SenderWrapper<UMessage>),
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
    listener_map: Arc<Mutex<HashMap<(UAuthority, SenderWrapper<UMessage>), String>>>,
    command_sender: Sender<UTransportRouterCommand>,
    command_receiver: Receiver<UTransportRouterCommand>,
    message_sender: SenderWrapper<UMessage>,
    message_receiver: Receiver<UMessage>,
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

        let utransport_router_inner = UTransportRouterInner {
            utransport,
            listener_map: Arc::new(Mutex::new(HashMap::new())),
            command_sender: command_sender.clone(),
            command_receiver: command_receiver.clone(),
            message_sender: message_sender.clone(),
            message_receiver: message_receiver.clone(),
        };

        utransport_router_inner
            .launch(command_receiver, message_receiver)
            .await;

        Ok(UTransportRouterHandle {
            command_sender,
            message_sender,
        })
    }

    async fn launch(
        &self,
        mut command_receiver: Receiver<UTransportRouterCommand>,
        mut message_receiver: Receiver<UMessage>,
    ) {
        let mut command_fut = command_receiver.recv().fuse();
        let mut message_fut = message_receiver.recv().fuse();

        loop {
            select! {
                command = command_fut => match command {
                    Ok(command) => {
                        self.handle_command(command).await;
                        command_fut = command_receiver.recv().fuse(); // Re-arm future for the next iteration
                    },
                    Err(e) => println!("Error receiving a command: {:?}", e),
                },
                message = message_fut => match message {
                    Ok(msg) => {
                        self.handle_message(msg).await;
                        message_fut = message_receiver.recv().fuse(); // Re-arm future for the next iteration
                    },
                    Err(e) => println!("Error receiving a message: {:?}", e),
                },
            }
        }
    }

    async fn handle_command(&self, command: UTransportRouterCommand) {
        match command {
            UTransportRouterCommand::Register(in_authority, in_sender_wrapper) => {
                if self.message_sender == in_sender_wrapper {
                    // bail in this case, we shouldn't be sending to ourselves
                    // log an error
                }

                let mut listener_map = self.listener_map.lock().unwrap();

                if listener_map
                    .get(&(in_authority.clone(), in_sender_wrapper.clone()))
                    .is_none()
                {
                    let in_sender_wrapper_closure = in_sender_wrapper.clone();
                    let callback_closure = move |received: Result<UMessage, UStatus>| {
                        let in_sender_wrapper_closure = in_sender_wrapper_closure.clone();
                        task::spawn_local(forwarding_callback(
                            received,
                            in_sender_wrapper_closure.clone(),
                        ));
                    };

                    let registration_uuri = uauthority_to_uuri(in_authority.clone());
                    let registration_result = self
                        .utransport
                        .register_listener(registration_uuri, Box::new(callback_closure))
                        .await;
                    if let Ok(registration_string) = registration_result {
                        listener_map.insert((in_authority, in_sender_wrapper), registration_string);
                    }
                }
            }
            UTransportRouterCommand::Unregister(in_authority, in_sender_wrapper) => {
                if self.message_sender == in_sender_wrapper {
                    // bail in this case, we shouldn't be sending to ourselves
                    // log an error
                }

                let mut listener_map = self.listener_map.lock().unwrap();

                if listener_map
                    .remove(&(in_authority.clone(), in_sender_wrapper.clone()))
                    .is_none()
                {
                    // log an error
                }
            }
        }
    }

    async fn handle_message(&self, message: UMessage) {
        let send_result = self.utransport.send(message).await;
        if let Err(e) = send_result {
            // log an error
        }
    }
}

async fn forwarding_callback(
    received: Result<UMessage, UStatus>,
    in_sender_wrapper: SenderWrapper<UMessage>,
) {
    if let Ok(msg) = received {
        let forward_result = in_sender_wrapper.send(msg).await;
        if let Err(e) = forward_result {
            // log error e here
        }
    }
}

pub struct UTransportRouterHandle {
    pub(crate) command_sender: Sender<UTransportRouterCommand>,
    pub(crate) message_sender: SenderWrapper<UMessage>,
}

impl UTransportRouterHandle {
    pub async fn register(
        &self,
        in_authority: UAuthority,
        in_sender_wrapper: SenderWrapper<UMessage>,
    ) -> Result<(), UStatus> {
        let send_res = self
            .command_sender
            .send(UTransportRouterCommand::Register(
                in_authority,
                in_sender_wrapper,
            ))
            .await
            .map_err(|e| {
                UStatus::fail_with_code(UCode::INTERNAL, format!("Unable to forward: {:?}", e))
            })?;
        Ok(())
    }

    pub async fn unregister(
        &self,
        in_authority: UAuthority,
        in_sender_wrapper: SenderWrapper<UMessage>,
    ) -> Result<(), UStatus> {
        let send_res = self
            .command_sender
            .send(UTransportRouterCommand::Unregister(
                in_authority,
                in_sender_wrapper,
            ))
            .await
            .map_err(|e| {
                UStatus::fail_with_code(UCode::INTERNAL, format!("Unable to forward: {:?}", e))
            })?;
        Ok(())
    }
}

mod tests {

    use crate::utransport_builder::UTransportBuilder;
    use crate::utransport_router::UTransportRouter;
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
