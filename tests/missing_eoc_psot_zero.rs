//! `MissingEoc` for a last tile-part with `Psot` 0, with `strict` off: it is
//! reported exactly when the input does not end in `FF D9`, whether the input
//! is cut inside the tile-part's header or inside its data.

#![cfg(feature = "__fuzz")]

#[path = "support/synth.rs"]
mod synth;
use synth::{cod_body, qcd_none_body, seg, siz_body, tile_part};

use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined, ReportEntry, decode_with};

/// The `SOT` of the fixture's last tile-part.
const LAST_SOT: usize = 5854;

/// `tiles_multipart_lossless.j2k` with `Psot` 0 in its last tile-part.
fn fixture() -> Vec<u8> {
    let mut bytes = std::fs::read("tests/fixtures/tiles_multipart_lossless.j2k").unwrap();
    assert_eq!(bytes[LAST_SOT..LAST_SOT + 2], [0xFF, 0x90]);
    bytes[LAST_SOT + 6..LAST_SOT + 10].fill(0);
    bytes
}

fn count_missing_eoc(entries: &[ReportEntry]) -> usize {
    entries
        .iter()
        .filter(|entry| matches!(entry, ReportEntry::MissingEoc { .. }))
        .count()
}

#[test]
fn every_cut_of_a_psot_zero_tile_part_reports_the_missing_eoc() {
    let bytes = fixture();
    let options = DecodeOptions::default().with_strict(false);
    for end in LAST_SOT..=bytes.len() {
        let cut = &bytes[..end];
        rust_j2k::fuzz::tile_parts_walks_agree(cut);
        let image = decode_with(cut, &options).unwrap_or_else(|e| panic!("cut {end}: {e}"));
        let entries = &image.report.entries;
        assert_eq!(
            count_missing_eoc(entries),
            usize::from(!cut.ends_with(&[0xFF, 0xD9])),
            "cut {end}: {entries:?}"
        );
    }
}

#[test]
fn an_incremental_decode_reports_the_same_missing_eoc() {
    let bytes = fixture();
    let options = DecodeOptions::default().with_strict(false);
    for end in [LAST_SOT + 12, LAST_SOT + 100, bytes.len() - 2, bytes.len()] {
        let cut = &bytes[..end];
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        decoder.feed(cut);
        let Ok(Refined::Image(image)) = decoder.refine() else {
            panic!("cut {end}: no image");
        };
        assert_eq!(
            count_missing_eoc(&image.report.entries),
            usize::from(!cut.ends_with(&[0xFF, 0xD9])),
            "cut {end}"
        );
    }
}

/// A two-tile codestream (8 x 8 tiles, one component) with a `PPM` of
/// `chunks` chunks, then `parts`, then an `EOC`. Tile 1's tile-part has
/// `Psot` 0. Returns it and the offset of tile 1's `SOT`.
fn two_tiles(chunks: usize, header: &[u8], data: &[u8]) -> (Vec<u8>, usize) {
    let mut out = vec![0xFF, 0x4F];
    out.extend(seg(
        0xFF51,
        &siz_body([16, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
    ));
    out.extend(seg(0xFF52, &cod_body(1, 0, 4, 0)));
    out.extend(seg(0xFF5C, &qcd_none_body(2, &[8])));
    let mut ppm = vec![0];
    ppm.extend(std::iter::repeat_n([0, 0, 0, 1, 0], chunks).flatten());
    out.extend(seg(0xFF60, &ppm));
    out.extend(tile_part(0, 0, 1, &[], &[]));
    let at = out.len();
    let mut part = tile_part(1, 0, 1, header, data);
    part[6..10].fill(0);
    out.extend(part);
    out.extend([0xFF, 0xD9]);
    (out, at)
}

/// A header cut of a last tile-part with `Psot` 0 whose bytes happen to end
/// in `FF D9` is read as a cut like any other, so an exact `PPM` is not
/// refused at that one length.
#[test]
fn a_header_cut_ending_in_ffd9_is_read_as_a_cut() {
    let com = seg(0xFF64, &[0, 1, b'a', 0xFF, 0xD9, b'b']);
    let (bytes, at) = two_tiles(2, &com, &[]);
    let options = DecodeOptions::default().with_strict(false);
    let mut ended_in_ffd9 = false;
    for end in at..=bytes.len() {
        let cut = &bytes[..end];
        rust_j2k::fuzz::tile_parts_walks_agree(cut);
        let image = decode_with(cut, &options).unwrap_or_else(|e| panic!("cut {end}: {e}"));
        assert_eq!(
            count_missing_eoc(&image.report.entries),
            usize::from(!cut.ends_with(&[0xFF, 0xD9])),
            "cut {end}"
        );
        ended_in_ffd9 |= end < bytes.len() && cut.ends_with(&[0xFF, 0xD9]);
    }
    assert!(ended_in_ffd9);
}

/// A `PPM` with a chunk more than the codestream has tile-parts is refused
/// when the last tile-part, with `Psot` 0, is whole in its header, and read
/// as a cut when the input ends inside that header.
#[test]
fn surplus_ppm_chunks_are_refused_unless_the_header_is_cut() {
    let (bytes, at) = two_tiles(3, &[], &[0; 6]);
    let options = DecodeOptions::default().with_strict(false);
    for end in at + 12..=bytes.len() {
        let cut = &bytes[..end];
        rust_j2k::fuzz::tile_parts_walks_agree(cut);
        let result = decode_with(cut, &options);
        if end < at + 14 {
            let entries = result
                .unwrap_or_else(|e| panic!("cut {end}: {e}"))
                .report
                .entries;
            assert!(
                entries
                    .iter()
                    .any(|e| matches!(e, ReportEntry::TruncatedTilePart { .. })),
                "cut {end}: {entries:?}"
            );
            assert_eq!(count_missing_eoc(&entries), 1, "cut {end}");
        } else {
            let error = result.expect_err(&format!("cut {end}")).to_string();
            assert!(error.contains("`PPM` carries 3"), "cut {end}: {error}");
        }
    }
}

/// Input that ends right after tile 0's tile-part, or one byte into the next
/// `SOT`, holds one tile-part of the two an exact `PPM` has chunks for. It
/// reads as a cut, not as a `PPM` with chunks left over.
#[test]
fn a_cut_after_the_first_tile_part_keeps_an_exact_ppm() {
    let (bytes, at) = two_tiles(2, &[], &[0; 6]);
    let options = DecodeOptions::default().with_strict(false);
    for end in [at, at + 1] {
        let cut = &bytes[..end];
        rust_j2k::fuzz::tile_parts_walks_agree(cut);
        let image = decode_with(cut, &options).unwrap_or_else(|e| panic!("cut {end}: {e}"));
        assert_eq!(count_missing_eoc(&image.report.entries), 1, "cut {end}");
    }
}
