// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Rust System allocation positive controls, with first operations on new threads.
use std::ffi::{c_char, c_void};
#[link(name = "dl")]
unsafe extern "C" { fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void; }
type Replace = unsafe extern "C" fn(u32) -> u32;
fn exercise(scope: Replace) {
    // SAFETY: the exact versioned calibration interposer exposes this scalar
    // thread-local flag operation and remains loaded for the process lifetime.
    let previous = unsafe { scope(1) };
    let mut bytes = Vec::<u8>::with_capacity(127);
    bytes.resize(127, 17);
    std::hint::black_box(&bytes);
    bytes.reserve(4096);
    std::hint::black_box(&bytes);
    let zeros = vec![0u64; 73];
    std::hint::black_box(&zeros);
    drop((bytes, zeros));
    // SAFETY: restore the previous scalar flag on this same thread.
    unsafe { scope(previous); }
}
fn main() {
    // SAFETY: process-local static symbol, checked before converting to its
    // known C signature. The explicitly launched interposer is never unloaded.
    let scope: Replace = unsafe {
        let symbol = dlsym(std::ptr::null_mut(), c"up_flow_alloc_replace_scope".as_ptr());
        assert!(!symbol.is_null());
        std::mem::transmute::<*mut c_void, Replace>(symbol)
    };
    let workers: Vec<_> = (0..4).map(|_| std::thread::spawn(move || exercise(scope))).collect();
    for worker in workers { worker.join().unwrap(); }
    exercise(scope);
}
