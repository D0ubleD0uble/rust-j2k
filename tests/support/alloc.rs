//! Allocator shims for the test binaries that count allocations. Std only.
//! Each binary installs one with
//! `#[global_allocator] static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;`.
//! This file holds the only `unsafe` in the repository.

#![allow(dead_code)]

// Each shim forwards every call to `System` unchanged and only observes the
// sizes, so the contract the caller upholds is the contract `System` needs.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bytes live now, for [`PeakAlloc`].
pub static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// The high-water mark of [`CURRENT`]; a test resets it with a store.
pub static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Allocations made, for [`CountAlloc`].
pub static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

fn grow(size: usize) {
    let now = CURRENT.fetch_add(size, Ordering::SeqCst) + size;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

/// Tracks live bytes in [`CURRENT`] and their peak in [`PEAK`]. A `realloc`
/// counts as the new block allocated before the old one is freed.
pub struct PeakAlloc;

// SAFETY: every call forwards to `System` unchanged; the counters only
// observe the sizes.
unsafe impl GlobalAlloc for PeakAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        // SAFETY: the caller's contract for `alloc_zeroed` is passed through.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller's contract for `dealloc` is passed through.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        grow(new_size);
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller's contract for `realloc` is passed through.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Counts allocations in [`ALLOCATIONS`].
pub struct CountAlloc;

// SAFETY: every call forwards to `System` unchanged; the counter only
// observes the calls.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the caller's contract for `alloc` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract for `dealloc` is passed through.
        unsafe { System.dealloc(ptr, layout) }
    }
}
