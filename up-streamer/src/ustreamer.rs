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

use crate::endpoint::Endpoint;
use async_std::channel::{Receiver, Sender};
use async_std::sync::{Arc, Mutex};
use async_std::{channel, task};
use async_trait::async_trait;
use log::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::thread;
use up_rust::{UAuthority, UCode, UListener, UMessage, UStatus, UTransport, UUIDBuilder, UUri};

const USTREAMER_TAG: &str = "UStreamer:";
const USTREAMER_FN_NEW_TAG: &str = "new():";
const USTREAMER_FN_ADD_FORWARDING_RULE_TAG: &str = "add_forwarding_rule():";
const USTREAMER_FN_DELETE_FORWARDING_RULE_TAG: &str = "delete_forwarding_rule():";

// the 'gatekeeper' which will prevent us from erroneously being able to add duplicate
// forwarding rules or delete those rules which don't exist
type ForwardingRules = Mutex<
    HashMap<
        (
            UAuthority,
            UAuthority,
            ComparableTransport,
            ComparableTransport,
        ),
        Arc<dyn UListener>,
    >,
>;
// for keeping track of how many users there are of the TransportForwarder so that we can remove
// from the TransportForwarders later when no longer needed
type TransportForwarderCount = Mutex<HashMap<ComparableTransport, usize>>;
// we only need one TransportForwarder per out `UTransport`, so we keep track of that one here
// and the Sender necessary to hand off to the listener for the in `UTransport`
type TransportForwarders =
    Mutex<HashMap<ComparableTransport, (TransportForwarder, Sender<Arc<UMessage>>)>>;
// we need to keep track of in `UTransport` and out `UAuthority` since in the case of the same
// in `UTransport` and out `UAuthority` we should not register more than once
type InTransportOutAuthorities = Mutex<HashMap<(ComparableTransport, UAuthority), usize>>;

