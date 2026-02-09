//! Data-plane layer.
//!
//! Owns ingress listener registration/unregistration and egress worker pooling.
//! This layer translates resolved route policy into concrete ingress/egress
//! dispatch execution paths.
//!
//! ```ignore
//! use std::sync::Arc;
//! use up_rust::UTransport;
//! use up_streamer::{Endpoint, UStreamer};
//! use usubscription_static_file::USubscriptionStaticFile;
//!
//! # let left_transport: Arc<dyn UTransport> = todo!();
//! # let right_transport: Arc<dyn UTransport> = todo!();
//! # let usubscription = Arc::new(USubscriptionStaticFile::new(String::new()));
//! # let mut streamer = UStreamer::new("data-plane-doc", 16, usubscription).unwrap();
//! let ingress = Endpoint::new("ingress", "authority-a", left_transport);
//! let egress = Endpoint::new("egress", "authority-b", right_transport);
//!
//! // Registering and deleting routes creates/drops ingress listeners and egress workers.
//! streamer.add_route(ingress.clone(), egress.clone()).await.unwrap();
//! streamer.delete_route(ingress, egress).await.unwrap();
//! ```

pub(crate) mod egress_pool;
pub(crate) mod egress_worker;
pub(crate) mod ingress_listener;
pub(crate) mod ingress_registry;
