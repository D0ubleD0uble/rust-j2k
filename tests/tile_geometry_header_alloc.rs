//! The header read's count of each tile's code-blocks and precincts reads
//! nothing sized by the input: `read_header_with` on a file of a tile in
//! four tile-parts of 4 MiB each holds a few kilobytes of heap at its peak,
//! both where the main header shows the tile within the caps (nothing past
//! the first `SOT` is read) and where a tile-part `COD` brings a tile the
//! main header puts over them back within (the tile-part headers are read,
//! but no tile data is joined or copied). Where every tile of many
//! components has a tile-part `COD` that brings it within, the peak is one
//! tile's coding parameters, however many tiles there are.
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

use rust_j2k::{DecodeOptions, read_header_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

/// The most heap the header read may hold at its peak.
const PEAK_BYTES: usize = 64 << 10;

/// The bytes of each tile-part's data.
const PART: usize = 4 << 20;

/// A one-tile `width` x `height` codestream of `levels` levels and code-blocks
/// of `4 << block`, its tile in four tile-parts of `PART` bytes, the first
/// carrying `extra` in its header.
fn four_parts(width: u32, height: u32, levels: u8, block: u8, extra: &[u8]) -> Vec<u8> {
    let mut bytes = synth::empty_tiles(
        [width, height, 0, 0, width, height, 0, 0],
        levels,
        block,
        &[],
        &[0],
        0,
    );
    // The main header, without its tile-part of no data and `EOC`.
    bytes.truncate(bytes.len() - 16);
    for part in 0..4 {
        let header = if part == 0 { extra } else { &[] };
        bytes.extend(synth::tile_part(0, part, 4, header, &vec![0; PART]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// `tiles` tiles of 8x16 and 4096 components, no levels, 1x1 precincts in
/// the main header (524,288 precincts a tile, over the 2^18 cap), each with
/// a tile-part `COD` of the maximal precincts (4096 a tile), and no data.
fn many_components(tiles: u32) -> Vec<u8> {
    let components = 4096;
    let mut cod = synth::cod_body(1, 0, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body(
        [8 * tiles, 16, 0, 0, 8, 16, 0, 0],
        &vec![[7, 1, 1]; components],
    );
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
    for tile in 0..tiles {
        bytes.extend(synth::tile_part(tile as u16, 0, 1, &own, &[]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// The heap `read_header_with` holds at its peak over what was live before.
fn peak_of_header_read(bytes: &[u8]) -> usize {
    let options = DecodeOptions::default();
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let header = read_header_with(bytes, &options).expect("the header is admitted");
    let peak = PEAK.load(Ordering::SeqCst) - before;
    drop(header);
    peak
}

#[test]
fn the_header_read_holds_no_tile_data() {
    // 8192 x 8192 in 64x64 code-blocks: within the caps under the main header.
    let within = four_parts(8192, 8192, 5, 4, &[]);
    let peak = peak_of_header_read(&within);
    eprintln!("within the caps: {peak} bytes at the peak");
    assert!(peak < PEAK_BYTES, "{peak} bytes");

    // 4096 x 4160 in 4x4 code-blocks is over the cap under the main header;
    // the tile-part `COD` of 64x64 brings it within.
    let cod = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
    let rescued = four_parts(4096, 4160, 0, 0, &cod);
    let peak = peak_of_header_read(&rescued);
    eprintln!("over the caps under the main header: {peak} bytes at the peak");
    assert!(peak < PEAK_BYTES, "{peak} bytes");

    // Each tile's own header is a copy of the main header's 4096 components;
    // only one is held at a time, so 256 tiles peak as 4 do.
    let few = peak_of_header_read(&many_components(4));
    let many = peak_of_header_read(&many_components(256));
    eprintln!("4096 components: {few} bytes at the peak for 4 tiles, {many} for 256");
    assert!(many <= few + PEAK_BYTES, "{few} and {many} bytes");
}
