//! A tile-part POC overrides the main header's POC (A.6.6).
//!
//! Each codestream is a lossless `opj_compress` encode of `source.raw` with a
//! two-volume tile-part POC, with a main-header POC spliced in. The packets
//! follow the tile-part POC, so the decode must equal the source.
//! `opj_decompress` 2.5.0 decodes the first two wrong: it walks the main
//! volume first.
//! See `tests/fixtures/poc_precedence/README.md` and docs/openjpeg-departures.md.

use std::path::PathBuf;

use rust_j2k::decode;

const WIDTH: usize = 24;
const HEIGHT: usize = 16;
const COMPONENTS: usize = 3;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/poc_precedence")
}

fn assert_round_trips(name: &str) {
    let source = std::fs::read(dir().join("source.raw")).unwrap();
    assert_eq!(source.len(), WIDTH * HEIGHT * COMPONENTS, "source.raw size");
    let image =
        decode(&std::fs::read(dir().join(name)).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        (image.width, image.height),
        (WIDTH as u32, HEIGHT as u32),
        "{name}"
    );
    assert_eq!(image.components.len(), COMPONENTS, "{name}");
    for (c, (component, plane)) in image
        .components
        .iter()
        .zip(source.chunks(WIDTH * HEIGHT))
        .enumerate()
    {
        assert_eq!(
            (component.bit_depth, component.signed),
            (8, false),
            "{name}"
        );
        let expected: Vec<i32> = plane.iter().map(|&b| i32::from(b)).collect();
        assert!(
            component.samples == expected,
            "{name}: component {c} differs from source.raw"
        );
    }
}

/// Both tile volumes sit in the first tile-part's POC.
#[test]
fn a_tile_part_poc_overrides_the_main_header_poc() {
    assert_round_trips("poc_main_and_tile.j2k");
}

/// The tile's volumes are split over its two tile-parts. Together they
/// override the main header's POC.
#[test]
fn a_tile_poc_split_over_tile_parts_overrides_the_main_header_poc() {
    assert_round_trips("poc_main_and_split_tile.j2k");
}

/// The tile's first tile-part has no POC, which A.6.6 forbids, so its first
/// packets follow the main POC. The main volume is walked first and the
/// later tile-part's volume after it, as `opj_decompress` 2.5.0 reads the file.
#[test]
fn a_poc_first_seen_in_a_later_tile_part_follows_the_main_header_poc() {
    assert_round_trips("poc_main_then_later_tile_part.j2k");
}

/// The coverage check reads the volumes the tile walks. The main POC covers
/// every packet, but the tile's own POC replaces it and leaves out
/// resolutions 1 and 2, so the tile is refused before any packet is read.
/// Checking the main and tile volumes together would pass it.
#[test]
fn the_coverage_check_sees_only_the_tile_volumes() {
    let name = "poc_tile_leaves_out_packets.j2k";
    let error = decode(&std::fs::read(dir().join(name)).unwrap())
        .expect_err("the tile's volumes leave out packets");
    let message = error.to_string();
    assert!(
        matches!(error, rust_j2k::Error::Codestream(_))
            && message.contains("tile 0 leave out its packets of component 0, resolution 1"),
        "{name}: {message}"
    );
}
