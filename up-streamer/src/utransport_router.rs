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
    pub fn new<T>(name: String, utransport_builder: T) -> Result<UTransportRouterHandle, UStatus>
    where
        T: UTransportBuilder + 'static,
    {
        let (tx, rx) = mpsc::channel();

        println!("{name}: before spawning thread");

        thread::spawn(move || {
            println!("{name}: inside spawned thread");
            task::block_on(async {
                println!("{name}: inside task::block_on()");
                let result = UTransportRouterInner::new(name, utransport_builder).await;
                tx.send(result).unwrap();
            });
        });
        rx.recv().unwrap()
    }
}

struct UTransportRouterInner {
    name: Arc<String>,
    utransport: Box<dyn UTransport>,
    listener_map: Arc<Mutex<HashMap<(UAuthority, SenderWrapper<UMessage>), String>>>,
    command_sender: Sender<UTransportRouterCommand>,
    command_receiver: Receiver<UTransportRouterCommand>,
    message_sender: SenderWrapper<UMessage>,
    message_receiver: Receiver<UMessage>,
}

impl UTransportRouterInner {
    pub async fn new<T>(
        name: String,
        utransport_builder: T,
    ) -> Result<UTransportRouterHandle, UStatus>
    where
        T: UTransportBuilder,
    {
        let name = name.clone();
        println!("{name}: inside UTransportRouterInner");

        let utransport = utransport_builder.build(); // TODO: May want to allow this to fail
        let (command_sender, command_receiver) = bounded(100);
        let (message_sender, message_receiver) = bounded(200);
        let message_sender = SenderWrapper::new(message_sender);

        println!("{name}: before creating UTransportRouterInner");

        let utransport_router_inner = Arc::new(UTransportRouterInner {
            name: Arc::new(name.to_string()),
            utransport,
            listener_map: Arc::new(Mutex::new(HashMap::new())),
            command_sender: command_sender.clone(),
            command_receiver: command_receiver.clone(),
            message_sender: message_sender.clone(),
            message_receiver: message_receiver.clone(),
        });

        println!("{name}: after creating UTransportRouterInner");

        let utransport_router_inner_clone = utransport_router_inner.clone();
        let name_clone = name.clone();
        task::spawn_local(async move {
            println!("{name_clone}: inside of task::spawn_local to launch");
            utransport_router_inner_clone
                .launch(command_receiver, message_receiver)
                .await;
            println!("{name_clone}: inside of task::spawn_local after launch");
        });
        println!("{name}: after task::spawn_local for launch");

        Ok(UTransportRouterHandle {
            name: name.to_string(),
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

        println!("{}: inside of launch", &self.name);

        loop {
            println!("{}: top of loop before select!", &self.name);
            select! {
                command = command_fut => match command {
                    Ok(command) => {
                        println!("{}: received command", &self.name);
                        self.handle_command(command).await;
                        command_fut = command_receiver.recv().fuse(); // Re-arm future for the next iteration
                    },
                    Err(e) => println!("{}: Error receiving a command: {:?}", &self.name, e),
                },
                message = message_fut => match message {
                    Ok(msg) => {
                        println!("{}: received message", &self.name);
                        self.handle_message(msg).await;
                        message_fut = message_receiver.recv().fuse(); // Re-arm future for the next iteration
                    },
                    Err(e) => println!("{}: Error receiving a message: {:?}", &self.name, e),
                },
            }
        }
    }

    async fn handle_command(&self, command: UTransportRouterCommand) {
        match command {
            UTransportRouterCommand::Register(in_authority, in_sender_wrapper) => {
                println!("{}: Register command", &self.name);
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
                    let callback_closure: Box<
                        dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static,
                    > = Box::new(move |received: Result<UMessage, UStatus>| {
                        let in_sender_wrapper_closure = in_sender_wrapper_closure.clone();
                        task::spawn_local(forwarding_callback(
                            // self.name.clone(),
                            received,
                            in_sender_wrapper_closure.clone(),
                        ));
                    });

                    let registration_uuri = uauthority_to_uuri(in_authority.clone());
                    let registration_result = self
                        .utransport
                        .register_listener(registration_uuri, callback_closure)
                        .await;
                    if let Ok(registration_string) = registration_result {
                        listener_map.insert((in_authority, in_sender_wrapper), registration_string);
                    }
                }
            }
            UTransportRouterCommand::Unregister(in_authority, in_sender_wrapper) => {
                println!("{}: Unregister command", &self.name);
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
        println!("{}: inside handle_message", &self.name);
        let send_result = self.utransport.send(message).await;
        if let Err(e) = send_result {
            // log an error
        }
    }
}

async fn forwarding_callback(
    // name: String,
    received: Result<UMessage, UStatus>,
    in_sender_wrapper: SenderWrapper<UMessage>,
) {
    // println!("{}: inside of forwarding_callback", name);
    if let Ok(msg) = received {
        let forward_result = in_sender_wrapper.send(msg).await;
        if let Err(e) = forward_result {
            // log error e here
        }
    }
}

pub struct UTransportRouterHandle {
    pub(crate) name: String,
    pub(crate) command_sender: Sender<UTransportRouterCommand>,
    pub(crate) message_sender: SenderWrapper<UMessage>,
}

impl UTransportRouterHandle {
    pub async fn register(
        &self,
        in_authority: UAuthority,
        in_sender_wrapper: SenderWrapper<UMessage>,
    ) -> Result<(), UStatus> {
        println!("{}: inside of register", &self.name);
        self.command_sender
            .send(UTransportRouterCommand::Register(
                in_authority,
                in_sender_wrapper,
            ))
            .await
            .map_err(|e| {
                UStatus::fail_with_code(
                    UCode::INTERNAL,
                    format!("{}: Unable to forward: {:?}", &self.name, e),
                )
            })?;
        Ok(())
    }

    pub async fn unregister(
        &self,
        in_authority: UAuthority,
        in_sender_wrapper: SenderWrapper<UMessage>,
    ) -> Result<(), UStatus> {
        println!("{}: inside of unregister", &self.name);
        self.command_sender
            .send(UTransportRouterCommand::Unregister(
                in_authority,
                in_sender_wrapper,
            ))
            .await
            .map_err(|e| {
                UStatus::fail_with_code(
                    UCode::INTERNAL,
                    format!("{}: Unable to forward: {:?}", &self.name, e),
                )
            })?;
        Ok(())
    }
}
