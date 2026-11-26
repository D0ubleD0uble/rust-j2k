//! A tile with more tile-parts than `TNsot` states, even after OpenJPEG's
//! `TPsot == TNsot` correction (docs/openjpeg-departures.md, "A tile with
//! more tile-parts than `TNsot`"). With `strict` off every part found is
//! decoded and the tile reports `TilePartCountMismatch`; with `strict` on the
//! file is refused.
//!
//! Both files come from `tnsot_extra_part/`, three tile-parts per tile
//! (`opj_compress -TP R`), with `TNsot` edited to 1 on the tile that carries
//! the extra part. `opj_decompress` 2.5.0, with and without
//! `-allow-partial`, warns "Non conformant codestream TPsot==TNsot." and then:
//!
//! - one tile, nothing after it: decodes from the first two parts and drops
//!   the third (917 of 1024 samples differ from the full image), exit 0;
//! - another tile follows: refuses with "In SOT marker, TPSot (2) is not
//!   valid regards to the previous number of tile-part (2), giving up".
//!
//! `image.pgm` is `opj_decompress` on the unedited files (both give the same
//! image, which is also the source).

use rust_j2k::{DecodeOptions, Error, ReportEntry, decode_with};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/tnsot_extra_part/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The oracle's 32x32 samples.
fn oracle() -> Vec<i32> {
    let pgm = fixture("image.pgm");
    let header = b"P5\n32 32\n255\n";
    assert_eq!(&pgm[..header.len()], header);
    pgm[header.len()..].iter().map(|&v| i32::from(v)).collect()
}

/// The file with `TNsot` of each tile-part set from `counts`, in file order.
fn with_counts(name: &str, counts: &[u8]) -> Vec<u8> {
    let mut bytes = fixture(name);
    let mut at = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    for &count in counts {
        assert_eq!(bytes[at..at + 2], [0xFF, 0x90]);
        bytes[at + 11] = count;
        at += u32::from_be_bytes(bytes[at + 6..at + 10].try_into().unwrap()) as usize;
    }
    assert_eq!(bytes[at..], [0xFF, 0xD9], "every tile-part was edited");
    bytes
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// The count entries of tile `tile`, as text.
fn entries(report: &[ReportEntry], tile: u32) -> Vec<String> {
    report
        .iter()
        .filter(|e| matches!(e, ReportEntry::TilePartCountMismatch { tile: t, .. } if *t == tile))
        .map(ToString::to_string)
        .collect()
}

fn pinned(bytes: &[u8], tile: u32, strict_error: &str) {
    let image = decode_with(bytes, &options(false)).unwrap();
    assert_eq!(image.components[0].samples, oracle());
    assert_eq!(
        entries(&image.report.entries, tile),
        [
            "tile 0 declares 1 tile-part(s), 2 with the `TPsot == TNsot` correction, and 3 were found"
        ]
    );
    match decode_with(bytes, &options(true)) {
        Err(Error::Codestream(m)) => assert!(m.message().ends_with(strict_error), "{m}"),
        other => panic!("expected Codestream, got {other:?}"),
    }
}

const REFUSED: &str =
    "tile 0 tile-part 2 of a declared 1 tile-parts, 2 with the `TPsot == TNsot` correction";

/// The unedited files decode to the oracle's image, so the edits below are
/// the only difference.
#[test]
fn the_unedited_files_decode_to_the_oracle() {
    for name in ["tile1_tp3.j2k", "tiles2_tp3.j2k"] {
        let image = decode_with(&fixture(name), &options(true)).unwrap();
        assert_eq!(image.components[0].samples, oracle(), "{name}");
    }
}

/// One tile, three parts, each stating 1: OpenJPEG drops the third part.
#[test]
fn a_last_tile_with_an_extra_part_decodes_whole() {
    pinned(&with_counts("tile1_tp3.j2k", &[1, 1, 1]), 0, REFUSED);
}

/// Tile 0 states 1 and carries 3, tile 1 follows: OpenJPEG refuses.
#[test]
fn an_extra_part_before_another_tile_decodes_whole() {
    pinned(
        &with_counts("tiles2_tp3.j2k", &[1, 1, 1, 3, 3, 3]),
        0,
        REFUSED,
    );
}