/// A [`UStreamer`] is used to coordinate the addition and deletion of forwarding rules between
/// [`Endpoint`][crate::Endpoint]s
///
/// Essentially, it's a means of setting up rules so that messages from one transport (e.g. Zenoh)
/// are bridged onto another transport (e.g. SOME/IP).
///
/// # Examples
///
/// ## Typical usage
/// ```
/// use std::sync::Arc;
/// use async_std::sync::Mutex;
/// use up_rust::{Number, UAuthority, UListener, UTransport};
/// use up_streamer::{Endpoint, UStreamer};
/// # pub mod up_client_foo {
/// #     use std::sync::Arc;
/// use async_trait::async_trait;
/// #     use up_rust::{UListener, UMessage, UStatus, UUIDBuilder, UUri};
/// #     use up_rust::UTransport;
/// #
/// #     pub struct UPClientFoo;
/// #
/// #     #[async_trait]
/// #     impl UTransport for UPClientFoo {
/// #         async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
/// #             todo!()
/// #         }
/// #
/// #         async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
/// #             todo!()
/// #         }
/// #
/// #         async fn register_listener(
/// #             &self,
/// #             topic: UUri,
/// #             _listener: Arc<dyn UListener>,
/// #         ) -> Result<(), UStatus> {
/// #             println!("UPClientFoo: registering topic: {:?}", topic);
/// #             Ok(())
/// #         }
/// #
/// #         async fn unregister_listener(&self, topic: UUri, _listener: Arc<dyn UListener>) -> Result<(), UStatus> {
/// #             println!(
/// #                 "UPClientFoo: unregistering topic: {topic:?}"
/// #             );
/// #             Ok(())
/// #         }
/// #     }
/// #
/// #     impl UPClientFoo {
/// #         pub fn new() -> Self {
/// #             Self {}
/// #         }
/// #     }
/// # }
/// #
/// # pub mod up_client_bar {
/// #     use std::sync::Arc;
/// #     use async_trait::async_trait;
/// #     use up_rust::{UListener, UMessage, UStatus, UTransport, UUIDBuilder, UUri};
/// #     pub struct UPClientBar;
/// #
/// #     #[async_trait]
/// #     impl UTransport for UPClientBar {
/// #         async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
/// #             todo!()
/// #         }
/// #
/// #         async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
/// #             todo!()
/// #         }
/// #
/// #         async fn register_listener(
/// #             &self,
/// #             topic: UUri,
/// #             _listener: Arc<dyn UListener>,
/// #         ) -> Result<(), UStatus> {
/// #             println!("UPClientBar: registering topic: {:?}", topic);
/// #             Ok(())
/// #         }
/// #
/// #         async fn unregister_listener(&self, topic: UUri, _listener: Arc<dyn UListener>) -> Result<(), UStatus> {
/// #             println!(
/// #                 "UPClientFoo: unregistering topic: {topic:?}"
/// #             );
/// #             Ok(())
/// #         }
/// #     }
/// #
/// #     impl UPClientBar {
/// #         pub fn new() -> Self {
/// #             Self {}
/// #         }
/// #     }
/// # }
/// #
/// # async fn async_main() {
///
/// // Local transport
/// let local_transport: Arc<dyn UTransport> = Arc::new(up_client_foo::UPClientFoo::new());
///
/// // Remote transport router
/// let remote_transport: Arc<dyn UTransport> = Arc::new(up_client_bar::UPClientBar::new());
///
/// // Local endpoint
/// let local_authority = UAuthority {
///     name: Some("local".to_string()),
///     number: Some(Number::Ip(vec![192, 168, 1, 100])),
///     ..Default::default()
/// };
/// let local_endpoint = Endpoint::new("local_endpoint", local_authority, local_transport.clone());
///
/// // A remote endpoint
/// let remote_authority = UAuthority {
///     name: Some("remote".to_string()),
///     number: Some(Number::Ip(vec![192, 168, 1, 200])),
///     ..Default::default()
/// };
/// let remote_endpoint = Endpoint::new("remote_endpoint", remote_authority, remote_transport.clone());
///
/// let streamer = UStreamer::new("hoge", 100);
///
/// // Add forwarding rules to endpoint local<->remote
/// assert_eq!(
///     streamer
///         .add_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
///         .await,
///     Ok(())
/// );
/// assert_eq!(
///     streamer
///         .add_forwarding_rule(remote_endpoint.clone(), local_endpoint.clone())
///         .await,
///     Ok(())
/// );
///
/// // Add forwarding rules to endpoint local<->local, should report an error
/// assert!(streamer
///     .add_forwarding_rule(local_endpoint.clone(), local_endpoint.clone())
///     .await
///     .is_err());
///
/// // Rule already exists so it should report an error
/// assert!(streamer
///     .add_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
///     .await
///     .is_err());
///
/// // Try and remove an invalid rule
/// assert!(streamer
///     .delete_forwarding_rule(remote_endpoint.clone(), remote_endpoint.clone())
///     .await
///     .is_err());
///
/// // remove valid routing rules
/// assert_eq!(
///     streamer
///         .delete_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
///         .await,
///     Ok(())
/// );
/// assert_eq!(
///     streamer
///         .delete_forwarding_rule(remote_endpoint.clone(), local_endpoint.clone())
///         .await,
///     Ok(())
/// );
///
/// // Try and remove a rule that doesn't exist, should report an error
/// assert!(streamer
///     .delete_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
///     .await
///     .is_err());
/// # }
/// ```
pub struct UStreamer {
    #[allow(dead_code)]
    name: String,
    registered_forwarding_rules: ForwardingRules,
    transport_forwarders: TransportForwarders,
    transport_forwarder_count: TransportForwarderCount,
    in_transport_out_authorities: InTransportOutAuthorities,
    message_queue_size: usize,
}

impl UStreamer {
    /// Creates a new UStreamer which can be used to add forwarding rules.
    ///
    /// # Parameters
    ///
    /// * name - Used to uniquely identify this UStreamer in logs
    /// * message_queue_size - Determines size of channel used to communicate between `ForwardingListener`
    ///                        and the worker tasks for each currently endpointd `UTransport`
    pub fn new(name: &str, message_queue_size: usize) -> Self {
        let name = format!("{USTREAMER_TAG}:{name}:");
        // Try to initiate logging.
        // Required in case of dynamic lib, otherwise no logs.
        // But cannot be done twice in case of static link.
        let _ = env_logger::try_init();
        debug!(
            "{}:{}:{} UStreamer created",
            &name, USTREAMER_TAG, USTREAMER_FN_NEW_TAG
        );

        Self {
            name: name.to_string(),
            registered_forwarding_rules: Mutex::new(HashMap::new()),
            transport_forwarders: Mutex::new(HashMap::new()),
            transport_forwarder_count: Mutex::new(HashMap::new()),
            in_transport_out_authorities: Mutex::new(HashMap::new()),
            message_queue_size,
        }
    }

