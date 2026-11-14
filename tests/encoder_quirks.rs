//! Three encoder quirks OpenJPEG 2.5.0 corrects on purpose, and this decoder
//! with it (docs/openjpeg-departures.md, "Encoder mistakes OpenJPEG corrects"):
//!
//! - every tile's last part with `TPsot == TNsot` (Adobe Photoshop's plug-in);
//! - the main COD and QCD restated in every later tile-part (BAP Image
//!   Systems);
//! - the color transform flag on a one-component image.
//!
//! The fixtures are `opj_compress` encodes rewritten to carry each quirk, by
//! `scripts/gen-encoder-quirk-fixtures.py`; their snapshots are
//! `opj_decompress` 2.5.0's output, equal to the source image.

mod support;

use std::path::PathBuf;

use rust_j2k::{Error, decode, read_header};
use support::{Outcome, discover, run_fixture};

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}.j2k"))
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(path(name)).unwrap()
}

/// Decodes, sample for sample equal to `opj_decompress`, and `read_header`
/// describes it.
fn matches_the_oracle(name: &str) {
    let fixtures = discover(path(name).parent().unwrap());
    let fixture = fixtures.iter().find(|f| f.name == name).unwrap();
    match run_fixture(fixture) {
        Outcome::Passed => {}
        other => panic!("{name}: {other:?}"),
    }
    read_header(&bytes(name)).unwrap_or_else(|e| panic!("{name}: read_header: {e}"));
}

#[test]
fn a_last_tpsot_equal_to_tnsot_decodes_as_openjpeg_decodes_it() {
    matches_the_oracle("quirk_tpsot_tnsot_lossless");
}

#[test]
fn a_cod_and_qcd_restated_in_later_tile_parts_decode_as_openjpeg_decodes_them() {
    matches_the_oracle("quirk_restated_cod_qcd_lossless");
}

#[test]
fn a_color_transform_on_one_component_is_skipped_as_openjpeg_skips_it() {
    matches_the_oracle("quirk_mct_one_component_lossless");
}

/// The offsets of the restated COD and QCD segments in the later tile-parts:
/// every occurrence of the marker after the first SOT.
fn later_segments(bytes: &[u8], marker: [u8; 2]) -> Vec<usize> {
    let sot = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let found: Vec<usize> = bytes
        .windows(2)
        .enumerate()
        .skip(sot)
        .filter(|(_, w)| *w == marker)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        found.len(),
        2,
        "one restatement in each of the two later parts"
    );
    found
}

/// Flip `bits` of the restated byte at `offset` past the marker in the last
/// part, and expect the decode to refuse the codestream for it.
fn a_changed_restatement_is_refused(marker: [u8; 2], offset: usize, bits: u8, names: &str) {
    let mut changed = bytes("quirk_restated_cod_qcd_lossless");
    let at = *later_segments(&changed, marker).last().unwrap() + offset;
    changed[at] ^= bits;
    let e = decode(&changed).expect_err("a restatement that differs");
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains(names)),
        "got {e:?}"
    );
}

/// The COD's layer count (its low byte): three layers where one is in force.
#[test]
fn a_later_cod_that_differs_is_refused() {
    a_changed_restatement_is_refused([0xFF, 0x52], 7, 0x02, "a `COD` that differs");
}

/// The QCD's first exponent (the byte's top five bits), one more.
#[test]
fn a_later_qcd_that_differs_is_refused() {
    a_changed_restatement_is_refused([0xFF, 0x5C], 5, 0x08, "a `QCD` that differs");
}
