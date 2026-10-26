// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Optional test observer attachment at the shared prepared-core setup boundary.

use up_rust::{UCode, UStatus};
use up_transport_zenoh::ZenohAllocationObserver;

pub(super) fn configured() -> Result<Option<&'static dyn ZenohAllocationObserver>, UStatus> {
    if std::env::var_os("UPROTOCOL_ALLOCATION_COUNTER_FILE").is_none() {
        return Ok(None);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    return linux::configured().map(Some);
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    Err(invalid())
}

fn invalid() -> UStatus {
    UStatus::fail_with_code(
        UCode::FailedPrecondition,
        "configured allocation observer is unavailable or incompatible",
    )
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod linux {
    use super::*;
    use std::ffi::{c_char, c_void};
    use std::sync::OnceLock;

    #[link(name = "dl")]
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    type Query = unsafe extern "C" fn() -> u32;
    type Replace = unsafe extern "C" fn(u32) -> u32;
    struct Observer(Replace);
    impl ZenohAllocationObserver for Observer {
        fn replace_scope(&self, counted: bool) -> bool {
            // SAFETY: setup resolved this exact versioned, process-lifetime C
            // function. Its scalar interface stores only the calling thread's
            // flag; it retains no Rust references and performs no allocation.
            unsafe { (self.0)(u32::from(counted)) != 0 }
        }
    }
    static OBSERVER: OnceLock<Observer> = OnceLock::new();
    pub(super) fn configured() -> Result<&'static dyn ZenohAllocationObserver, UStatus> {
        if let Some(observer) = OBSERVER.get() {
            return Ok(observer);
        }
        // SAFETY: dlsym reads these terminated static symbol names from the
        // current process. No library is unloaded. Null is checked before each
        // conversion; the v1 test interposer defines these exact C signatures.
        let (version, mapped, replace) = unsafe {
            let version = dlsym(
                std::ptr::null_mut(),
                c"up_flow_alloc_observer_version".as_ptr(),
            );
            let mapped = dlsym(
                std::ptr::null_mut(),
                c"up_flow_alloc_observer_mapped".as_ptr(),
            );
            let replace = dlsym(
                std::ptr::null_mut(),
                c"up_flow_alloc_replace_scope".as_ptr(),
            );
            if version.is_null() || mapped.is_null() || replace.is_null() {
                return Err(invalid());
            }
            (
                std::mem::transmute::<*mut c_void, Query>(version),
                std::mem::transmute::<*mut c_void, Query>(mapped),
                std::mem::transmute::<*mut c_void, Replace>(replace),
            )
        };
        // SAFETY: checked versioned C query functions use no arguments or Rust
        // memory. Mapping must exist before a binding scope can be observed.
        if unsafe { version() != 1 || mapped() != 1 } {
            return Err(invalid());
        }
        Ok(OBSERVER.get_or_init(|| Observer(replace)))
    }
}
