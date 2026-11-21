//! The cost of `read_header_with` where it refuses a tile and so looks for
//! a fault in the tile-parts first, on the 235 MB file of
//! `docs/resource-bounds.md`: 65535 tiles of 1024 x 1024 in a 255 x 257
//! grid, 255 tile-parts a tile (16.7 million tile-parts), each tile over the
//! precinct cap under the main header and brought within by its first
//! tile-part's `COD`, except the last, which is refused. The tile read is
//! the last one alone, so the walk runs to the end. Other files restate
//! long and short first tile-part headers in every later tile-part.
//!
//! The test is ignored, as it builds that file. Run it in release with
//! `cargo test --release --test tile_parts_fault_cost -- --ignored
//! --nocapture`. It holds a single test, as the peak is measured with a
//! counting global allocator (`tests/support/alloc.rs`, the only `unsafe`
//! the project conventions allow).

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/synth.rs"]
mod synth;

use alloc::{CURRENT, PEAK};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rust_j2k::{DecodeOptions, Error, Region, read_header_with};

#[global_allocator]
static ALLOCATOR: alloc::PeakAlloc = alloc::PeakAlloc;

const COLUMNS: u32 = 255;
const ROWS: u32 = 257;
const PARTS: u8 = 255;
const LAST: u16 = (COLUMNS * ROWS - 1) as u16;

/// The 235 MB file, with the last tile's tile-parts left out when `missing`,
/// and `end` after the last tile-part.
fn many_tile_parts(missing: bool, end: &[u8]) -> Vec<u8> {
    let side = 1024;
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body(
        [side * COLUMNS, side * ROWS, 0, 0, side, side, 0, 0],
        &[[7, 1, 1]],
    );
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let own = synth::seg(0xFF52, &synth::cod_body(1, 0, 4, 0));
    let first = synth::tile_part(0, 0, PARTS, &own, &[0]);
    let rest = synth::tile_part(0, 1, PARTS, &[], &[]);
    bytes.reserve((COLUMNS * ROWS) as usize * (first.len() + 254 * rest.len()) + 2);
    for tile in 0..=LAST {
        if missing && tile == LAST {
            break;
        }
        let header: &[u8] = if tile == LAST { &[] } else { &own };
        bytes.extend(synth::tile_part(tile, 0, PARTS, header, &[0]));
        for part in 1..PARTS {
            bytes.extend(synth::tile_part(tile, part, PARTS, &[], &[]));
        }
    }
    bytes.extend_from_slice(end);
    bytes
}

/// The time and the heap at its peak, over what was live before, of
/// `read_header_with`, and its error.
fn refusal(bytes: &[u8], options: &DecodeOptions) -> (Duration, usize, Error) {
    let before = CURRENT.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let start = Instant::now();
    let header = read_header_with(bytes, options);
    let spent = start.elapsed();
    let peak = PEAK.load(Ordering::SeqCst) - before;
    (spent, peak, header.expect_err("the read refuses the file"))
}

