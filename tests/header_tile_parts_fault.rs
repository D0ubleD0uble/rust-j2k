//! `read_header_with` against `decode_with` where a tile is refused and the
//! tile-parts hold a fault.
//!
//! The decode parses every tile-part before it counts any tile, so a fault
//! in the tile-parts comes before a tile over a cap. Where the header read
//! refuses a tile, it looks for that fault with a walk of the tile-parts that
//! keeps no tile record, and gives it, with `strict` on and off.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, LimitKind, Region, decode_with, read_header_with};

/// `result`'s error as its `Debug` text: [`Error`] has no `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> Result<(), String> {
    result.map(drop).map_err(|error| format!("{error:?}"))
}

/// A gray codestream `1000 * tiles` wide and 1000 tall, with `XOsiz` 992 and
/// tiles of 1000 x 1000, so tile 0 is 8 wide and the others 1000. The main
/// `COD` gives one level, 1x1 precincts at resolution 0 and 2x2 at 1, which
/// puts the tiles 1000 wide over the precinct cap and leaves tile 0 within
/// it. Tile 0 has a tile-part `COD` of no levels and one empty packet. Each
/// entry of `parts` is a tile-part of the tile it names, with the header
/// markers it gives and no data, and `end` closes the codestream.
fn row(tiles: u32, tile_0: &[u8], parts: &[(u16, Vec<u8>)], end: &[u8]) -> Vec<u8> {
    let siz = synth::siz_body([1000 * tiles, 1000, 992, 0, 1000, 1000, 0, 0], &[[7, 1, 1]]);
    let mut cod = synth::cod_body(1, 1, 0, 0);
    cod[0] = 1;
    cod.extend([0x00, 0x11]);
    let mut bytes = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &cod),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 4])),
    ]
    .concat();
    let own = [
        synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)),
        tile_0.to_vec(),
    ]
    .concat();
    bytes.extend(synth::tile_part(0, 0, 1, &own, &[0]));
    for (tile, header) in parts {
        bytes.extend(synth::tile_part(*tile, 0, 1, header, &[]));
    }
    bytes.extend_from_slice(end);
    bytes
}

/// A tile-part `COD` of no levels and 1x1 precincts: 10^6 precincts in a
/// tile of 1000 x 1000, over the cap.
fn cod_over_the_cap() -> Vec<u8> {
    let mut cod = synth::cod_body(1, 0, 0, 0);
    cod[0] = 1;
    cod.push(0x00);
    synth::seg(0xFF52, &cod)
}

/// That `header` refuses tile `tile` over the precinct cap: `TilePrecincts`
/// on a 64-bit target, `AddressSpace` on a 32-bit one, where the sample limit
/// that would admit the tile is past what the target can address. The message
/// names the tile on both. That pins today's behavior, not a promise: a later
/// release may give a 32-bit target the same kind.
fn assert_tile_refused<T: std::fmt::Debug>(header: &Result<T, Error>, tile: u32, context: &str) {
    let kind = if cfg!(target_pointer_width = "64") {
        LimitKind::TilePrecincts
    } else {
        LimitKind::AddressSpace
    };
    let prefix = format!("tile {tile} holds ");
    assert!(
        matches!(header, Err(Error::Limit(limit))
            if limit.kind() == kind && limit.message().starts_with(&prefix)),
        "{context}: {header:?}"
    );
}

const EOC: &[u8] = &[0xFF, 0xD9];

/// Tile 1 refused under its own header and tile 2 missing (the seed
/// `tests/fixtures/header/own_cap_then_missing.j2k`): of three tiles, tile
/// 1's tile-part `COD` keeps it over the precinct cap, and tile 2 has no
/// tile-part. With `strict` on the decode refuses the file for the missing
/// tile before it counts any tile, and the header read, which refuses tile
/// 1 first, gives that error too. With `strict` off both leave tile 2 out
/// and refuse tile 1.
#[test]
fn a_missing_tile_comes_before_a_tile_its_own_header_refuses() {
    let bytes = row(3, &[], &[(1, cod_over_the_cap())], EOC);
    assert_eq!(
        &bytes[..],
        &include_bytes!("fixtures/header/own_cap_then_missing.j2k")[..],
        "the seed is this shape"
    );
    let missing = "the codestream carries no tile-part for tile 2";
    for region in [Region::Whole, Region::Tile(1)] {
        let options = DecodeOptions::default().with_region(region);
        let header = read_header_with(&bytes, &options);
        assert!(
            matches!(&header, Err(Error::Codestream(d)) if d.message() == missing),
            "{header:?}"
        );
        assert_eq!(keyed(header), keyed(decode_with(&bytes, &options)));

        let lenient = options.with_strict(false);
        let header = read_header_with(&bytes, &lenient);
        assert_tile_refused(&header, 1, "strict off");
        assert_eq!(keyed(header), keyed(decode_with(&bytes, &lenient)));
    }
}

