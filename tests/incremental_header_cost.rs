//! The cost of calling `IncrementalDecoder::header` after every feed.
//!
//! The decoder keeps the walk of the tile-parts across calls, so a call
//! after each feed costs about one walk of the file in all, not one walk a
//! call. The 235 MB file of `docs/resource-bounds.md` is built here: 65535
//! tiles of 1024 x 1024 in a 255 x 257 grid, each over the precinct cap
//! under the main header and brought within by its first tile-part's `COD`,
//! with 255 tile-parts a tile, each tile's together. It is read through a
//! window of the last 6 rows, 1530 tiles, so the walk runs to the end. The
//! tests are ignored, as they build that file; run them in release with
//! `cargo test --release --test incremental_header_cost -- --ignored
//! --nocapture`.

#[path = "support/synth.rs"]
mod synth;

use std::time::{Duration, Instant};

use rust_j2k::{DecodeOptions, IncrementalDecoder, Region, Window, read_header_with};

/// The 235 MB file.
fn many_tile_parts() -> Vec<u8> {
    let (side, columns, rows, parts) = (1024, 255, 257, 255);
    let mut cod = synth::cod_body(1, 0, 4, 0);
    cod[0] = 1;
    cod.push(0x00);
    let siz = synth::siz_body(
        [side * columns, side * rows, 0, 0, side, side, 0, 0],
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
    let first = synth::tile_part(0, 0, parts, &own, &[0]);
    let rest = synth::tile_part(0, 1, parts, &[], &[]);
    bytes.reserve((columns * rows) as usize * (first.len() + 254 * rest.len()) + 2);
    for tile in 0..(columns * rows) as u16 {
        bytes.extend(synth::tile_part(tile, 0, parts, &own, &[0]));
        for part in 1..parts {
            bytes.extend(synth::tile_part(tile, part, parts, &[], &[]));
        }
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// `components` gray components over a row of `width` tiles of 1x1 and a
/// row of `width` tiles of 1 x `tall`, with `levels` levels and precincts
/// of 1x1 at resolution 0 and 2x2 above. Every tile has a tile-part with no
/// `COD` or `COC` and no data. (`tests/tile_geometry_cap.rs` has the same.)
fn small_row_over_tall_row(components: usize, width: u32, tall: u32, levels: u8) -> Vec<u8> {
    let siz = synth::siz_body(
        [width, 2 * tall, 0, tall - 1, 1, tall, 0, 0],
        &vec![[7, 1, 1]; components],
    );
    let mut cod = synth::cod_body(1, levels, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    cod.extend(vec![0x11; usize::from(levels)]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(
            0xFF5C,
            &synth::qcd_none_body(2, &vec![8; 3 * usize::from(levels) + 1]),
        ),
    ]
    .concat();
    for tile in 0..2 * width {
        bytes.extend(synth::tile_part(tile as u16, 0, 1, &[], &[]));
    }
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// Feed `bytes` to a decoder under `options` in pieces of `piece` bytes,
/// calling `header` after each, and return the calls and the time they
/// took. The last call must give what `read_header_with` gives on the file.
fn header_after_each_feed(
    bytes: &[u8],
    options: &DecodeOptions,
    piece: usize,
) -> (usize, Duration) {
    let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
    let mut spent = Duration::ZERO;
    let mut calls = 0;
    let mut last = None;
    for chunk in bytes.chunks(piece) {
        decoder.feed(chunk);
        let start = Instant::now();
        let header = decoder.header();
        spent += start.elapsed();
        calls += 1;
        last = Some(header);
    }
    let whole = read_header_with(bytes, &options.clone().with_strict(false));
    assert_eq!(
        format!("{:?}", last.expect("one call")),
        format!("{:?}", whole.map(Some)),
    );
    (calls, spent)
}

#[test]
#[ignore = "builds a 235 MB file; run in release"]
fn header_after_each_feed_of_a_235_mb_file_costs_about_one_walk() {
    let bytes = many_tile_parts();
    let window = Window::new(0, 1024 * 251, 1024 * 255, 1024 * 257);
    let options = DecodeOptions::default().with_region(Region::Window(window));
    for piece in [1 << 20, 1 << 18] {
        let (calls, spent) = header_after_each_feed(&bytes, &options, piece);
        eprintln!(
            "{} MB, {piece}-byte feeds: {calls} calls in {spent:?}",
            bytes.len() / 1_000_000
        );
        #[cfg(not(debug_assertions))]
        if piece == 1 << 18 {
            assert!(spent < Duration::from_millis(500), "{spent:?}");
        }
    }
}

#[test]
#[ignore = "a measurement; run in release"]
fn header_after_each_feed_of_many_components_under_a_reduction() {
    let bytes = small_row_over_tall_row(1024, 400, 4096, 1);
    let options = DecodeOptions::default().with_resolution_reduction(1);
    for piece in [1 << 18, 1024, 256] {
        let (calls, spent) = header_after_each_feed(&bytes, &options, piece);
        eprintln!(
            "{} bytes, {piece}-byte feeds: {calls} calls in {spent:?}",
            bytes.len()
        );
    }
}
