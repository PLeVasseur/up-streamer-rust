//! Ingress listener registry and lifecycle management.

use crate::data_plane::ingress_listener::ForwardingListener;
use crate::control_plane::transport_identity::TransportIdentityKey;
use crate::routing::publish_resolution::derive_publish_source_filters;
use crate::routing::subscription_directory::resolve_subscribers_for_authority;
use crate::ustreamer::uauthority_to_uuri;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use subscription_cache::SubscriptionCache;
use tokio::sync::broadcast::Sender;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use up_rust::{UMessage, UTransport, UUri};

const FORWARDING_LISTENERS_TAG: &str = "ForwardingListeners:";
const FORWARDING_LISTENERS_FN_INSERT_TAG: &str = "insert:";
const FORWARDING_LISTENERS_FN_REMOVE_TAG: &str = "remove:";

pub enum ForwardingListenerError {
    FailToRegisterNotificationRequestResponseListener,
    FailToRegisterPublishListener(UUri),
}

impl Debug for ForwardingListenerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ForwardingListenerError::FailToRegisterNotificationRequestResponseListener => {
                write!(f, "FailToRegisterNotificationRequestResponseListener")
            }
            ForwardingListenerError::FailToRegisterPublishListener(uri) => {
                write!(f, "FailToRegisterPublishListener({:?})", uri)
            }
        }
    }
}

impl Display for ForwardingListenerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ForwardingListenerError::FailToRegisterNotificationRequestResponseListener => {
                write!(
                    f,
                    "Failed to register notification request/response listener"
                )
            }
            ForwardingListenerError::FailToRegisterPublishListener(uri) => {
                write!(f, "Failed to register publish listener for URI: {}", uri)
            }
        }
    }
}

impl Error for ForwardingListenerError {}

type ForwardingListenersContainer =
    Mutex<HashMap<(TransportIdentityKey, String, String), (usize, Arc<ForwardingListener>)>>;

pub(crate) struct ForwardingListeners {
    listeners: ForwardingListenersContainer,
}

