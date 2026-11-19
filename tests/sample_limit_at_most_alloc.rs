//! Under `WorkLimit::AtMost(n)` a decode holds about what its `n` samples
//! need: an image of exactly `n` samples decodes within the documented
//! multiple of its `4 * n` bytes of output, and one of a row more is refused
//! before anything sized by the image is allocated.
//!
//! The image is the shape of `peak_97.j2k` from
//! `scripts/gen-peak-memory-shapes.py`, as in `tests/decode_peak_alloc.rs`:
//! one component, one tile, the 9/7 wavelet with 5 levels, and an empty
//! packet per resolution, so the decode's cost is its buffers alone.
//!
//! This counts the bytes allocated, not the pages the operating system maps,
//! which are fewer (see `tests/decode_peak_alloc.rs`).
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
#[path = "support/synth.rs"]
mod synth;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;
use synth::seg;

use rust_j2k::{DecodeOptions, Error, LimitKind, WorkLimit, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

/// The ceiling: 2^24 samples, 64 MiB of `i32` output.
const CEILING: u64 = 1 << 24;
const SIDE: u32 = 4096;
const LEVELS: u8 = 5;

/// The most a decode may peak at, as a multiple of its output: the "no more
/// than about 3.2 times" the `sample_limit` rustdoc documents.
const PEAK_RATIO: f64 = 3.2;

/// One `SIDE` x `height` 8-bit component in one tile, LRCP, one layer,
/// `LEVELS` levels of 64x64 code-blocks, on the 9/7 wavelet (expounded steps
/// of exponent 8); every packet empty.
fn single_tile(height: u32) -> Vec<u8> {
    let siz = synth::siz_body([SIDE, height, 0, 0, SIDE, height, 0, 0], &[[7, 1, 1]]);
    let bands = 1 + 3 * usize::from(LEVELS);
    let mut qcd = vec![0x42];
    for _ in 0..bands {
        qcd.extend_from_slice(&(8u16 << 11).to_be_bytes());
    }
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &[0, 0, 0, 1, 0, LEVELS, 4, 4, 0, 0]));
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

/// The heap a decode of `bytes` under `AtMost(CEILING)` holds at its peak,
/// over what was live before, and its result.
fn peak(bytes: &[u8]) -> (usize, rust_j2k::Result<rust_j2k::Image>) {
    let options = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(CEILING));
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let result = decode_with(bytes, &options);
    (PEAK.load(Ordering::SeqCst) - before, result)
}

#[test]
fn a_decode_under_at_most_peaks_within_the_documented_multiple() {
    let output = CEILING as usize * size_of::<i32>();
    let (at, image) = peak(&single_tile(SIDE));
    let image = image.expect("an image of exactly the ceiling decodes");
    let samples = &image.components[0].samples;
    assert_eq!(samples.len() as u64, CEILING);
    assert!(
        samples.iter().all(|&v| v == 128),
        "a flat image at mid-gray"
    );
    drop(image);
    let ratio = at as f64 / output as f64;
    assert!(
        ratio < PEAK_RATIO,
        "peak {at} bytes is {ratio:.2} times the {output}-byte output"
    );

    let (over, refused) = peak(&single_tile(SIDE + 1));
    match refused {
        Err(Error::Limit(limit)) => {
            assert_eq!(limit.kind(), LimitKind::Samples, "{limit}");
            assert_eq!(limit.retry_with(), None, "{limit}");
        }
        other => panic!("{:?}", other.map(|i| (i.width, i.height))),
    }
    assert!(over < 64 * 1024, "the refusal peaked at {over} bytes");
}
