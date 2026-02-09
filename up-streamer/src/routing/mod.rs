//! Routing and subscription-resolution layer.
//!
//! Encapsulates publish-source derivation, wildcard authority handling, and dedupe
//! policy used when converting subscription directory state into listener filters.
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
//! # let mut streamer = UStreamer::new("routing-doc", 16, usubscription).unwrap();
//! let ingress = Endpoint::new("ingress", "authority-a", left_transport);
//! let egress = Endpoint::new("egress", "authority-b", right_transport);
//!
//! // Routing policy resolves publish filters from subscription directory state.
//! streamer.add_route(ingress, egress).await.unwrap();
//! ```

pub(crate) mod authority_filter;
pub(crate) mod publish_resolution;
pub(crate) mod subscription_directory;
