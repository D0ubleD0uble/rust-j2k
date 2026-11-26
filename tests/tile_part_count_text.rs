//! The errors `strict` gives for a tile's count of tile-parts name the count
//! the tile's `SOT`s state, and say when OpenJPEG's `TPsot == TNsot`
//! correction holds the tile to one more, as `ReportEntry::
//! TilePartCountMismatch` does with `strict` off on the same bytes.
//!
//! OpenJPEG 2.5.0 (`opj_decompress`, with and without `-allow-partial`):
//! the first file decodes with exit 0 and "Non conformant codestream
//! TPsot==TNsot". The second decodes the same way, without the warning. It
//! refuses the third ("In SOT marker, TPSot (2) is not valid regards to the
//! previous number of tile-part (2)", after warning of the correction) and
//! the fourth (the same error), which `strict` here refuses too.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, ReportEntry, decode_with};
use synth::{cod_body, qcd_none_body, seg, tile_part};

/// Two 8x8 tiles of one component and their tile-parts as `(tile, TPsot,
/// TNsot)`, then `EOC`.
fn file(parts: &[(u16, u8, u8)]) -> Vec<u8> {
    let mut out = [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([16, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &cod_body(1, 0, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8])),
    ]
    .concat();
    for &(tile, part, count) in parts {
        out.extend(tile_part(tile, part, count, &[], &[0; 4]));
    }
    out.extend([0xFF, 0xD9]);
    out
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// The text of the error `strict` gives. The walk that keeps no tile
/// record, which the header read uses, must give the same one.
fn strict_error(bytes: &[u8]) -> String {
    #[cfg(feature = "__fuzz")]
    rust_j2k::fuzz::tile_parts_walks_agree(bytes);
    let error = decode_with(bytes, &options(true)).unwrap_err();
    assert!(matches!(error, Error::Codestream(_)), "{error:?}");
    error.to_string()
}

/// What `strict` off reports of the tiles' counts for the same bytes.
fn lenient_entries(bytes: &[u8]) -> Vec<String> {
    let image = decode_with(bytes, &options(false)).unwrap();
    image
        .report
        .entries
        .iter()
        .filter(|entry| matches!(entry, ReportEntry::TilePartCountMismatch { .. }))
        .map(ToString::to_string)
        .collect()
}

/// Tile 1's parts all say 2 and it carries 2, but tile 0's third part sets
/// the correction, so tile 1 is held to 3.
#[test]
fn a_tile_held_to_the_corrected_count_says_so() {
    let bytes = file(&[(0, 0, 2), (0, 1, 2), (1, 0, 2), (1, 1, 2), (0, 2, 2)]);
    let error = strict_error(&bytes);
    assert!(
        error.ends_with(
            "tile 1 declares 2 tile-parts, 3 with the `TPsot == TNsot` correction, but carries 2"
        ),
        "{error}"
    );
    assert_eq!(
        lenient_entries(&bytes),
        [
            "tile 1 declares 2 tile-part(s), 3 with the `TPsot == TNsot` correction, and 2 were found"
        ]
    );
}

/// Without the correction the message is the stated count alone.
#[test]
fn a_tile_without_the_correction_gives_the_stated_count() {
    let bytes = file(&[(0, 0, 2), (0, 1, 2), (1, 0, 3), (1, 1, 3)]);
    let error = strict_error(&bytes);
    assert!(
        error.ends_with("tile 1 declares 3 tile-parts but carries 2"),
        "{error}"
    );
    assert_eq!(
        lenient_entries(&bytes),
        ["tile 1 declares 3 tile-part(s) and 2 were found"]
    );
}

/// Tile 0 states 1 part. The look-ahead from its first part finds the second
/// with `TPsot == TNsot`, so it is held to 2, and its third is past that.
#[test]
fn a_part_past_the_corrected_count_says_so() {
    let bytes = file(&[(0, 0, 1), (0, 1, 1), (0, 2, 1)]);
    let error = strict_error(&bytes);
    assert!(
        error.ends_with(
            "tile 0 tile-part 2 of a declared 1 tile-parts, 2 with the `TPsot == TNsot` correction"
        ),
        "{error}"
    );
    assert_eq!(
        lenient_entries(&bytes),
        [
            "tile 0 declares 1 tile-part(s), 2 with the `TPsot == TNsot` correction, and 3 were found"
        ]
    );
}

/// Two counts without the correction give the counts the `SOT`s state.
#[test]
fn two_counts_without_the_correction_give_the_stated_ones() {
    let bytes = file(&[(0, 0, 2), (0, 1, 2), (0, 2, 3), (0, 3, 3)]);
    let error = strict_error(&bytes);
    assert!(
        error.ends_with("tile 0 declares 3 tile-parts in one `SOT` and 2 in another"),
        "{error}"
    );
}

/// With `strict` off the two-counts error gives the stated counts alone, on
/// a file where the look-ahead would raise the counts if it ran: tile 0's
/// first part states 1 and its second has `TPsot == TNsot`, then a part
/// states 3. Both tile-part walks give the same error, with `strict` on
/// (which adds the raised counts) and off.
#[test]
fn two_counts_strict_off_leave_the_correction_out() {
    let bytes = file(&[(0, 0, 1), (0, 1, 1), (0, 2, 3)]);
    #[cfg(feature = "__fuzz")]
    rust_j2k::fuzz::tile_parts_walks_agree(&bytes);
    let error = decode_with(&bytes, &options(false)).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("tile 0 declares 3 tile-parts in one `SOT` and 1 in another"),
        "{error}"
    );
    assert!(strict_error(&bytes).ends_with(
        "tile 0 declares 3 tile-parts in one `SOT` and 1 in another, 4 and 2 with the `TPsot == TNsot` correction"
    ));
}
