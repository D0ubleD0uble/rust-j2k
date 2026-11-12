//! An RGN maxshift with a bypass code-block style.
//!
//! Each codestream is a lossless `opj_compress -ROI c=0,U=3` encode of
//! `source.pgm` with bypass alone, bypass + termination on every pass, bypass +
//! segmentation symbols, or bypass + predictable termination. The decode must
//! equal the source. `opj_decompress` 2.5.0 decodes all four wrong: it
//! switches to raw coding `SPrgn` planes too late. See
//! `tests/fixtures/roi_bypass/README.md` and docs/correctness.md.

use std::path::PathBuf;

use rust_j2k::decode;

const WIDTH: usize = 64;
const HEIGHT: usize = 64;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/roi_bypass")
}

/// The samples of the 8-bit `P5` source image.
fn source_samples() -> Vec<i32> {
    let bytes = std::fs::read(dir().join("source.pgm")).unwrap();
    let header = b"P5\n64 64\n255\n";
    assert_eq!(&bytes[..header.len()], header, "source.pgm header");
    assert_eq!(
        bytes.len(),
        header.len() + WIDTH * HEIGHT,
        "source.pgm size"
    );
    bytes[header.len()..]
        .iter()
        .map(|&b| i32::from(b))
        .collect()
}

fn assert_round_trips(name: &str) {
    let image =
        decode(&std::fs::read(dir().join(name)).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        (image.width, image.height),
        (WIDTH as u32, HEIGHT as u32),
        "{name}"
    );
    assert_eq!(image.components.len(), 1, "{name}");
    let component = &image.components[0];
    assert_eq!(
        (component.bit_depth, component.signed),
        (8, false),
        "{name}"
    );
    assert!(
        component.samples == source_samples(),
        "{name}: the decode differs from source.pgm"
    );
}

#[test]
fn maxshift_with_bypass_round_trips() {
    assert_round_trips("roi_bypass_m1.j2k");
}

/// Every pass is its own segment here, so a segment opens at the significance
/// pass of the fourth coded plane too. Of the four fixtures here, it is the
/// only one that tells a raw decision one plane early from the right one.
#[test]
fn maxshift_with_bypass_and_termination_on_every_pass_round_trips() {
    assert_round_trips("roi_bypass_m5.j2k");
}

#[test]
fn maxshift_with_bypass_and_segmentation_symbols_round_trips() {
    assert_round_trips("roi_bypass_m33.j2k");
}

#[test]
fn maxshift_with_bypass_and_predictable_termination_round_trips() {
    assert_round_trips("roi_bypass_m17.j2k");
}
