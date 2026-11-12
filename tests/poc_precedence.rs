//! A tile-part POC overrides the main header's POC (A.6.6).
//!
//! Each codestream is a lossless `opj_compress` encode of `source.raw` with a
//! two-volume tile-part POC, with a different main-header POC spliced in. The
//! packets follow the tile-part POC, so the decode must equal the source.
//! `opj_decompress` 2.5.0 decodes both wrong: it walks the main volume first.
//! See `tests/fixtures/poc_precedence/README.md` and docs/correctness.md.

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