impl ForwardingListeners {
    pub(crate) fn new() -> Self {
        Self {
            listeners: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn insert(
        &self,
        in_transport: Arc<dyn UTransport>,
        in_authority: &str,
        out_authority: &str,
        forwarding_id: &str,
        out_sender: Sender<Arc<UMessage>>,
        subscription_cache: Arc<Mutex<SubscriptionCache>>,
    ) -> Result<Option<Arc<ForwardingListener>>, ForwardingListenerError> {
        let in_transport_key = TransportIdentityKey::new(in_transport.clone());
        let mut forwarding_listeners = self.listeners.lock().await;

        if let Some((active, forwarding_listener)) = forwarding_listeners.get_mut(&(
            in_transport_key.clone(),
            in_authority.to_string(),
            out_authority.to_string(),
        )) {
            *active += 1;
            if *active > 1 {
                return Ok(None);
            }
            return Ok(Some(forwarding_listener.clone()));
        }

        let forwarding_listener =
            Arc::new(ForwardingListener::new(forwarding_id, out_sender.clone()));

        type SourceSinkFilterPair = (UUri, Option<UUri>);
        #[allow(clippy::mutable_key_type)]
        let mut uuris_to_backpedal: HashSet<SourceSinkFilterPair> = HashSet::new();

        let request_source_filter = uauthority_to_uuri(in_authority);
        let request_sink_filter = uauthority_to_uuri(out_authority);

        uuris_to_backpedal.insert((
            request_source_filter.clone(),
            Some(request_sink_filter.clone()),
        ));

        if let Err(err) = in_transport
            .register_listener(
                &request_source_filter,
                Some(&request_sink_filter),
                forwarding_listener.clone(),
            )
            .await
        {
            warn!(
                "{}:{} unable to register request listener, error: {}",
                FORWARDING_LISTENERS_TAG, FORWARDING_LISTENERS_FN_INSERT_TAG, err
            );
            for uuri_pair in &uuris_to_backpedal {
                if let Err(err) = in_transport
                    .unregister_listener(
                        &uuri_pair.0,
                        uuri_pair.1.as_ref(),
                        forwarding_listener.clone(),
                    )
                    .await
                {
                    warn!(
                        "{}:{} unable to unregister listener, error: {}",
                        FORWARDING_LISTENERS_TAG, FORWARDING_LISTENERS_FN_INSERT_TAG, err
                    );
                };
            }
            return Err(ForwardingListenerError::FailToRegisterNotificationRequestResponseListener);
        }

        debug!(
            "{}:{} able to register request listener",
            FORWARDING_LISTENERS_TAG, FORWARDING_LISTENERS_FN_INSERT_TAG
        );

        #[allow(clippy::mutable_key_type)]
        let subscribers = resolve_subscribers_for_authority(
            &subscription_cache,
            out_authority,
            FORWARDING_LISTENERS_TAG,
            FORWARDING_LISTENERS_FN_INSERT_TAG,
        )
        .await;

        #[allow(clippy::mutable_key_type)]
        let publish_source_filters = derive_publish_source_filters(
            in_authority,
            out_authority,
            &subscribers,
            FORWARDING_LISTENERS_TAG,
            FORWARDING_LISTENERS_FN_INSERT_TAG,
        );

        for source_uri in publish_source_filters {
            info!(
                "in authority: {}, out authority: {}, source URI filter: {:?}",
                in_authority, out_authority, source_uri
            );

            if let Err(err) = in_transport
                .register_listener(&source_uri, None, forwarding_listener.clone())
                .await
            {
                warn!(
                    "{}:{} unable to register listener, error: {}",
                    FORWARDING_LISTENERS_TAG, FORWARDING_LISTENERS_FN_INSERT_TAG, err
                );
                for uuri_pair in &uuris_to_backpedal {
                    if let Err(err) = in_transport
                        .unregister_listener(
                            &uuri_pair.0,
                            uuri_pair.1.as_ref(),
                            forwarding_listener.clone(),
                        )
                        .await
                    {
                        warn!(
                            "{}:{} unable to unregister listener, error: {}",
                            FORWARDING_LISTENERS_TAG, FORWARDING_LISTENERS_FN_INSERT_TAG, err
                        );
                    };
                }
                return Err(ForwardingListenerError::FailToRegisterPublishListener(
                    source_uri,
                ));
            }

            uuris_to_backpedal.insert((source_uri, None));
            debug!("{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_INSERT_TAG} able to register listener");
        }

        forwarding_listeners.insert(
            (
                in_transport_key,
                in_authority.to_string(),
                out_authority.to_string(),
            ),
            (1, forwarding_listener.clone()),
        );
        Ok(Some(forwarding_listener))
    }

    pub(crate) async fn remove(
        &self,
        in_transport: Arc<dyn UTransport>,
        in_authority: &str,
        out_authority: &str,
        subscription_cache: Arc<Mutex<SubscriptionCache>>,
    ) {
        let in_transport_key = TransportIdentityKey::new(in_transport.clone());

        let mut forwarding_listeners = self.listeners.lock().await;

        let active_num = {
            let Some((active, _)) = forwarding_listeners.get_mut(&(
                in_transport_key.clone(),
                in_authority.to_string(),
                out_authority.to_string(),
            )) else {
                warn!(
                    "{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_REMOVE_TAG} no such in_transport_key, out_authority: {out_authority:?}"
                );
                return;
            };
            *active -= 1;
            *active
        };

        if active_num == 0 {
            let removed = forwarding_listeners.remove(&(
                in_transport_key,
                in_authority.to_string(),
                out_authority.to_string(),
            ));
            if let Some((_, forwarding_listener)) = removed {
                let request_source_filter = uauthority_to_uuri(in_authority);
                let request_sink_filter = uauthority_to_uuri(out_authority);

                let request_unreg_res = in_transport
                    .unregister_listener(
                        &request_source_filter,
                        Some(&request_sink_filter),
                        forwarding_listener.clone(),
                    )
                    .await;

                if let Err(err) = request_unreg_res {
                    warn!("{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_REMOVE_TAG} unable to unregister request listener, error: {err}");
                } else {
                    debug!("{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_REMOVE_TAG} able to unregister request listener");
                }

                #[allow(clippy::mutable_key_type)]
                let subscribers = resolve_subscribers_for_authority(
                    &subscription_cache,
                    out_authority,
                    FORWARDING_LISTENERS_TAG,
                    FORWARDING_LISTENERS_FN_REMOVE_TAG,
                )
                .await;

                #[allow(clippy::mutable_key_type)]
                let publish_source_filters = derive_publish_source_filters(
                    in_authority,
                    out_authority,
                    &subscribers,
                    FORWARDING_LISTENERS_TAG,
                    FORWARDING_LISTENERS_FN_REMOVE_TAG,
                );

                for source_uri in publish_source_filters {
                    if let Err(err) = in_transport
                        .unregister_listener(&source_uri, None, forwarding_listener.clone())
                        .await
                    {
                        warn!("{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_REMOVE_TAG} unable to unregister publish listener, error: {err}");
                    } else {
                        debug!("{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_REMOVE_TAG} able to unregister publish listener");
                    }
                }
            } else {
                warn!("{FORWARDING_LISTENERS_TAG}:{FORWARDING_LISTENERS_FN_REMOVE_TAG} none found we can remove, out_authority: {out_authority:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ForwardingListeners;
    use crate::ustreamer::uauthority_to_uuri;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex as StdMutex};
    use subscription_cache::SubscriptionCache;
    use tokio::sync::Mutex;
    use up_rust::core::usubscription::{FetchSubscriptionsResponse, SubscriberInfo, Subscription};
    use up_rust::{UCode, UListener, UMessage, UStatus, UTransport, UUri};

    #[derive(Clone, Debug, Eq, PartialEq, Hash)]
    struct ListenerRegistration {
        source_filter: UUri,
        sink_filter: Option<UUri>,
    }

    fn listener_registration(
        source_filter: &UUri,
        sink_filter: Option<&UUri>,
    ) -> ListenerRegistration {
        ListenerRegistration {
            source_filter: source_filter.clone(),
            sink_filter: sink_filter.cloned(),
        }
    }

    #[derive(Default)]
    struct RecordingTransport {
        register_call_counts: StdMutex<HashMap<ListenerRegistration, usize>>,
        unregister_call_counts: StdMutex<HashMap<ListenerRegistration, usize>>,
    }

    impl RecordingTransport {
        fn register_call_count(&self, source_filter: &UUri, sink_filter: Option<&UUri>) -> usize {
            self.register_call_counts
                .lock()
                .expect("lock register_call_counts")
                .get(&listener_registration(source_filter, sink_filter))
                .copied()
                .unwrap_or(0)
        }

        fn unregister_call_count(&self, source_filter: &UUri, sink_filter: Option<&UUri>) -> usize {
            self.unregister_call_counts
                .lock()
                .expect("lock unregister_call_counts")
                .get(&listener_registration(source_filter, sink_filter))
                .copied()
                .unwrap_or(0)
        }
    }

    #[async_trait]
    impl UTransport for RecordingTransport {
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            Ok(())
        }

        async fn receive(
            &self,
            _source_filter: &UUri,
            _sink_filter: Option<&UUri>,
        ) -> Result<UMessage, UStatus> {
            Err(UStatus::fail_with_code(
                UCode::UNIMPLEMENTED,
                "not used in tests",
            ))
        }

        async fn register_listener(
            &self,
            source_filter: &UUri,
            sink_filter: Option<&UUri>,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            let registration = listener_registration(source_filter, sink_filter);
            let mut counts = self
                .register_call_counts
                .lock()
                .expect("lock register_call_counts");
            let entry = counts.entry(registration).or_insert(0);
            *entry += 1;
            Ok(())
        }

        async fn unregister_listener(
            &self,
            source_filter: &UUri,
            sink_filter: Option<&UUri>,
            _listener: Arc<dyn UListener>,
        ) -> Result<(), UStatus> {
            let registration = listener_registration(source_filter, sink_filter);
            let mut counts = self
                .unregister_call_counts
                .lock()
                .expect("lock unregister_call_counts");
            let entry = counts.entry(registration).or_insert(0);
            *entry += 1;
            Ok(())
        }
    }

    fn make_subscription_cache(entries: &[(&str, &str)]) -> Arc<Mutex<SubscriptionCache>> {
        let subscriptions = entries
            .iter()
            .map(|(topic, subscriber)| Subscription {
                topic: Some(UUri::from_str(topic).expect("valid topic UUri")).into(),
                subscriber: Some(SubscriberInfo {
                    uri: Some(UUri::from_str(subscriber).expect("valid subscriber UUri")).into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .collect();

        Arc::new(Mutex::new(
            SubscriptionCache::new(FetchSubscriptionsResponse {
                subscriptions,
                ..Default::default()
            })
            .expect("valid subscription cache"),
        ))
    }

    #[tokio::test]
    async fn insert_and_remove_registers_and_unregisters_request_and_publish_filters() {
        let forwarding_listeners = ForwardingListeners::new();
        let recording_transport = Arc::new(RecordingTransport::default());
        let in_transport: Arc<dyn UTransport> = recording_transport.clone();
        let (out_sender, _) = tokio::sync::broadcast::channel(16);
        let subscription_cache =
            make_subscription_cache(&[("//authority-a/5BA0/1/8001", "//authority-b/5678/1/1234")]);

        assert!(forwarding_listeners
            .insert(
                in_transport.clone(),
                "authority-a",
                "authority-b",
                "test-forwarding",
                out_sender,
                subscription_cache.clone(),
            )
            .await
            .is_ok());

        forwarding_listeners
            .remove(
                in_transport,
                "authority-a",
                "authority-b",
                subscription_cache,
            )
            .await;

        let request_source = uauthority_to_uuri("authority-a");
        let request_sink = uauthority_to_uuri("authority-b");
        let publish_source =
            UUri::try_from_parts("authority-a", 0x5BA0, 0x1, 0x8001).expect("valid publish source");

        assert_eq!(
            recording_transport.register_call_count(&request_source, Some(&request_sink)),
            1
        );
        assert_eq!(
            recording_transport.register_call_count(&publish_source, None),
            1
        );
        assert_eq!(
            recording_transport.unregister_call_count(&request_source, Some(&request_sink)),
            1
        );
        assert_eq!(
            recording_transport.unregister_call_count(&publish_source, None),
            1
        );
    }

    #[tokio::test]
    async fn duplicate_insert_for_same_route_keeps_single_listener_registration() {
        let forwarding_listeners = ForwardingListeners::new();
        let recording_transport = Arc::new(RecordingTransport::default());
        let in_transport: Arc<dyn UTransport> = recording_transport.clone();
        let (out_sender, _) = tokio::sync::broadcast::channel(16);
        let subscription_cache =
            make_subscription_cache(&[("//authority-a/5BA0/1/8001", "//authority-b/5678/1/1234")]);

        let first_insert = forwarding_listeners
            .insert(
                in_transport.clone(),
                "authority-a",
                "authority-b",
                "test-forwarding",
                out_sender.clone(),
                subscription_cache.clone(),
            )
            .await
            .expect("first insert success");
        let second_insert = forwarding_listeners
            .insert(
                in_transport,
                "authority-a",
                "authority-b",
                "test-forwarding",
                out_sender,
                subscription_cache,
            )
            .await
            .expect("second insert success");

        assert!(first_insert.is_some());
        assert!(second_insert.is_none());

        let request_source = uauthority_to_uuri("authority-a");
        let request_sink = uauthority_to_uuri("authority-b");
        let publish_source =
            UUri::try_from_parts("authority-a", 0x5BA0, 0x1, 0x8001).expect("valid publish source");

        assert_eq!(
            recording_transport.register_call_count(&request_source, Some(&request_sink)),
            1
        );
        assert_eq!(
            recording_transport.register_call_count(&publish_source, None),
            1
        );
    }
}