/// One gray tile of 1000 x 1000 whose main `COD` gives no levels and 1x1
/// precincts, over the precinct cap, with an empty tile-part, and `end`
/// after it.
fn one_tile(end: &[u8]) -> Vec<u8> {
    let siz = synth::siz_body([1000, 1000, 0, 0, 1000, 1000, 0, 0], &[[7, 1, 1]]);
    [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        cod_over_the_cap(),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8])),
        synth::tile_part(0, 0, 1, &[], &[]),
        end.to_vec(),
    ]
    .concat()
}

/// A tile refused under the main header and a fault in the tile-parts: the
/// decode refuses the file for the fault, found when it parses the
/// tile-parts before it counts any tile, and the header read gives the
/// fault too, with `strict` on and off. Decoding the tile alone gives the
/// fault as well. The faults: the chain does not end in `SOT` or `EOC` (the
/// seeds `tests/fixtures/header/strict_off_chain_fault.j2k` and
/// `tests/fixtures/header/strict_off_one_tile_chain_fault.j2k`, the shape of
/// the `header` fuzz finds of #504), or tile 0's header holds a `COD` too
/// short for its length field, or a `COD` cut short or a `COM` that runs
/// past the tile-part's `Psot`, which is shorter than its header. Without the fault, the header read refuses the tile.
#[test]
fn a_tile_parts_fault_comes_before_a_tile_refused() {
    let mut cut_cod = synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0));
    cut_cod.truncate(6);
    let mut long_com = vec![0xFF, 0x64, 0xFF, 0xF0, 0, 1];
    long_com.extend(synth::seg(0xFF52, &synth::cod_body(1, 0, 0, 0)));
    let junk = [0x00, 0x00];
    let chain = row(2, &[], &[(1, vec![])], &junk);
    assert_eq!(
        &chain[..],
        &include_bytes!("fixtures/header/strict_off_chain_fault.j2k")[..],
        "the seed is this shape"
    );
    let one = one_tile(&junk);
    assert_eq!(
        &one[..],
        &include_bytes!("fixtures/header/strict_off_one_tile_chain_fault.j2k")[..],
        "the seed is this shape"
    );
    let chain_fault = "Codestream(\"expected `SOT` or `EOC` after a tile-part, found 0x0000\")";
    let cases = [
        (chain, 1, chain_fault),
        (one, 0, chain_fault),
        (
            row(2, &cut_cod, &[(1, vec![])], EOC),
            1,
            "Codestream(\"tile 0 tile-part 0 declares `Psot` 35, shorter than its header\")",
        ),
        (
            row(2, &[0xFF, 0x52, 0x00, 0x01], &[(1, vec![])], EOC),
            1,
            "Marker(\"marker segment length below 2\")",
        ),
        (
            row(2, &long_com, &[(1, vec![])], EOC),
            1,
            "Codestream(\"tile 0 tile-part 0 declares `Psot` 49, shorter than its header\")",
        ),
    ];
    for (bytes, tile, fault) in &cases {
        for strict in [false, true] {
            let options = DecodeOptions::default().with_strict(strict);
            let fault = Err(fault.to_string());
            assert_eq!(keyed(read_header_with(bytes, &options)), fault, "{strict}");
            assert_eq!(keyed(decode_with(bytes, &options)), fault, "{strict}");
            let alone = options.clone().with_region(Region::Tile(*tile));
            assert_eq!(keyed(decode_with(bytes, &alone)), fault, "{strict}");
        }
    }
    // The same shapes with the fault mended: the header read refuses the
    // tile.
    let mended = [(row(2, &[], &[(1, vec![])], EOC), 1), (one_tile(EOC), 0)];
    for (bytes, tile) in &mended {
        for strict in [false, true] {
            let options = DecodeOptions::default().with_strict(strict);
            let header = read_header_with(bytes, &options);
            assert_tile_refused(&header, *tile, "mended");
            assert_eq!(keyed(header), keyed(decode_with(bytes, &options)));
        }
    }
}

