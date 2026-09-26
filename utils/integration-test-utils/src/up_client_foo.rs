/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

use async_broadcast::{Receiver, Sender};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use tokio::runtime::Builder;
use tokio::sync::Mutex;
use tracing::debug;
use up_rust::{
    verify_filter_criteria, ComparableListener, ListenerAdmission, UCode, UListener, UMessage,
    UStatus, UTransport, UUri,
};

type Registrations =
    HashMap<(UUri, Option<UUri>), HashMap<ComparableListener, Arc<ListenerAdmission>>>;

/// In-process bus fixture with the same full-filter/admission contract as UTransport.
pub struct UPClientFoo {
    name: Arc<String>,
    protocol_sender: Sender<Result<UMessage, UStatus>>,
    listeners: Arc<Mutex<Registrations>>,
    pub times_received: Arc<AtomicU64>,
}

impl UPClientFoo {
    pub async fn new(
        name: &str,
        mut protocol_receiver: Receiver<Result<UMessage, UStatus>>,
        protocol_sender: Sender<Result<UMessage, UStatus>>,
    ) -> Self {
        let me = Self {
            name: Arc::new(name.to_string()),
            protocol_sender,
            listeners: Arc::new(Mutex::new(HashMap::new())),
            times_received: Arc::new(AtomicU64::new(0)),
        };
        let name = me.name.clone();
        let listeners = me.listeners.clone();
        let times_received = me.times_received.clone();
        thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("create fixture runtime");
            runtime.block_on(async move {
                while let Ok(received) = protocol_receiver.recv().await {
                    let msg = match received {
                        Ok(msg) => msg,
                        Err(status) => {
                            debug!("{name}: bus error: {status:?}");
                            continue;
                        }
                    };
                    let matching = {
                        let registrations = listeners.lock().await;
                        registrations
                            .iter()
                            .filter(|((source, sink), _)| {
                                source.matches(msg.attributes().source())
                                    && match (sink.as_ref(), msg.attributes().sink()) {
                                        (None, None) => true,
                                        (Some(pattern), Some(actual)) => pattern.matches(actual),
                                        _ => false,
                                    }
                            })
                            .flat_map(|(_, entries)| {
                                entries.iter().map(|(listener, admission)| {
                                    (listener.clone(), Arc::clone(admission))
                                })
                            })
                            .collect::<Vec<_>>()
                    };
                    // No registry lock crosses user code. A removed registration
                    // cannot enter from an already collected dispatch snapshot.
                    for (listener, admission) in matching {
                        admission
                            .dispatch(|| async {
                                times_received.fetch_add(1, Ordering::SeqCst);
                                listener.on_receive(msg.clone()).await;
                            })
                            .await;
                    }
                }
            });
        });
        me
    }
}

#[async_trait]
impl UTransport for UPClientFoo {
    async fn send(&self, message: UMessage) -> Result<(), UStatus> {
        self.protocol_sender
            .broadcast(Ok(message))
            .await
            .map(|_| ())
            .map_err(|_| {
                UStatus::fail_with_code(UCode::Internal, "Unable to send over Foo protocol")
            })
    }

    async fn receive(&self, _source: &UUri, _sink: Option<&UUri>) -> Result<UMessage, UStatus> {
        Err(UStatus::fail_with_code(
            UCode::Unimplemented,
            "Foo fixture supports listener receive",
        ))
    }

    async fn register_listener(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        listener: Arc<dyn UListener>,
    ) -> Result<(), UStatus> {
        verify_filter_criteria(source, sink).map_err(|status| *status)?;
        let mut registrations = self.listeners.lock().await;
        let entries = registrations
            .entry((source.clone(), sink.cloned()))
            .or_default();
        let listener = ComparableListener::new(listener);
        match entries.entry(listener) {
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(UStatus::fail_with_code(
                    UCode::AlreadyExists,
                    "filter/listener already registered",
                ));
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Arc::new(ListenerAdmission::new()));
            }
        }
        Ok(())
    }

    async fn unregister_listener(
        &self,
        source: &UUri,
        sink: Option<&UUri>,
        listener: Arc<dyn UListener>,
    ) -> Result<(), UStatus> {
        let key = (source.clone(), sink.cloned());
        let admission = {
            let mut registrations = self.listeners.lock().await;
            let entries = registrations.get_mut(&key).ok_or_else(|| {
                UStatus::fail_with_code(UCode::NotFound, "filter/listener not registered")
            })?;
            let admission = entries
                .remove(&ComparableListener::new(listener))
                .ok_or_else(|| {
                    UStatus::fail_with_code(UCode::NotFound, "filter/listener not registered")
                })?;
            if entries.is_empty() {
                registrations.remove(&key);
            }
            admission
        };
        admission.stop().await;
        Ok(())
    }
}
