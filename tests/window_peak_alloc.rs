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
//! A global allocator can only be written with `unsafe`. The shim in
//! `tests/support/alloc.rs` forwards every call to `System` unchanged, which
//! the project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

#[path = "support/alloc.rs"]
mod alloc;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, Window, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

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
