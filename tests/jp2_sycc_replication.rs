//! sYCC chroma replication under `ColorOutput::Srgb` against `opj_decompress`,
//! at both origin parities and one resolution level down.
//!
//! OpenJPEG keys its replication on the image origin's parity. Under `-r 1`
//! it still takes the parity of the full-resolution origin (`XOsiz`,
//! `YOsiz`), not that of the reduced image's origin (`ceil(XOsiz / 2)`), and
//! so must this decoder. One level down, the odd fixtures' origin (5, 3)
//! becomes (3, 2) and the even fixtures' (6, 2) becomes (3, 1): the parity
//! flips in y for the odd ones and in x and y for the even ones, so the
//! 4:2:0 fixtures and the even 4:2:2 one tell the two rules apart.
//!
//! Each snapshot is `opj_decompress` on the `.jp2` (with `-r 1` for
//! `<name>.srgb_r1.json`), graded within 1: OpenJPEG's formula truncates and
//! uses 0.344 and 0.714, and the decoder rounds with the IEC constants (an
//! owner-approved deviation). Provenance is in the fixtures README.

mod support;

use std::path::PathBuf;

use rust_j2k::{ColorOutput, ColorSpace, DecodeOptions, decode_with};
use support::{Expected, Tolerance, compare};

const ODD: [&str; 2] = ["jp2_sycc_420_odd", "jp2_sycc_422_odd"];
const EVEN: [&str; 2] = ["jp2_sycc_420_even", "jp2_sycc_422_even"];

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Decode `name.jp2` as sRGB at `reduction` and grade it against
/// `name.snapshot.json`: the image size, each channel's size, then every
/// sample within 1.
fn grade(name: &str, snapshot: &str, reduction: u8) {
    let bytes = std::fs::read(fixtures().join(format!("{name}.jp2"))).unwrap();
    let text = std::fs::read_to_string(fixtures().join(format!("{name}.{snapshot}.json")))
        .unwrap_or_else(|e| panic!("{name}.{snapshot}.json: {e}"));
    let want = Expected::from_json(&text).unwrap();
    assert_eq!(
        want.tolerance,
        Tolerance::Absolute { max_abs_error: 1.0 },
        "{name}.{snapshot}"
    );
    let options = DecodeOptions::default()
        .with_color(ColorOutput::Srgb)
        .with_resolution_reduction(reduction);
    let image = decode_with(&bytes, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    if let Err(mismatch) = compare(&image, &want) {
        panic!("{name}.{snapshot}: {mismatch}");
    }
    let jp2 = image.jp2.as_ref().expect("JP2 metadata");
    assert_eq!(jp2.color, ColorSpace::Srgb, "{name}.{snapshot}");
}

/// At an even origin no row or column takes chroma of 0: every luma sample
/// pairs with the chroma sample above and to its left.
#[test]
fn even_origin_matches_openjpeg_within_one() {
    for name in EVEN {
        grade(name, "srgb", 0);
    }
}

/// One level down, the output matches `opj_decompress -r 1` in size and to
/// within 1 on every channel (7x6 for the odd 4:2:0 fixture, 6x2 for the odd
/// 4:2:2 one).
#[test]
fn reduced_matches_openjpeg_within_one() {
    for name in ODD.into_iter().chain(EVEN) {
        grade(name, "srgb_r1", 1);
    }
}
