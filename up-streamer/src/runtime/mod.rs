//! Runtime integration layer.
//!
//! Isolates subscription bootstrap and worker runtime boundaries so async/threading
//! behavior remains localized and predictable for the rest of the crate.
//!
//! ```ignore
//! // Runtime adapters are internal helpers and should not carry API/domain policy.
//! // They expose focused spawn/bootstrap utilities for internal modules only.
//! ```

pub(crate) mod subscription_runtime;
pub(crate) mod worker_runtime;
