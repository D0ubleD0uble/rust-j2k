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
    filled_then_termall(side, 1, 1, passes, len)
}

/// `cols` x `rows` tiles of `side` x `side`, the layout of [`termall`]: each
/// tile but the last an empty packet, so its samples are all 0, and the
/// last a [`termall_data`] tile. The decode fills the earlier tiles' part
/// of the image before it reads the last tile's segments.
fn filled_then_termall(side: u32, cols: u32, rows: u32, passes: u32, len: u32) -> Vec<u8> {
    let siz = synth::siz_body(
        [side * cols, side * rows, 0, 0, side, side, 0, 0],
        &[[7, 1, 1]],
    );
    let mut out = vec![0xFF, 0x4F];
    out.extend(seg(0xFF51, &siz));
    out.extend(seg(0xFF52, &synth::cod_body(1, 0, 0, 0x04)));
    out.extend(seg(0xFF5C, &synth::qcd_none_body(2, &[29])));
    let last = u16::try_from(cols * rows - 1).expect("too many tiles");
    for tile in 0..last {
        out.extend(synth::tile_part(tile, 0, 1, &[], &[0x00]));
    }
    let data = termall_data(side, passes, len);
    out.extend(synth::tile_part(last, 0, 1, &[], &data));
    out.extend([0xFF, 0xD9]);
    out
}

/// The tile data of [`termall`]: one packet holding every block.
fn termall_data(side: u32, passes: u32, len: u32) -> Vec<u8> {
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
    data
}

/// The heap `bytes` peaks at under `options` past what was live before,
/// with the image it decodes to.
fn peak(bytes: &[u8], options: &DecodeOptions) -> usize {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let image = decode_with(bytes, options).expect("the file decodes");
    let peak = PEAK.load(Ordering::SeqCst) - before;
    let area = image.width as usize * image.height as usize;
    assert_eq!(image.components[0].samples.len(), area);
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
/// adds its chunk, Tier-1's slice of it and the packet's note of it, about
/// 85 bytes in all. `passes` is 65 so that the lists of a block's segments
/// and chunks have just grown, near twice what they hold.
#[test]
fn a_segment_costs_a_few_bytes_of_heap() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let empty = per_segment(256, 65, 0);
    let one_byte = per_segment(256, 65, 1);
    eprintln!("segment_records_alloc: {empty} bytes a segment of none, {one_byte} of one byte");
    assert!(empty <= 24, "a segment of no bytes costs {empty} bytes");
    assert!(
        one_byte <= 96,
        "a segment of one byte costs {one_byte} bytes"
    );
}

/// The shapes of `docs/resource-bounds.md` ("Segment records"), one tile
/// each: 4096 x 4096 of 88 segments of no bytes at the defaults (37 MB,
/// which peaked at 11.2 GiB) and under `AtMost(2^24)`, and 2048 x 2048 of
/// 88, 65 and 33 segments of a byte. A block of 65 or 33 segments has just
/// grown its lists to about twice what they hold, the most room they keep.
/// Each stays under 72 bytes of heap a byte of input, under the 102 the
/// bound states. Last, 4096 x 4096 of 65 segments of a byte (96 MB) under
/// `AtMost(2^24)` stays under what `SECURITY.md` says to plan on for it.
#[cfg(target_pointer_width = "64")]
#[test]
#[ignore = "peaks near 6 GiB"]
fn large_termall_files_peak_within_the_documented_bound() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    let at_most = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(1 << 24));
    for (side, passes, len, options) in [
        (4096, 88, 0, DecodeOptions::default()),
        (4096, 88, 0, at_most.clone()),
        (2048, 88, 1, DecodeOptions::default()),
        (2048, 65, 1, DecodeOptions::default()),
        (2048, 33, 1, DecodeOptions::default()),
    ] {
        let bytes = termall(side, passes, len);
        let peak = peak(&bytes, &options);
        let per_byte = peak as f64 / bytes.len() as f64;
        eprintln!(
            "segment_records_alloc: {side}x{side}, {passes} {len}-byte segments, {} bytes: \
             {peak} bytes at peak, {per_byte:.1} a byte",
            bytes.len()
        );
        assert!(
            per_byte < 72.0,
            "{per_byte:.1} bytes of heap a byte of input"
        );
    }
    // 3.2 x 4n for the samples, about 0.8 GiB for one tile's code-block and
    // precinct records, about 65 bytes a byte of input for the segment
    // records, and the input.
    let bytes = termall(4096, 65, 1);
    let n = 1u64 << 24;
    let plan = 3.2 * (4 * n) as f64 + 0.8 * (1u64 << 30) as f64 + 66.0 * bytes.len() as f64;
    let peak = peak(&bytes, &at_most) as f64;
    eprintln!(
        "segment_records_alloc: 4096x4096, 65 1-byte segments, {} bytes, AtMost(2^24): \
         {peak} bytes at peak, {plan} planned",
        bytes.len()
    );
    assert!(peak < plan, "{peak} bytes at peak, over the {plan} planned");
}

/// A tile's segment records sit on top of the image the earlier tiles have
/// filled. Tiles of 4096 x 4096 with an empty packet each, then a last one
/// of `passes` segments of a byte in each 4x4 block. 8 x 4 tiles of 33
/// segments are 49 MB and 2^29 samples, the floor, held to about 6.4 GiB.
/// 8 x 5 tiles of 65 segments are 96 MB and 40 x 2^24 samples, 2.5 GiB as
/// `i32`, held to about 102 bytes a byte of input.
#[cfg(target_pointer_width = "64")]
#[test]
#[ignore = "peaks near 9 GiB"]
fn a_termall_tile_after_filled_tiles_peaks_within_the_bound() {
    let _one = ONE_AT_A_TIME.lock().unwrap();
    for (rows, passes) in [(4, 33), (5, 65)] {
        let bytes = filled_then_termall(4096, 8, rows, passes, 1);
        let peak = peak(&bytes, &DecodeOptions::default()) as f64;
        let len = bytes.len() as f64;
        let bound = if bytes.len() <= 64 << 20 {
            6.4 * (1u64 << 30) as f64
        } else {
            102.0 * len
        };
        eprintln!(
            "segment_records_alloc: 8x{rows} tiles, {passes} segments, {len} bytes: \
             {peak} bytes at peak, {:.1} a byte, against {bound}",
            peak / len
        );
        assert!(peak < bound, "{peak} bytes of heap, over {bound}");
    }
}
