//! The parse of the tile-parts keeps a record for each tile that has a
//! tile-part, read or not, and 2 bytes for each tile the SIZ grid declares;
//! no sample limit counts them. This holds them to the figures
//! `docs/resource-bounds.md` gives: under 1 KiB for each tile present (832
//! bytes measured), and about 128 KiB for 65535 declared tiles with one
//! present.
//!
//! Both files declare the most tiles a codestream can: 255x257 tiles of one
//! sample, 65535 in all. One has every tile, each in one 15-byte tile-part,
//! and is decoded strict. The other, 82 bytes, has only the tile-part of
//! tile 0: with `strict` off it decodes, and strict it is refused for the
//! missing tiles. Under `WorkLimit::AtMost(4096)` tile 0 decodes, and the
//! decode's peak is the records.
//!
//! This counts the bytes allocated. The resident growth measured about the
//! same (892 bytes a tile, before the records of absent tiles shrank), as
//! these allocations are small and all written. The records are smaller on a
//! 32-bit target, so the test runs on 64-bit ones.
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

/// The most the record may take for each tile present, in bytes.
const PER_TILE: usize = 1024;

/// The most a decode of the file with one tile present may peak at.
const SPARSE_PEAK: usize = 1 << 20;

/// One 8-bit component in `COLUMNS` x `ROWS` tiles of one sample, no
/// wavelet levels, one layer; the first `present` tiles each in one
/// tile-part with a single empty packet.
fn one_sample_tiles(present: u32) -> Vec<u8> {
    let siz = synth::siz_body([COLUMNS, ROWS, 0, 0, 1, 1, 0, 0], &[[7, 1, 1]]);
    let mut cs = vec![0xFF, 0x4F];
    cs.extend(seg(0xFF51, &siz));
    cs.extend(seg(0xFF52, &synth::cod_body(1, 0, 4, 0)));
    cs.extend(seg(0xFF5C, &synth::qcd_none_body(1, &[8])));
    for tile in 0..present {
        let tile = u16::try_from(tile).expect("65535 tiles at most");
        cs.extend(synth::tile_part(tile, 0, 1, &[], &[0]));
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
    let tiles = (COLUMNS * ROWS) as usize;

    // Every tile present: each costs its record, under 1 KiB.
    let every = one_sample_tiles(COLUMNS * ROWS);
    let (at, decoded) = peak(&every, true);
    decoded.expect("tile 0 decodes");
    assert!(
        at < PER_TILE * tiles,
        "every tile: peak {at} bytes is {:.0} bytes a tile for {tiles} tiles",
        at as f64 / tiles as f64
    );
    // Most of it is the records: a decode of one sample needs a few KiB.
    assert!(
        at > PER_TILE / 2 * tiles,
        "every tile: peak {at} bytes: the record has shrunk, so update the docs"
    );

    // One tile present of the 65535 declared: a tile with no tile-part costs
    // a few bytes, so the decode stays under 1 MiB. Strict, the file is
    // refused for the missing tiles, and the refusal costs no more.
    let one = one_sample_tiles(1);
    assert_eq!(one.len(), 82);
    let (at, decoded) = peak(&one, false);
    decoded.expect("tile 0 decodes");
    assert!(at < SPARSE_PEAK, "one tile, strict off: peak {at} bytes");
    let (at, decoded) = peak(&one, true);
    let error = decoded.expect_err("strict refuses the missing tiles");
    assert!(
        error.to_string().contains("no tile-part for tile 1"),
        "strict refusal: {error}"
    );
    assert!(at < SPARSE_PEAK, "one tile, strict on: peak {at} bytes");
}