    fn uauthority_to_uuri(authority: UAuthority) -> UUri {
        UUri {
            authority: Some(authority).into(),
            ..Default::default()
        }
    }

    #[inline(always)]
    fn forwarding_id(r#in: &Endpoint, out: &Endpoint) -> String {
        format!(
            "[in.name: {}, in.authority: {:?} ; out.name: {}, out.authority: {:?}]",
            r#in.name, r#in.authority, out.name, out.authority
        )
    }

    #[inline(always)]
    fn fail_due_to_same_authority(&self, r#in: &Endpoint, out: &Endpoint) -> Result<(), UStatus> {
        let err = Err(UStatus::fail_with_code(
            UCode::INVALID_ARGUMENT,
            format!(
                "{} are the same. Unable to delete.",
                Self::forwarding_id(r#in, out)
            ),
        ));
        error!(
            "{}:{}:{} Deleting forwarding rule failed: {:?}",
            self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG, err
        );
        err
    }

    /// Adds a forwarding rule to the [`UStreamer`] based on an in [`Endpoint`][crate::Endpoint] and an
    /// out [`Endpoint`][crate::Endpoint]
    ///
    /// Works for any [`UMessage`][up_rust::UMessage] type which has a destination / sink contained
    /// in its attributes, i.e.
    /// * [`UMessageType::UMESSAGE_TYPE_NOTIFICATION`][up_rust::UMessageType::UMESSAGE_TYPE_NOTIFICATION]
    /// * [`UMessageType::UMESSAGE_TYPE_REQUEST`][up_rust::UMessageType::UMESSAGE_TYPE_REQUEST]
    /// * [`UMessageType::UMESSAGE_TYPE_RESPONSE`][up_rust::UMessageType::UMESSAGE_TYPE_RESPONSE]
    ///
    /// # Parameters
    ///
    /// * `in` - [`Endpoint`][crate::Endpoint] we will bridge _from_
    /// * `out` - [`Endpoint`][crate::Endpoint] we will bridge _onto_
    ///
    /// # Errors
    ///
    /// If unable to add this forwarding rule, we return a [`UStatus`][up_rust::UStatus] noting
    /// the error.
    ///
    /// Typical errors include
    /// * already have this forwarding rule registered
    /// * attempting to forward onto the same [`Endpoint`][crate::Endpoint]
    pub async fn add_forwarding_rule(&self, r#in: Endpoint, out: Endpoint) -> Result<(), UStatus> {
        debug!(
            "{}:{}:{} Adding forwarding rule for {}",
            self.name,
            USTREAMER_TAG,
            USTREAMER_FN_ADD_FORWARDING_RULE_TAG,
            Self::forwarding_id(&r#in, &out)
        );

        if r#in.authority == out.authority {
            return self.fail_due_to_same_authority(&r#in, &out);
        }

        let in_comparable_transport = ComparableTransport::new(r#in.transport.clone());
        let out_comparable_transport = ComparableTransport::new(out.transport.clone());

        let sender = {
            let mut transport_forwarders = self.transport_forwarders.lock().await;

            let (_, sender) = transport_forwarders
                .entry(out_comparable_transport.clone())
                .or_insert_with(|| {
                    let (tx, rx) = channel::bounded(self.message_queue_size);
                    (TransportForwarder::new(out.transport.clone(), rx), tx)
                });
            sender.clone()
        };
        let forwarding_listener: Arc<dyn UListener> =
            Arc::new(ForwardingListener::new(&Self::forwarding_id(&r#in, &out), sender).await);

        let insertion_result = {
            let mut registered_forwarding_rules = self.registered_forwarding_rules.lock().await;
            registered_forwarding_rules.insert(
                (
                    r#in.authority.clone(),
                    out.authority.clone(),
                    in_comparable_transport.clone(),
                    out_comparable_transport.clone(),
                ),
                forwarding_listener.clone(),
            )
        };
        if let Some(_) = insertion_result {
            let err = Err(UStatus::fail_with_code(
                UCode::ALREADY_EXISTS,
                format!(
                    "{} are already routed. Unable to add same rule twice.",
                    Self::forwarding_id(&r#in, &out)
                ),
            ));
            warn!(
                "{}:{}:{} Adding forwarding rule failed: {:?}",
                self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG, err
            );
            err
        } else {
            // we keep track of how many entries are using this TransportForwarder so that we can
            // remove it later
            {
                let mut transport_forwarder_count = self.transport_forwarder_count.lock().await;
                let count = transport_forwarder_count
                    .entry(in_comparable_transport.clone())
                    .or_default();
                *count += 1;
            }

            let mut in_transport_out_authorities = self.in_transport_out_authorities.lock().await;
            if let Some(count) = in_transport_out_authorities
                .get_mut(&(in_comparable_transport.clone(), out.authority.clone()))
            {
                debug!(
                    "{}:{}:{} This pair of in_transport and out_authority already registered, avoid duplication of messages.",
                    self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG
                );
                *count += 1;
                Ok(())
            } else {
                debug!(
                    "{}:{}:{} This pair of in_transport and out_authority not registered yet, proceeding to register on in_transport.",
                    self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG
                );
                let registration_result = r#in
                    .transport
                    .register_listener(
                        Self::uauthority_to_uuri(out.authority.clone()),
                        forwarding_listener,
                    )
                    .await;
                if let Err(err) = registration_result.as_ref() {
                    error!(
                        "{}:{}:{} Adding forwarding rule failed: {:?}",
                        self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG, err
                    );
                } else {
                    in_transport_out_authorities
                        .insert((in_comparable_transport.clone(), out.authority), 1);
                    debug!(
                        "{}:{}:{} Adding forwarding rule succeeded",
                        self.name, USTREAMER_TAG, USTREAMER_FN_ADD_FORWARDING_RULE_TAG
                    );
                }
                registration_result
            }
        }
    }

    /// Deletes a forwarding rule from the [`UStreamer`] based on an in [`Endpoint`][crate::Endpoint] and an
    /// out [`Endpoint`][crate::Endpoint]
    ///
    /// Works for any [`UMessage`][up_rust::UMessage] type which has a destination / sink contained
    /// in its attributes, i.e.
    /// * [`UMessageType::UMESSAGE_TYPE_NOTIFICATION`][up_rust::UMessageType::UMESSAGE_TYPE_NOTIFICATION]
    /// * [`UMessageType::UMESSAGE_TYPE_REQUEST`][up_rust::UMessageType::UMESSAGE_TYPE_REQUEST]
    /// * [`UMessageType::UMESSAGE_TYPE_RESPONSE`][up_rust::UMessageType::UMESSAGE_TYPE_RESPONSE]
    ///
    /// # Parameters
    ///
    /// * `in` - [`Endpoint`][crate::Endpoint] we will bridge _from_
    /// * `out` - [`Endpoint`][crate::Endpoint] we will bridge _onto_
    ///
    /// # Errors
    ///
    /// If unable to delete this forwarding rule, we return a [`UStatus`][up_rust::UStatus] noting
    /// the error.
    ///
    /// Typical errors include
    /// * No such route has been added
    /// * attempting to delete a forwarding rule where we would forward onto the same [`Endpoint`][crate::Endpoint]
    pub async fn delete_forwarding_rule(
        &self,
        r#in: Endpoint,
        out: Endpoint,
    ) -> Result<(), UStatus> {
        debug!(
            "{}:{}:{} Deleting forwarding rule for {}",
            self.name,
            USTREAMER_TAG,
            USTREAMER_FN_DELETE_FORWARDING_RULE_TAG,
            Self::forwarding_id(&r#in, &out)
        );

        if r#in.authority == out.authority {
            return self.fail_due_to_same_authority(&r#in, &out);
        }

        let in_comparable_transport = ComparableTransport::new(r#in.transport.clone());
        let out_comparable_transport = ComparableTransport::new(out.transport.clone());

        let remove_res = {
            let mut registered_forwarding_rules = self.registered_forwarding_rules.lock().await;
            registered_forwarding_rules.remove(&(
                r#in.authority.clone(),
                out.authority.clone(),
                in_comparable_transport.clone(),
                out_comparable_transport.clone(),
            ))
        };
        if let Some(exists) = remove_res {
            // check if all users of this TransportForwarder have been unregistered and if so
            // remove it
            {
                let count = {
                    let mut transport_forwarder_count = self.transport_forwarder_count.lock().await;
                    let count = transport_forwarder_count
                        .entry(in_comparable_transport.clone())
                        .or_default();
                    *count -= 1;
                    *count
                };
                if count == 0 {
                    let mut transport_forwarders = self.transport_forwarders.lock().await;

                    if let Some(_) = transport_forwarders.remove(&out_comparable_transport.clone())
                    {
                        debug!(
                            "{}:{}:{} Removing TransportForwarder succeeded",
                            self.name, USTREAMER_TAG, USTREAMER_FN_DELETE_FORWARDING_RULE_TAG,
                        );
                    } else {
                        let err = UStatus::fail_with_code(
                            UCode::NOT_FOUND,
                            "TrnasportForwarder not found",
                        );
                        error!(
                            "{}:{}:{} Removing TransportForwarder failed. {:?}",
                            self.name, USTREAMER_TAG, USTREAMER_FN_DELETE_FORWARDING_RULE_TAG, err,
                        );
                        return Err(err);
                    }
                }
            }

            let mut in_transport_out_authorities = self.in_transport_out_authorities.lock().await;
            if let Some(count) = in_transport_out_authorities
                .get_mut(&(in_comparable_transport.clone(), out.authority.clone()))
            {
                *count -= 1;
                if *count == 0 {
                    let unregister_res = r#in
                        .transport
                        .unregister_listener(Self::uauthority_to_uuri(out.authority), exists)
                        .await;

                    if let Err(err) = unregister_res.as_ref() {
                        error!(
                            "{}:{}:{} Deleting forwarding rule failed: {:?}",
                            self.name, USTREAMER_TAG, USTREAMER_FN_DELETE_FORWARDING_RULE_TAG, err
                        );
                    } else {
                        debug!(
                            "{}:{}:{} Deleting forwarding rule succeeded",
                            self.name, USTREAMER_TAG, USTREAMER_FN_DELETE_FORWARDING_RULE_TAG
                        );
                    }
                    unregister_res
                } else {
                    let no_in_transport_out_authority_found = UStatus::fail_with_code(
                        UCode::NOT_FOUND,
                        "No in_transport and out_authority found corresponding",
                    );
                    error!(
                        "{}:{}:{} Deleting forwarding rule failed: {:?}",
                        self.name,
                        USTREAMER_TAG,
                        USTREAMER_FN_DELETE_FORWARDING_RULE_TAG,
                        &no_in_transport_out_authority_found
                    );
                    Err(no_in_transport_out_authority_found)
                }
            } else {
                Ok(())
            }
        } else {
            let err = Err(UStatus::fail_with_code(
                UCode::NOT_FOUND,
                format!(
                    "{} is not routed. No rule to delete.",
                    Self::forwarding_id(&r#in, &out)
                ),
            ));
            warn!(
                "{}:{}:{} Deleting forwarding rule failed: {:?}",
                self.name, USTREAMER_TAG, USTREAMER_FN_DELETE_FORWARDING_RULE_TAG, err
            );
            err
        }
    }
}

#[derive(Clone)]
pub(crate) struct ComparableTransport {
    transport: Arc<dyn UTransport>,
}

impl ComparableTransport {
    pub fn new(transport: Arc<dyn UTransport>) -> Self {
        Self { transport }
    }
}

impl Hash for ComparableTransport {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.transport).hash(state);
    }
}

impl PartialEq for ComparableTransport {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.transport, &other.transport)
    }
}

impl Eq for ComparableTransport {}

const TRANSPORT_FORWARDER_TAG: &str = "TransportForwarder:";
const TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG: &str = "message_forwarding_loop():";
pub(crate) struct TransportForwarder {}

impl TransportForwarder {
    fn new(out_transport: Arc<dyn UTransport>, message_receiver: Receiver<Arc<UMessage>>) -> Self {
        let out_transport_clone = out_transport.clone();
        let message_receiver_clone = message_receiver.clone();
        thread::spawn(|| {
            task::block_on(Self::message_forwarding_loop(
                UUIDBuilder::build().to_hyphenated_string(),
                out_transport_clone,
                message_receiver_clone,
            ))
        });

        Self {}
    }

    async fn message_forwarding_loop(
        id: String,
        out_transport: Arc<dyn UTransport>,
        message_receiver: Receiver<Arc<UMessage>>,
    ) {
        while let Ok(msg) = message_receiver.recv().await {
            debug!(
                "{}:{}:{} Attempting send of message: {:?}",
                id,
                TRANSPORT_FORWARDER_TAG,
                TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG,
                msg
            );

            let send_res = out_transport.send(msg.deref().clone()).await;
            if let Err(err) = send_res {
                warn!(
                    "{}:{}:{} Sending on out_transport failed: {:?}",
                    id,
                    TRANSPORT_FORWARDER_TAG,
                    TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG,
                    err
                );
            } else {
                debug!(
                    "{}:{}:{} Sending on out_transport succeeded",
                    id, TRANSPORT_FORWARDER_TAG, TRANSPORT_FORWARDER_FN_MESSAGE_FORWARDING_LOOP_TAG
                );
            }
        }
    }
}

const FORWARDING_LISTENER_TAG: &str = "ForwardingListener:";
const FORWARDING_LISTENER_FN_ON_RECEIVE_TAG: &str = "on_receive():";
const FORWARDING_LISTENER_FN_ON_ERROR_TAG: &str = "on_error():";

#[derive(Clone)]
pub(crate) struct ForwardingListener {
    forwarding_id: String,
    sender: Sender<Arc<UMessage>>,
}

impl ForwardingListener {
    pub(crate) async fn new(forwarding_id: &str, sender: Sender<Arc<UMessage>>) -> Self {
        Self {
            forwarding_id: forwarding_id.to_string(),
            sender,
        }
    }
}

#[async_trait]
impl UListener for ForwardingListener {
    async fn on_receive(&self, msg: UMessage) {
        debug!(
            "{}:{}:{} Received message: {:?}",
            self.forwarding_id,
            FORWARDING_LISTENER_TAG,
            FORWARDING_LISTENER_FN_ON_RECEIVE_TAG,
            &msg
        );
        if let Err(e) = self.sender.send(Arc::new(msg)).await {
            error!(
                "{}:{}:{} Unable to send message to worker pool: {e:?}",
                self.forwarding_id, FORWARDING_LISTENER_TAG, FORWARDING_LISTENER_FN_ON_RECEIVE_TAG,
            );
        }
    }

    async fn on_error(&self, err: UStatus) {
        error!(
            "{}:{}:{} Received error instead of message from UTransport, with error: {err:?}",
            self.forwarding_id, FORWARDING_LISTENER_TAG, FORWARDING_LISTENER_FN_ON_ERROR_TAG
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::{Endpoint, UStreamer};
    use async_trait::async_trait;
    use std::sync::Arc;
    use up_rust::{Number, UAuthority, UListener, UMessage, UStatus, UTransport, UUri};

    pub struct UPClientFoo;

    #[async_trait]
    impl UTransport for UPClientFoo {
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            println!("UPClientFoo: registering topic: {:?}", topic);
            Ok(())
        }

        async fn unregister_listener(
            &self,
            topic: UUri,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            println!("UPClientFoo: unregistering topic: {topic:?}");
            Ok(())
        }
    }

    pub struct UPClientBar;

    #[async_trait]
    impl UTransport for UPClientBar {
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            println!("UPClientBar: registering topic: {:?}", topic);
            Ok(())
        }

        async fn unregister_listener(
            &self,
            topic: UUri,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            println!("UPClientBar: unregistering topic: {topic:?}");
            Ok(())
        }
    }

    #[async_std::test]
    async fn test_simple_with_a_single_input_and_output_endpoint() {
        // Local endpoint
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_transport: Arc<dyn UTransport> = Arc::new(UPClientFoo);
        let local_endpoint = Endpoint::new(
            "local_endpoint",
            local_authority.clone(),
            local_transport.clone(),
        );

        // A remote endpoint
        let remote_authority = UAuthority {
            name: Some("remote".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
        let remote_transport: Arc<dyn UTransport> = Arc::new(UPClientBar);
        let remote_endpoint = Endpoint::new(
            "remote_endpoint",
            remote_authority.clone(),
            remote_transport.clone(),
        );

        let ustreamer = UStreamer::new("foo_bar_streamer", 100);
        // Add forwarding rules to endpoint local<->remote
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint.clone(), local_endpoint.clone())
            .await
            .is_ok());

        // Add forwarding rules to endpoint local<->local, should report an error
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), local_endpoint.clone())
            .await
            .is_err());

        // Rule already exists so it should report an error
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
            .await
            .is_err());

        // Add forwarding rules to endpoint remote<->remote, should report an error
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint.clone(), remote_endpoint.clone())
            .await
            .is_err());

        // remove valid routing rules
        assert!(ustreamer
            .delete_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .delete_forwarding_rule(remote_endpoint.clone(), local_endpoint.clone())
            .await
            .is_ok());

        // Try and remove a rule that doesn't exist, should report an error
        assert!(ustreamer
            .delete_forwarding_rule(local_endpoint.clone(), remote_endpoint.clone())
            .await
            .is_err());
    }

