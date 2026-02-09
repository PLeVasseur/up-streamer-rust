//! Runtime integration layer.
//!
//! Isolates subscription bootstrap and worker runtime boundaries so async/threading
//! behavior remains localized and predictable for the rest of the crate.
//!
//! ```ignore
//! use up_streamer::UStreamer;
//!
//! // Runtime adapters isolate blocking/subscriber bootstrap details from API users.
//! # let _ = UStreamer::new;
//! ```

pub(crate) mod subscription_runtime;
pub(crate) mod worker_runtime;
