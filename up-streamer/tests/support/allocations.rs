// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Thread-local allocation observation for current-thread SDK forwarding tests.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static COUNTS: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}

fn allocated(bytes: usize) {
    let _ = COUNTS.try_with(|counts| {
        let (active, calls, total) = counts.get();
        if active {
            counts.set((true, calls.saturating_add(1), total.saturating_add(bytes)));
        }
    });
}

/// Delegates unchanged storage behavior to System and counts observed allocation attempts.
pub struct TrackingAllocator;
// SAFETY: each operation forwards the original allocator contract and arguments
// to System. Observation uses only constant-initialized thread-local Cells.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocated(layout.size());
        // SAFETY: caller supplies the GlobalAlloc layout contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocated(layout.size());
        // SAFETY: caller supplies the GlobalAlloc layout contract unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocated(size);
        // SAFETY: pointer, old layout and new size retain the caller's contract.
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout are forwarded to their allocating System.
        unsafe { System.dealloc(pointer, layout) }
    }
}

/// Counts the caller and generic workers running on its same current-thread runtime.
pub struct Scope;
impl Scope {
    /// Starts an unnested measured window after creation-reserved setup.
    pub fn start() -> Self {
        COUNTS.with(|counts| {
            assert!(!counts.get().0, "allocation scopes cannot nest");
            counts.set((true, 0, 0));
        });
        Self
    }
    /// Ends the window and returns allocation calls and requested bytes without subtraction.
    pub fn finish(self) -> (usize, usize) {
        COUNTS.with(|counts| {
            let (_, calls, bytes) = counts.get();
            counts.set((false, calls, bytes));
            (calls, bytes)
        })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        COUNTS.with(|counts| {
            let (_, calls, bytes) = counts.get();
            counts.set((false, calls, bytes));
        });
    }
}
