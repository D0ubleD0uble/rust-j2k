//! A code-block's codeword segments cost a few bytes of heap each, so a file
//! of many of them stays within the documented peak (#585).
//!
//! Under the code-block style `termall` (0x04) each coding pass is its own
//! segment, and a packet header can code one in 3 bits, since `Lblock` is at
//! least 3. The tile caps count code-blocks, not segments, so what a segment
//! costs sets the heap per byte of input. It was about 130 bytes, about 350
//! bytes of heap a byte of input.
//!
//! The files are [`termall`]: one tile of 4x4 code-blocks, each of `passes`
//! one-pass segments of `len` bytes, the shape of `shaped_bomb(..,
//! termall=True)` in `scripts/gen-tier1-work-bound-fixtures.py`. The cheap
//! test pins the bytes a segment costs; the ignored ones decode the large
//! shapes of `docs/resource-bounds.md` and check the peak per byte.
//!
//! The peak is measured with a counting global allocator, so the tests take
//! one lock and run one at a time. The lock is taken before the baseline is
//! read: a std mutex may box itself on first use (macOS's pthread mutex
//! does), which would count against the decode.
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
use rust_j2k::{DecodeOptions, WorkLimit, decode_with};
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use synth::{Bits, seg};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

/// One measurement at a time: they share the allocator's counters.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The pass count's code (ISO/IEC 15444-1 Table B.4).
fn num_passes(bits: &mut Bits, passes: u32) {
    match passes {
        1 => bits.put(0),
        2 => bits.put_int(0b10, 2),
        3..=5 => {
            bits.put_int(0b11, 2);
            bits.put_int(passes - 3, 2);
        }
        6..=36 => {
            bits.put_int(0b1111, 4);
            bits.put_int(passes - 6, 5);
        }
        _ => {
            bits.put_int(0b1_1111_1111, 9);
            bits.put_int(passes - 37, 7);
        }
    }
}

/// One `side` x `side` 8-bit tile, one layer, no wavelet levels, in 4x4
/// code-blocks of code-block style `termall`. Every block is in the layer
/// with `passes` passes, each its own codeword segment of `len` bytes of
/// zeros (`len` below 8, so the length field keeps `Lblock` at 3).
fn termall(side: u32, passes: u32, len: u32) -> Vec<u8> {
    assert!((1..=164).contains(&passes) && len < 8);
    let cols = side.div_ceil(4);
    // The tag trees' levels, a leaf level first, up to the 1x1 root.
    let mut levels = 1;
    while cols.div_ceil(1 << (levels - 1)) > 1 {
        levels += 1;
    }
    let mut bits = Bits::new();
    bits.put(1);
    // Every value of both tag trees is 0, so each node on a leaf's path not
    // yet coded codes a 1 (B.10.2).
    let mut known = [HashSet::new(), HashSet::new()];
    for y in 0..cols {
        for x in 0..cols {
            for tree in &mut known {
                for level in (0..levels).rev() {
                    if tree.insert((level, x >> level, y >> level)) {
                        bits.put(1);
                    }
                }
            }
            num_passes(&mut bits, passes);
            bits.put(0);
            for _ in 0..passes {
                bits.put_int(len, 3);
            }
        }
    }
    let mut data = bits.finish();
    data.resize(data.len() + (cols * cols * passes * len) as usize, 0);
    let siz = synth::siz_body([side, side, 0, 0, side, side, 0, 0], &[[7, 1, 1]]);
    let mut out = vec![0xFF, 0x4F];
    out.extend(seg(0xFF51, &siz));
    out.extend(seg(0xFF52, &synth::cod_body(1, 0, 0, 0x04)));
    out.extend(seg(0xFF5C, &synth::qcd_none_body(2, &[29])));
    out.extend(synth::tile_part(0, 0, 1, &[], &data));
    out.extend([0xFF, 0xD9]);
    out
}

/// The heap `bytes` peaks at under `options` past what was live before,
/// with the image it decodes to.
fn peak(bytes: &[u8], options: &DecodeOptions) -> usize {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let image = decode_with(bytes, options).expect("the file decodes");
    let peak = PEAK.load(Ordering::SeqCst) - before;
    let side = image.width as usize;
    assert_eq!(image.components[0].samples.len(), side * side);
    peak
}

/// What one more segment costs at peak: a file of `passes` segments a block
/// against one of a segment a block, over the segments it adds.
fn per_segment(side: u32, passes: u32, len: u32) -> usize {
    let options = DecodeOptions::default();
    let one = peak(&termall(side, 1, len), &options);
    let many = peak(&termall(side, passes, len), &options);
    let blocks = side.div_ceil(4).pow(2) as usize;
    let segments = blocks * (passes as usize - 1);
    eprintln!("segment_records_alloc: {one} and {many} bytes at peak, {segments} more segments");
    many.saturating_sub(one) / segments
}

/// A segment of no bytes costs its pass count and Tier-1's head of it,
/// about 15 bytes, where it cost about 130 before (#585). One of a byte
/// adds its chunk and the packet's note of it, about 110 bytes in all.
/// `passes` is 65 so that the lists of a block's segments and chunks have
/// just grown, near twice what they hold.
#[test]
fn a_segment_costs_a_few_bytes_of_heap() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let empty = per_segment(256, 65, 0);
    let one_byte = per_segment(256, 65, 1);
    eprintln!("segment_records_alloc: {empty} bytes a segment of none, {one_byte} of one byte");
    assert!(empty <= 24, "a segment of no bytes costs {empty} bytes");
    assert!(
        one_byte <= 128,
        "a segment of one byte costs {one_byte} bytes"
    );
}

/// The shapes of `docs/resource-bounds.md` ("Segment records"): 4096 x 4096
/// with segments of no bytes at the defaults (37 MB, which peaked at 11.2
/// GiB) and under `AtMost(2^24)`, and 2048 x 2048 with segments of one byte
/// (32 MB). They peak at about 40 and 64 bytes of heap a byte of input, the
/// most a segment costs, under the 102 bytes a byte the bound states.
#[cfg(target_pointer_width = "64")]
#[test]
#[ignore = "peaks near 2 GiB"]
fn large_termall_files_peak_within_the_documented_bound() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let at_most = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(1 << 24));
    for (side, len, options) in [
        (4096, 0, DecodeOptions::default()),
        (4096, 0, at_most),
        (2048, 1, DecodeOptions::default()),
    ] {
        let bytes = termall(side, 88, len);
        let peak = peak(&bytes, &options);
        let per_byte = peak as f64 / bytes.len() as f64;
        eprintln!(
            "segment_records_alloc: {side}x{side}, {len}-byte segments, {} bytes: \
             {peak} bytes at peak, {per_byte:.1} a byte",
            bytes.len()
        );
        assert!(
            per_byte < 80.0,
            "{per_byte:.1} bytes of heap a byte of input"
        );
    }
}