    #[async_std::test]
    async fn test_advanced_where_there_is_a_local_endpoint_and_two_remote_endpoints() {
        // Local endpoint
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_transport: Arc<dyn UTransport> = Arc::new(UPClientFoo);
        let local_endpoint = Endpoint::new(
            "local_endpoint",
            local_authority.clone(),
            local_transport.clone(),
        );

        // Remote endpoint - A
        let remote_authority_a = UAuthority {
            name: Some("remote_a".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
        let remote_transport_a: Arc<dyn UTransport> = Arc::new(UPClientBar);
        let remote_endpoint_a = Endpoint::new(
            "remote_endpoint_a",
            remote_authority_a.clone(),
            remote_transport_a.clone(),
        );

        // Remote endpoint - B
        let remote_authority_b = UAuthority {
            name: Some("remote_b".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 201])),
            ..Default::default()
        };
        let remote_transport_b: Arc<dyn UTransport> = Arc::new(UPClientBar);
        let remote_endpoint_b = Endpoint::new(
            "remote_endpoint_b",
            remote_authority_b.clone(),
            remote_transport_b.clone(),
        );

        let ustreamer = UStreamer::new("foo_bar_streamer", 100);

        // Add forwarding rules to endpoint local<->remote_a
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), remote_endpoint_a.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint_a.clone(), local_endpoint.clone())
            .await
            .is_ok());

        // Add forwarding rules to endpoint local<->remote_b
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), remote_endpoint_b.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint_b.clone(), local_endpoint.clone())
            .await
            .is_ok());

        // Add forwarding rules to endpoint remote_a<->remote_b
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint_a.clone(), remote_endpoint_b.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint_b.clone(), remote_endpoint_a.clone())
            .await
            .is_ok());
    }

    #[async_std::test]
    async fn test_advanced_where_there_is_a_local_endpoint_and_two_remote_endpoints_but_the_remote_endpoints_have_the_same_instance_of_utransport(
    ) {
        // Local endpoint
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_transport: Arc<dyn UTransport> = Arc::new(UPClientFoo);
        let local_endpoint = Endpoint::new(
            "local_endpoint",
            local_authority.clone(),
            local_transport.clone(),
        );

        let remote_transport: Arc<dyn UTransport> = Arc::new(UPClientBar);

        // Remote endpoint - A
        let remote_authority_a = UAuthority {
            name: Some("remote_a".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
        let remote_endpoint_a = Endpoint::new(
            "remote_endpoint_a",
            remote_authority_a.clone(),
            remote_transport.clone(),
        );

        // Remote endpoint - B
        let remote_authority_b = UAuthority {
            name: Some("remote_b".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 201])),
            ..Default::default()
        };
        let remote_endpoint_b = Endpoint::new(
            "remote_endpoint_b",
            remote_authority_b.clone(),
            remote_transport.clone(),
        );

        let ustreamer = UStreamer::new("foo_bar_streamer", 100);

        // Add forwarding rules to endpoint local<->remote_a
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), remote_endpoint_a.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint_a.clone(), local_endpoint.clone())
            .await
            .is_ok());

        // Add forwarding rules to endpoint local<->remote_b
        assert!(ustreamer
            .add_forwarding_rule(local_endpoint.clone(), remote_endpoint_b.clone())
            .await
            .is_ok());
        assert!(ustreamer
            .add_forwarding_rule(remote_endpoint_b.clone(), local_endpoint.clone())
            .await
            .is_ok());
    }
}
