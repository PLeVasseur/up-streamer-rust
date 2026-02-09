//! Routing and subscription-resolution layer.
//!
//! Encapsulates publish-source derivation, wildcard authority handling, and dedupe
//! policy used when converting subscription directory state into listener filters.
//!
//! ```ignore
//! use up_streamer::UStreamer;
//!
//! // Routing logic computes publish filters from subscription cache state.
//! # let _ = UStreamer::add_forwarding_rule;
//! ```

pub(crate) mod authority_filter;
pub(crate) mod publish_resolution;
pub(crate) mod subscription_directory;
