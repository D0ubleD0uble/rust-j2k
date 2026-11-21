//! The header read's count of each tile's code-blocks and precincts reads
//! nothing sized by the input: `read_header_with` on a file of a tile in
//! four tile-parts of 4 MiB each holds a few kilobytes of heap at its peak,
//! both where the main header shows the tile within the caps (nothing past
//! the first `SOT` is read) and where a tile-part `COD` brings a tile the
//! main header puts over them back within (the tile-part headers are read,
//! but no tile data is joined or copied). Where every tile of many
//! components has a tile-part `COD` that brings it within, the peak is one
//! tile's coding parameters, however many tiles there are. Where a tile is
//! refused, the read walks every tile-part for the fault the decode gives
//! first, and holds about 10 bytes a tile for that walk.
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

use rust_j2k::{DecodeOptions, Error, Region, Window, read_header_with};

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
    peak_of_header_read_with(bytes, &DecodeOptions::default())
}

/// The same under `options`.
fn peak_of_header_read_with(bytes: &[u8], options: &DecodeOptions) -> usize {
    let (peak, header) = peak_and_result(bytes, options);
    header.expect("the header is admitted");
    peak
}

/// The same, for a read that refuses the file, with its error.
fn peak_of_refusal(bytes: &[u8], options: &DecodeOptions) -> (usize, rust_j2k::Error) {
    let (peak, header) = peak_and_result(bytes, options);
    (peak, header.expect_err("the read refuses the file"))
}

fn peak_and_result(
    bytes: &[u8],
    options: &DecodeOptions,
) -> (usize, rust_j2k::Result<rust_j2k::Header>) {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let header = read_header_with(bytes, options);
    let peak = PEAK.load(Ordering::SeqCst) - before;
    (peak, header)
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

    // The most tiles a codestream declares, each over the cap under the main
    // header and brought within by its tile-part `COD`, read in a window of
    // the first column, from tile 0 to tile 65280. The header read holds a
    // byte a tile for which tiles the selection reads, and 8 bytes a tile
    // from the first tile over a cap to the last read for where each one's
    // first tile-part starts.
    let tiles = 65535;
    let column = Window::new(0, 0, 520, 512 * 257);
    let options = DecodeOptions::default().with_region(Region::Window(column));
    let peak = peak_of_header_read_with(&most_tiles(), &options);
    eprintln!("{tiles} tiles, a column read: {peak} bytes at the peak");
    assert!(peak < PEAK_BYTES + 9 * tiles, "{peak} bytes");
    // A tile read alone holds the 8 bytes of its own offset.
    let last = DecodeOptions::default().with_region(Region::Tile(tiles as u32 - 1));
    let peak = peak_of_header_read_with(&most_tiles(), &last);
    eprintln!("{tiles} tiles, the last read: {peak} bytes at the peak");
    assert!(peak < PEAK_BYTES + tiles, "{peak} bytes");

    // The last tile refused, as it has no tile-part `COD`, and read alone:
    // the read then walks every tile-part for the fault the decode would
    // give first, with 6 bytes a tile, and 4 more a tile once a tile's first
    // tile-part has its own `COD`. With `00 00` where the `EOC` should be,
    // that fault is the read's error.
    for (end, strict) in [
        (EOC, true),
        (EOC, false),
        (&[0, 0][..], true),
        (&[0, 0], false),
    ] {
        let options = last.clone().with_strict(strict);
        let (peak, error) = peak_of_refusal(&most_tiles_last_refused(end), &options);
        eprintln!("{tiles} tiles, the last refused, strict {strict}: {peak} bytes at the peak");
        assert!(peak < PEAK_BYTES + 11 * tiles, "{peak} bytes");
        let fault = end != EOC;
        assert_eq!(
            matches!(&error, Error::Codestream(d)
                if d.message() == "expected `SOT` or `EOC` after a tile-part, found 0x0000"),
            fault,
            "{error:?}"
        );
        assert_eq!(matches!(error, Error::Limit(_)), !fault, "{error:?}");
    }
}

const EOC: &[u8] = &[0xFF, 0xD9];

/// 65535 tiles of 520 x 512 in a 255 x 257 grid, in 1x1 precincts under the
/// main header (266,240 a tile, over the 2^18 cap), each with a tile-part
/// `COD` of the maximal precincts and one empty packet.
fn most_tiles() -> Vec<u8> {
    most_tiles_with(u16::MAX, EOC)
}

/// [`most_tiles`], but the last tile has no tile-part `COD`, so it is
/// refused, and `end` closes the codestream.
fn most_tiles_last_refused(end: &[u8]) -> Vec<u8> {
    most_tiles_with(u16::MAX - 1, end)
}

/// [`most_tiles`], with a tile-part `COD` in tiles 0 to `own - 1` only, and
/// `end` after the last tile-part.
fn most_tiles_with(own_up_to: u16, end: &[u8]) -> Vec<u8> {
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body([520 * 255, 512 * 257, 0, 0, 520, 512, 0, 0], &[[7, 1, 1]]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
    for tile in 0..u16::MAX {
        let header: &[u8] = if tile < own_up_to { &own } else { &[] };
        bytes.extend(synth::tile_part(tile, 0, 1, header, &[0]));
    }
    bytes.extend_from_slice(end);
    bytes
}