#[test]
#[ignore = "builds a 235 MB file; run in release"]
fn a_refusal_on_the_235_mb_file_walks_the_tile_parts_once() {
    let last = DecodeOptions::default().with_region(Region::Tile(u32::from(LAST)));
    let cases = [
        ("last tile refused, strict on", false, &[0xFF, 0xD9], true),
        ("last tile refused, strict off", false, &[0xFF, 0xD9], false),
        (
            "last tile refused, junk for EOC, strict off",
            false,
            &[0, 0],
            false,
        ),
        ("last tile missing, strict on", true, &[0xFF, 0xD9], true),
    ];
    let mut over = Vec::new();
    for (name, missing, end, strict) in cases {
        let bytes = many_tile_parts(missing, end);
        let options = last.clone().with_strict(strict);
        let (spent, peak, error) = refusal(&bytes, &options);
        eprintln!(
            "{name}: {spent:?}, {} KiB at the peak: {error:?}",
            peak >> 10
        );
        if spent >= Duration::from_millis(300) || peak >= 1 << 20 {
            over.push(name);
        }
    }
    // A first tile-part header of 2^18 empty `COM`s that each of 254 later
    // tile-parts restates a `COD` over: the walk reads that header again
    // once a tile, so the refusal costs about what the decode's parse does.
    for tiles in [1, 4] {
        let bytes = restated_over_long_firsts(tiles);
        let options = DecodeOptions::default().with_region(Region::Tile(0));
        let (spent, peak, error) = refusal(&bytes, &options);
        let name = format!("{tiles} tiles of long first headers restated");
        eprintln!(
            "{name} ({} MB): {spent:?}, {} KiB at the peak: {error:?}",
            bytes.len() / 1_000_000,
            peak >> 10
        );
        if spent >= Duration::from_millis(50) {
            over.push("long first headers restated");
        }
    }
    // A first tile-part header of a `COD` and 143 `QCC`s, under 1 KiB, in
    // each of 2048 tiles of 256 components, and 254 later tile-parts a tile
    // that restate the main `QCD`, all alike or two headers in turn: the
    // walk reads each first header again at most twice, so the refusal
    // costs about what the decode's does.
    let options = DecodeOptions::default().with_region(Region::Tile(0));
    for turns in [1, 2] {
        let bytes = restated_over_qccs(turns);
        for strict in [true, false] {
            let options = options.clone().with_strict(strict);
            let (spent, _, error) = refusal(&bytes, &options);
            let start = Instant::now();
            let decoded = rust_j2k::decode_with(&bytes, &options);
            let decode = start.elapsed();
            let name = format!(
                "short first headers of `QCC`s, {turns} later headers, strict {strict}"
            );
            eprintln!(
                "{name} ({} MB): {spent:?}, the decode {decode:?}: {error:?}",
                bytes.len() / 1_000_000
            );
            let decoded = decoded.map(|_| ()).expect_err("the decode refuses the file");
            assert_eq!(format!("{decoded:?}"), format!("{error:?}"), "{name}");
            if spent > decode * 3 {
                over.push("short first headers of `QCC`s");
            }
        }
    }
    // Times are kept to a release build.
    #[cfg(not(debug_assertions))]
    assert!(over.is_empty(), "over its time or 1 MiB: {over:?}");
}

/// 2048 tiles of 64 x 64 in a row, of 256 components, over the precinct cap
/// under the main `COD` and under their own, each in 255 tile-parts: the
/// first's header is that `COD` and a `QCC` for each of components 0 to
/// 142, 1018 bytes, and the later ones' restate the main `QCD`, with an
/// empty `COM` after it in every `turns`-th.
fn restated_over_qccs(turns: u8) -> Vec<u8> {
    let (side, tiles) = (64, 2048);
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body([side * tiles, side, 0, 0, side, side, 0, 0], &[[7, 1, 1]; 256]);
    let cod = synth::seg(0xFF52, &cod);
    let qcd = synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8]));
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        cod.clone(),
        qcd.clone(),
    ]
    .concat();
    let mut first = cod;
    for component in 0..143 {
        let body = [&[component][..], &synth::qcd_none_body(2, &[8])].concat();
        first.extend(synth::seg(0xFF5D, &body));
    }
    let other = [&qcd[..], &[0xFF, 0x64, 0, 2]].concat();
    for tile in 0..tiles as u16 {
        bytes.extend(synth::tile_part(tile, 0, PARTS, &first, &[0]));
        for part in 1..PARTS {
            let later = if part % turns == 0 { &other } else { &qcd };
            bytes.extend(synth::tile_part(tile, part, PARTS, later, &[]));
        }
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// `tiles` tiles of 1024 x 1024 in a row, over the precinct cap under the
/// main `COD` and under their own, each in 255 tile-parts: the first's
/// header is that `COD` and 2^18 empty `COM`s, and each later one's
/// restates the `COD`.
fn restated_over_long_firsts(tiles: u16) -> Vec<u8> {
    let side = 1024;
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body(
        [side * u32::from(tiles), side, 0, 0, side, side, 0, 0],
        &[[7, 1, 1]],
    );
    let cod = synth::seg(0xFF52, &cod);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        cod.clone(),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
    ]
    .concat();
    let mut first = cod.clone();
    for _ in 0..1 << 18 {
        first.extend_from_slice(&[0xFF, 0x64, 0, 2]);
    }
    for tile in 0..tiles {
        bytes.extend(synth::tile_part(tile, 0, PARTS, &first, &[0]));
        for part in 1..PARTS {
            bytes.extend(synth::tile_part(tile, part, PARTS, &cod, &[]));
        }
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}
