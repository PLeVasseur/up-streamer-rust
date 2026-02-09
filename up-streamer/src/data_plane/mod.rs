//! Data-plane layer.
//!
//! Owns ingress listener registration/unregistration and egress worker pooling.
//! This layer translates resolved route policy into concrete message forwarding
//! execution paths.
//!
//! ```ignore
//! use up_streamer::UStreamer;
//!
//! // Data-plane components are created/managed by forwarding-rule lifecycle changes.
//! # let _ = UStreamer::delete_forwarding_rule;
//! ```

pub(crate) mod egress_pool;
pub(crate) mod egress_worker;
pub(crate) mod ingress_listener;
pub(crate) mod ingress_registry;
