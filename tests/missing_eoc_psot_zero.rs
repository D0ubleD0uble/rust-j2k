//! `MissingEoc` for a last tile-part with `Psot` 0, with `strict` off: it is
//! reported exactly when the input does not end in `FF D9`, whether the input
//! is cut inside the tile-part's header or inside its data.

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
