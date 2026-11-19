//! The parse of the tile-parts keeps one record for each tile that has a
//! tile-part, read or not, and 2 bytes for each tile the SIZ grid declares;
//! no sample limit counts them. This holds them to the figures
//! `docs/resource-bounds.md` gives: about 830 bytes for each tile present
//! (832 measured with the rest of the decode), and about 128 KiB for 65535
//! declared tiles with one present, however long its tile-part.
//!
//! Every file declares the most tiles a codestream can: 255x257 tiles of one
//! sample, 65535 in all. One has every tile, each in one 15-byte tile-part,
//! and is decoded strict; its ceiling is tight enough that a records vector
//! regrown by doubling would pass it. The others have only the tile-part of
//! tile 0, of 15 bytes (82 bytes in all) or with 100 KB of padding after its
//! packet: with `strict` off they decode, and strict they are refused for
//! the missing tiles. Under `WorkLimit::AtMost(4096)` tile 0 decodes, and the
//! decode's peak is the records.
//!
//! This counts the bytes allocated, a `realloc` as the new block allocated
//! before the old one is freed. The resident growth measured about the same
//! as the allocated bytes, as these allocations are small and all written.
//! The records are smaller on a 32-bit target, so the test runs on 64-bit
//! ones.
//!
//! The peak is measured with a counting global allocator, so this file holds a
//! single test: a second one running in parallel would count into the same
//! peak.
//!
//! A global allocator can only be written with `unsafe`. The shim in
//! `tests/support/alloc.rs` forwards every call to `System` unchanged, which
//! the project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.
#![cfg(target_pointer_width = "64")]

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;
use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;
use synth::seg;

use rust_j2k::{DecodeOptions, Region, WorkLimit, decode_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

const COLUMNS: u32 = 255;
const ROWS: u32 = 257;

/// The most a decode of the file with every tile present may peak at, in
/// bytes: 54,525,116 measured. One reserve of a record for each tile and
/// nothing more fits; a vector of records regrown to the same length
/// peaks at half as much again.
const DENSE_PEAK: usize = 56_000_000;

/// The least it may peak at: most of it is the records, as a decode of one
/// sample needs a few KiB.
const DENSE_FLOOR: usize = 50_000_000;

/// The most a decode of a file with one tile present may peak at.
const SPARSE_PEAK: usize = 1 << 20;

/// One 8-bit component in `COLUMNS` x `ROWS` tiles of one sample, no
/// wavelet levels, one layer; the first `present` tiles each in one
/// tile-part with a single empty packet and then `padding` bytes of 0.
fn one_sample_tiles(present: u32, padding: usize) -> Vec<u8> {
    let siz = synth::siz_body([COLUMNS, ROWS, 0, 0, 1, 1, 0, 0], &[[7, 1, 1]]);
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &synth::cod_body(1, 0, 4, 0)));
    cs.extend(seg(0xFF5C, &synth::qcd_none_body(1, &[8])));
    // The empty packet is the one 0 byte.
    let data = vec![0; 1 + padding];
    for tile in 0..present {
        let tile = u16::try_from(tile).expect("65535 tiles at most");
        cs.extend(synth::tile_part(tile, 0, 1, &[], &data));
    }
    cs.extend_from_slice(&[0xFF, 0xD9]);
    cs
}

/// The heap a decode of tile 0 of `bytes` under `AtMost(4096)` holds at its
/// peak, over what was live before, and what the decode gave.
fn peak(bytes: &[u8], strict: bool) -> (usize, Result<(), rust_j2k::Error>) {
    let options = DecodeOptions::default()
        .with_sample_limit(WorkLimit::AtMost(4096))
        .with_region(Region::Tile(0))
        .with_strict(strict);
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let decoded = decode_with(bytes, &options);
    let at = PEAK.load(Ordering::SeqCst) - before;
    let decoded = decoded.map(|image| assert_eq!((image.width, image.height), (1, 1)));
    eprintln!("strict {strict}, {} bytes: peak {at}", bytes.len());
    (at, decoded)
}

#[test]
fn the_tile_records_cost_the_tiles_present() {
    // Every tile present: one record each, reserved once.
    let every = one_sample_tiles(COLUMNS * ROWS, 0);
    let (at, decoded) = peak(&every, true);
    decoded.expect("tile 0 decodes");
    assert!(at < DENSE_PEAK, "every tile: peak {at} bytes");
    assert!(
        at > DENSE_FLOOR,
        "every tile: peak {at} bytes: the record has shrunk, so update the docs"
    );

    // One tile present of the 65535 declared: a tile with no tile-part costs
    // 2 bytes, so the decode stays under 1 MiB, and a longer tile-part
    // reserves no more. Strict, the file is refused for the missing tiles,
    // and the refusal costs no more.
    let short = one_sample_tiles(1, 0);
    assert_eq!(short.len(), 82);
    let long = one_sample_tiles(1, 100_000);
    for (what, bytes) in [("short", &short), ("long", &long)] {
        let (at, decoded) = peak(bytes, false);
        decoded.unwrap_or_else(|e| panic!("{what}: tile 0 decodes: {e}"));
        assert!(at < SPARSE_PEAK, "{what}, strict off: peak {at} bytes");
        let (at, decoded) = peak(bytes, true);
        let error = decoded.expect_err("strict refuses the missing tiles");
        assert!(
            error.to_string().contains("no tile-part for tile 1"),
            "{what}, strict refusal: {error}"
        );
        assert!(at < SPARSE_PEAK, "{what}, strict on: peak {at} bytes");
    }
}
