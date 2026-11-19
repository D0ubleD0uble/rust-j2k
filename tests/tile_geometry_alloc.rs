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
/// output: `docs/resource-bounds.md` gives about 80 bytes, measured.
const BYTES_PER_BLOCK: usize = 96;

/// The most heap a precinct of one empty code-block may take at its peak,
/// over the output, its code-block included: about 260 bytes, measured.
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
    // 2^21 code-blocks of 4x4, the cap under `AtMost(2^30)`.
    let n = 1 << 30;
    let (width, height) = (4096, 8192);
    let blocks = (width / 4) * (height / 4);
    assert_eq!(blocks as u64, n / 512);
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
    // `AtMost(2^32)`.
    let n = 1 << 32;
    let (width, height) = (1024u32, 2048u32);
    let precincts = (width * height) as usize;
    assert_eq!(precincts as u64, n / 2048);
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
