//! Control-plane layer.
//!
//! Owns route-registration lifecycle semantics and the route-table identity model.
//! This layer is responsible for idempotent insert/remove behavior and rollback-safe
//! transitions when listener registration fails.
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
//! # let mut streamer = UStreamer::new("control-plane-doc", 16, usubscription).unwrap();
//! let left = Endpoint::new("left", "left-authority", left_transport);
//! let right = Endpoint::new("right", "right-authority", right_transport);
//!
//! // The control plane ensures duplicate insert/remove transitions stay idempotent.
//! streamer.add_route(left.clone(), right.clone()).await.unwrap();
//! assert!(streamer.add_route(left.clone(), right.clone()).await.is_err());
//! streamer.delete_route(left.clone(), right.clone()).await.unwrap();
//! assert!(streamer.delete_route(left, right).await.is_err());
//! ```

pub(crate) mod route_lifecycle;
pub(crate) mod route_table;
pub(crate) mod transport_identity;
