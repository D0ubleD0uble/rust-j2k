//! With `strict` off, a tile is in `MissingTiles` when no tile-part of it was
//! read: none is there, or the input ends inside the first one's header,
//! before its `SOD` is whole. `TruncatedTilePart` names a tile-part from its
//! whole `SOT` on, so a tile cut in its tile-part header has both entries.
//!
//! The file is two 8x8 tiles, one tile-part each. Tile 1's tile-part is 15
//! bytes: the 12-byte `SOT`, `SOD` and one packet byte. It is cut on every
//! prefix from its first byte to the end of the file.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, decode_with};

const PART: usize = 15;
const MISSING_EOC: &str = "MissingEoc";
const MISSING_TILE_1: &str = "MissingTiles { tiles: 1..2 }";
const NO_PACKETS: &str = "MissingPackets { tile: 1, complete_layers: 0, complete_reduction: None }";

/// The report's entries as their `Debug` text: the variants are
/// `#[non_exhaustive]`, so a test outside the crate cannot build one.
fn entries(bytes: &[u8]) -> Vec<String> {
    decode_with(bytes, &DecodeOptions::default().with_strict(false))
        .unwrap_or_else(|e| panic!("{} bytes: {e}", bytes.len()))
        .report
        .entries
        .iter()
        .map(|entry| format!("{entry:?}"))
        .collect()
}

fn truncated(present: usize, header_cut: bool) -> String {
    format!(
        "TruncatedTilePart {{ tile: 1, tile_part: 0, declared: Some({PART}), \
         present: {present}, header_cut: {header_cut} }}"
    )
}

#[test]
fn a_cut_through_tile_1s_only_tile_part_is_reported_by_how_much_is_there() {
    // Two 8x8 tiles, one packet each; the file ends in `EOC`.
    let bytes = synth::empty_tiles([16, 8, 0, 0, 8, 8, 0, 0], 0, 3, &[], &[1, 1], 0);
    let start = bytes.len() - 2 - PART;
    assert_eq!(&bytes[start..start + 2], &[0xFF, 0x90], "tile 1's SOT");
    for present in 0..=PART + 2 {
        let got = entries(&bytes[..start + present]);
        let want: Vec<String> = match present {
            // Inside the `SOT`, or before it: no entry names the bytes, and
            // the tile has no tile-part.
            0..=11 => vec![MISSING_EOC.into(), MISSING_TILE_1.into()],
            // The `SOT` is whole and the input ends inside the header: the
            // tile-part is reported, but none of it is read, so the tile is
            // missing as well.
            12 | 13 => vec![
                truncated(present, true),
                MISSING_EOC.into(),
                MISSING_TILE_1.into(),
            ],
            // `SOD` is whole: the tile-part is read, with no packet bytes.
            14 => vec![truncated(14, false), MISSING_EOC.into(), NO_PACKETS.into()],
            // The tile-part is whole; only the `EOC` is partly or wholly
            // gone.
            15 | 16 => vec![MISSING_EOC.into()],
            _ => vec![],
        };
        assert_eq!(got, want, "tile 1's tile-part cut at {present} of {PART}");
    }
}

/// The rule is the tile's first tile-part, not its only one: with `TNsot` 2
/// and the first tile-part cut in its header, the tile is still missing.
#[test]
fn a_tile_whose_first_of_two_tile_parts_is_cut_in_its_header_is_missing() {
    let one = synth::empty_tiles([16, 8, 0, 0, 8, 8, 0, 0], 0, 3, &[], &[1, 1], 0);
    let tile_1 = one.len() - 2 - PART;
    let mut bytes = one[..tile_1].to_vec();
    bytes.extend(synth::tile_part(1, 0, 2, &[], &[0]));
    bytes.extend(synth::tile_part(1, 1, 2, &[], &[0]));
    for present in [12, 13] {
        assert_eq!(
            entries(&bytes[..tile_1 + present]),
            vec![
                truncated(present, true),
                MISSING_EOC.into(),
                MISSING_TILE_1.into()
            ],
            "cut at {present} of {PART}"
        );
    }
}
