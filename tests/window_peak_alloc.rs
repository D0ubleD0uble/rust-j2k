//! A small window of an image over the sample limit holds the tiles it reads
//! in heap, not the image: a 256x256 window inside one 4096x4096 tile of the
//! flat 16385x16385 image (`tests/fixtures/sample_limit/`), whose whole
//! decode would hold 1 GiB of samples, peaks within what that one tile's
//! samples admit.
//!
//! The peak is measured with a counting global allocator, so this file holds a
//! single test: a second one running in parallel would count into the same
//! peak.
//!
//! A global allocator can only be written with `unsafe`. This one is a shim in
//! a test binary that forwards every call to `System` unchanged, which the
//! project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use rust_j2k::{DecodeOptions, Window, decode_with};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(size: usize) {
    let now = CURRENT.fetch_add(size, Ordering::SeqCst) + size;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

// SAFETY: every call forwards to `System` unchanged; the counters only
// observe the sizes.
unsafe impl GlobalAlloc for Counting {
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
        if new_size > layout.size() {
            grow(new_size - layout.size());
        } else {
            CURRENT.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
        }
        // SAFETY: the caller's contract for `realloc` is passed through.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn a_small_window_peaks_at_the_tile_it_reads_not_the_image() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/sample_limit/flat_16385_tiled_lossless.j2k"
    );
    let bytes = std::fs::read(path).expect("read the fixture");
    let options = DecodeOptions::default().with_window(Some(Window::new(1000, 1000, 1256, 1256)));
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let image = decode_with(&bytes, &options).expect("the window decodes");
    let peak = PEAK.load(Ordering::SeqCst) - before;
    assert_eq!((image.width, image.height), (256, 256));
    // One tile's 4-byte samples, and the whole image's.
    let tile = 4096 * 4096 * 4;
    let image_bytes = 16385 * 16385 * 4;
    eprintln!(
        "peak {peak} bytes: {:.2} times one tile's samples, {:.3} times the image's",
        peak as f64 / tile as f64,
        peak as f64 / image_bytes as f64
    );
    assert!(peak < tile * 3 / 2, "peak {peak} bytes");
}
