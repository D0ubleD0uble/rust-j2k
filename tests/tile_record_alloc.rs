//! The parse of the tile-parts, once the codestream has one, keeps a record
//! for each tile the SIZ grid declares, read or not and present or not, and
//! no sample limit counts it. This holds that record to the figure
//! `docs/resource-bounds.md` gives: under 1 KiB a tile (872 bytes measured),
//! so at most about 57 MB for a file of any size.
//!
//! Both files declare the most tiles a codestream can: 255x257 tiles of one
//! sample, 65535 in all. One has every tile, each in one 15-byte tile-part.
//! The other, 82 bytes, has only the tile-part of tile 0 and is decoded with
//! `strict` off. Under `WorkLimit::AtMost(4096)` tile 0 of each decodes, and
//! the decode's peak is the record.
//!
//! This counts the bytes allocated. The resident growth measured the same
//! (892 bytes a tile), as these allocations are small and all written. The
//! records are smaller on a 32-bit target, so the test runs on 64-bit ones.
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

/// The most the record may take for each tile, in bytes.
const PER_TILE: usize = 1024;

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
/// peak, over what was live before.
fn peak(bytes: &[u8], strict: bool) -> usize {
    let options = DecodeOptions::default()
        .with_sample_limit(WorkLimit::AtMost(4096))
        .with_region(Region::Tile(0))
        .with_strict(strict);
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let image = decode_with(bytes, &options).expect("tile 0 decodes");
    let at = PEAK.load(Ordering::SeqCst) - before;
    assert_eq!((image.width, image.height), (1, 1));
    at
}

#[test]
fn the_tile_record_stays_under_a_kib_a_declared_tile() {
    let tiles = (COLUMNS * ROWS) as usize;
    let every = one_sample_tiles(COLUMNS * ROWS);
    let one = one_sample_tiles(1);
    assert_eq!(one.len(), 82);
    for (what, bytes, strict) in [("every tile", &every, true), ("one tile", &one, false)] {
        let at = peak(bytes, strict);
        assert!(
            at < PER_TILE * tiles,
            "{what}: peak {at} bytes is {:.0} bytes a tile for {tiles} tiles",
            at as f64 / tiles as f64
        );
        // Most of it is the record: a decode of one sample needs a few KiB.
        assert!(
            at > PER_TILE / 2 * tiles,
            "{what}: peak {at} bytes: the record has shrunk, so update the docs"
        );
    }
}