/// The repro of batch critique 10 on #504 (the seed
/// `tests/fixtures/header/missing_after_over_cap.j2k`): tile 0 is over the
/// precinct cap and has a tile-part, and tile 1 has none. With `strict` on
/// the header read gives the decode's error for the missing tile, where it
/// once refused tile 0; with `strict` off both refuse tile 0.
#[test]
fn a_missing_tile_after_a_tile_over_a_cap_comes_first() {
    let bytes = include_bytes!("fixtures/header/missing_after_over_cap.j2k");
    let strict = DecodeOptions::default();
    let header = read_header_with(bytes, &strict);
    assert!(
        matches!(&header, Err(Error::Codestream(d))
            if d.message() == "the codestream carries no tile-part for tile 1"),
        "{header:?}"
    );
    assert_eq!(keyed(header), keyed(decode_with(bytes, &strict)));
    let lenient = strict.with_strict(false);
    let header = read_header_with(bytes, &lenient);
    assert_tile_refused(&header, 0, "strict off");
    assert_eq!(keyed(header), keyed(decode_with(bytes, &lenient)));
}

/// A strict-only fault ahead of the chain fault (the seed
/// `tests/fixtures/header/strict_off_tnsot_then_chain.j2k`, with the main
/// header of `row` for two tiles): tile 1's one tile-part comes first, then
/// tile 0 has two tile-parts that each declare one, and `00 00` stands where
/// the `EOC` should be. With `strict` off the header read and the decode, of
/// the file or of either tile alone, give the chain's fault, as a tile-part
/// past its `TNsot` is read. With `strict` on both give the extra
/// tile-part's fault, which only a strict walk refuses.
#[test]
fn a_strict_only_fault_can_come_before_the_tile_parts_fault() {
    let bytes = include_bytes!("fixtures/header/strict_off_tnsot_then_chain.j2k");
    let chain = "Codestream(\"expected `SOT` or `EOC` after a tile-part, found 0x0000\")";
    let lenient = DecodeOptions::default().with_strict(false);
    let header = read_header_with(bytes, &lenient);
    assert_eq!(keyed(header), Err(chain.to_string()));
    assert_eq!(keyed(decode_with(bytes, &lenient)), Err(chain.to_string()));
    for tile in 0..2 {
        let alone = lenient.clone().with_region(Region::Tile(tile));
        assert_eq!(keyed(decode_with(bytes, &alone)), Err(chain.to_string()));
    }
    let strict = DecodeOptions::default();
    let header = read_header_with(bytes, &strict);
    assert!(
        matches!(&header, Err(Error::Codestream(d))
            if d.message().starts_with("tile 0 tile-part 1 of a declared 1 tile-parts")),
        "{header:?}"
    );
    assert_eq!(keyed(header), keyed(decode_with(bytes, &strict)));
}

/// The seed `tests/fixtures/header/s_three_or_hole.j2k`, from the confirm
/// round of #542: three tiles over the precinct cap under the main `COD`. Tiles 0
/// and 1 are brought within by a tile-part `COD` that turns on `SOP`, and
/// each has an `SOP` of `Lsop` 5. Tile 2 is over the cap. Tile 0 declares
/// two tile-parts and carries one. With `strict` on, the header read and
/// the decode give the count's fault. With `strict` off the count is
/// advisory: the header read refuses tile 2, and the decode refuses the
/// file first for the `SOP` in tile 0's data, which only decoding reads.
/// Decoding tile 2 alone gives the header's refusal.
#[test]
fn a_count_fault_with_strict_on_and_a_data_fault_with_it_off() {
    let bytes = include_bytes!("fixtures/header/s_three_or_hole.j2k");
    let count = "Codestream(\"tile 0 declares 2 tile-parts but carries 1\")";
    let strict = DecodeOptions::default();
    assert_eq!(
        keyed(read_header_with(bytes, &strict)),
        Err(count.to_string())
    );
    assert_eq!(keyed(decode_with(bytes, &strict)), Err(count.to_string()));

    let lenient = strict.with_strict(false);
    let header = read_header_with(bytes, &lenient);
    assert_tile_refused(&header, 2, "strict off");
    let sop = "Marker(\"`SOP` declares `Lsop` 5, expected 4\")";
    assert_eq!(keyed(decode_with(bytes, &lenient)), Err(sop.to_string()));
    let alone = lenient.with_region(Region::Tile(2));
    assert_eq!(keyed(header), keyed(decode_with(bytes, &alone)));
}
