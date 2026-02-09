//! Control-plane layer.
//!
//! Owns forwarding-rule lifecycle semantics and the route-table identity model.
//! This layer is responsible for idempotent insert/remove behavior and rollback-safe
//! transitions when listener registration fails.
//!
//! ```ignore
//! use up_streamer::Endpoint;
//!
//! // Control-plane operations apply the same tuple identity for add/remove.
//! # let _ = Endpoint::new;
//! ```

pub(crate) mod route_lifecycle;
pub(crate) mod route_table;
