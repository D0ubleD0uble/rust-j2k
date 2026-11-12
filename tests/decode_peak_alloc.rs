//! A single-tile decode holds at most about twice its output in heap, not
//! three times: the tile's raster becomes the image plane instead of being
//! copied into a second one, and each wavelet level's subbands are freed as
//! they are merged.
//!
//! The image is the shape of `peak_97.j2k` from
//! `scripts/gen-peak-memory-shapes.py` at 1024x1024: one component, one tile,
//! the 9/7 wavelet with 5 levels, and an empty packet per resolution, so the
//! decode's cost is its buffers alone. The 5/3 form is checked too.
//!
//! This counts the bytes allocated, not the pages the operating system maps.
//! The resident peak is lower: a fresh zeroed buffer maps its pages only as
//! they are written (see `merge_level` in src/dwt.rs). The `MAX_IMAGE_SAMPLES`
//! rustdoc records the resident peaks.
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

use rust_j2k::decode;

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
        // Counted as the new block allocated before the old one is freed.
        grow(new_size);
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the caller's contract for `realloc` is passed through.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const SIDE: u32 = 1024;
const LEVELS: u8 = 5;

/// A marker segment: the code, the length, the body.
fn seg(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = code.to_be_bytes().to_vec();
    out.extend_from_slice(&(2 + body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// One `SIDE` x `SIDE` 8-bit component in one tile, LRCP, one layer,
/// `LEVELS` levels of 64x64 code-blocks, on the 9/7 wavelet (expounded steps
/// of exponent 8) or the 5/3 (no quantization); every packet empty.
fn single_tile(irreversible: bool) -> Vec<u8> {
    let mut siz = 0u16.to_be_bytes().to_vec();
    for v in [SIDE, SIDE, 0, 0, SIDE, SIDE, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&1u16.to_be_bytes());
    siz.extend_from_slice(&[7, 1, 1]);
    let bands = 1 + 3 * usize::from(LEVELS);
    let qcd = if irreversible {
        let mut body = vec![0x42];
        for _ in 0..bands {
            body.extend_from_slice(&(8u16 << 11).to_be_bytes());
        }
        body
    } else {
        let mut body = vec![0x40];
        body.extend(std::iter::repeat_n(8 << 3, bands));
        body
    };
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(
        0xFF52,
        &[0, 0, 0, 1, 0, LEVELS, 4, 4, 0, u8::from(!irreversible)],
    ));
    cs.extend(seg(0xFF5C, &qcd));
    let packets = usize::from(LEVELS) + 1;
    let psot = 12 + 2 + packets as u32;
    cs.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    cs.extend_from_slice(&psot.to_be_bytes());
    cs.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    cs.extend(std::iter::repeat_n(0, packets));
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// The heap `decode` holds at its peak on `bytes`, over what was live before.
fn peak(bytes: &[u8]) -> usize {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let image = decode(bytes).expect("the flat image decodes");
    let samples = &image.components[0].samples;
    assert_eq!(samples.len(), (SIDE * SIDE) as usize);
    assert!(
        samples.iter().all(|&v| v == 128),
        "a flat image at mid-gray"
    );
    PEAK.load(Ordering::SeqCst) - before
}

/// The output is `SIDE^2` `i32` samples. At the finest level the decode holds
/// that level's subbands (one output's worth) and the grid they merge into
/// (another), so the peak is about twice the output. Copying the tile into a
/// separately allocated image plane added a third output's worth, and keeping
/// every level's subbands until the end added another quarter.
#[test]
fn single_tile_decode_peaks_near_twice_the_output() {
    let output = (SIDE * SIDE) as usize * size_of::<i32>();
    for irreversible in [true, false] {
        let peak = peak(&single_tile(irreversible));
        let ratio = peak as f64 / output as f64;
        assert!(
            ratio < 2.1,
            "irreversible {irreversible}: peak {peak} bytes is {ratio:.2} times the \
             {output}-byte output"
        );
    }
}
