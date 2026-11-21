//! A tile at the scaled caps holds about what `docs/resource-bounds.md`
//! says a code-block and a precinct cost: under `WorkLimit::AtMost(n)` a
//! tile of `n / 512` empty code-blocks, and one of `n / 2048` precincts of a
//! code-block each, decode within the documented bytes a code-block and a
//! precinct over their output. At the caps that is a small part of the
//! `4 * n` bytes of output the same limit allows.
//!
//! The files are those of `tests/tile_geometry_cap.rs`: one 8-bit component
//! in one tile, no wavelet levels, and an empty packet per precinct.
//!
//! The peak is measured with a counting global allocator, so this file holds a
//! single test: a second one running in parallel would count into the same
//! peak. It counts the bytes allocated, not the pages the operating system
//! maps.
//!
//! A global allocator can only be written with `unsafe`. The shim in
//! `tests/support/alloc.rs` forwards every call to `System` unchanged, which
//! the project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.
//!
//! On a 32-bit target the sample limit tops out at 2^29 - 1 samples, so the
//! caps stay at their floors of 2^20 code-blocks and 2^18 precincts, and the
//! test measures tiles at the floors under `AtMost(2^24)` instead of caps
//! scaled past them (`tests/tile_geometry_cap.rs` checks what it refuses
//! there).

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;

use rust_j2k::{DecodeOptions, WorkLimit, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

/// The most heap an empty code-block may take at its peak, over the
/// output: `docs/resource-bounds.md` gives 88 bytes, measured.
const BYTES_PER_BLOCK: usize = 96;

/// The most heap a precinct of one empty code-block may take at its peak,
/// over the output, its code-block included: 252 bytes, measured.
const BYTES_PER_PRECINCT: usize = 300;

/// The heap a decode of `bytes` under `AtMost(n)` holds at its peak over
/// what was live before, less its output, after checking the decode
/// returned `samples` samples.
fn peak_over_output(bytes: &[u8], n: u64, samples: usize) -> usize {
    let options = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(n));
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let image = decode_with(bytes, &options).expect("a tile at the caps decodes");
    let peak = PEAK.load(Ordering::SeqCst) - before;
    assert_eq!(image.components[0].samples.len(), samples);
    peak - samples * size_of::<i32>()
}

#[test]
fn a_tile_at_the_scaled_caps_peaks_within_the_documented_bytes() {
    // 2^21 code-blocks of 4x4, the cap under `AtMost(2^30)`. On a 32-bit
    // target 2^20 of them, the floor, in a 4096 x 4096 tile under 2^24.
    let (n, width, height): (u64, u32, u32) = if cfg!(target_pointer_width = "64") {
        (1 << 30, 4096, 8192)
    } else {
        (1 << 24, 4096, 4096)
    };
    let blocks = (width / 4) * (height / 4);
    if cfg!(target_pointer_width = "64") {
        assert_eq!(u64::from(blocks), n / 512);
    }
    let bytes = synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        0,
        0,
        &[],
        &[1],
        0,
    );
    let over = peak_over_output(&bytes, n, (width * height) as usize);
    let per_block = over as f64 / blocks as f64;
    eprintln!("{blocks} code-blocks: {over} bytes over the output, {per_block:.1} a block");
    assert!(
        over <= blocks as usize * BYTES_PER_BLOCK,
        "{per_block:.1} bytes a code-block"
    );

    // 2^21 precincts of 1x1, each with its code-block, the cap under
    // `AtMost(2^32)`. On a 32-bit target 2^18 of them, the floor, in a
    // 512 x 512 tile under 2^24.
    let (n, width, height): (u64, u32, u32) = if cfg!(target_pointer_width = "64") {
        (1 << 32, 1024, 2048)
    } else {
        (1 << 24, 512, 512)
    };
    let precincts = (width * height) as usize;
    if cfg!(target_pointer_width = "64") {
        assert_eq!(precincts as u64, n / 2048);
    }
    let bytes = synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        0,
        0,
        &[0],
        &[precincts],
        0,
    );
    let over = peak_over_output(&bytes, n, precincts);
    let per_precinct = over as f64 / precincts as f64;
    eprintln!("{precincts} precincts: {over} bytes over the output, {per_precinct:.1} a precinct");
    assert!(
        over <= precincts * BYTES_PER_PRECINCT,
        "{per_precinct:.1} bytes a precinct"
    );
}
