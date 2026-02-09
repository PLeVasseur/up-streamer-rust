//! API facade layer.
//!
//! This layer keeps outward usage streamer-centric while delegating internals to
//! domain-focused modules.
//!
//! ```ignore
//! use std::sync::Arc;
//! use up_rust::core::usubscription::USubscription;
//! use up_streamer::UStreamer;
//!
//! # let usubscription: Arc<dyn USubscription> = todo!("inject implementation");
//! let _streamer = UStreamer::new("bridge", 32, usubscription)?;
//! # Ok::<(), up_rust::UStatus>(())
//! ```

pub mod endpoint;
pub mod streamer;
