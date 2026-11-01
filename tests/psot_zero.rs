//! A `Psot = 0` last tile-part, with and without the closing EOC.
//!
//! OpenJPEG 2.5.0 gives a `Psot = 0` tile-part everything up to the last two
//! bytes of the codestream. It does not check that those bytes are FF D9 (it
//! warns). A codestream whose EOC is missing therefore loses two data bytes
//! and fails in tile decoding. Every case here was run through
//! `opj_decompress` 2.5.0; see `tests/fixtures/psot0/README.md`.

use std::path::PathBuf;

use rust_j2k::{Error, decode};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/psot0")
}

/// The samples of an 8-bit PGM (`P5`) from `opj_decompress`: the last
/// `width * height` bytes.
fn pgm_samples(name: &str) -> Vec<i32> {
    let bytes = std::fs::read(dir().join(name)).unwrap();
    bytes[bytes.len() - 48 * 40..]
        .iter()
        .map(|&b| i32::from(b))
        .collect()
}

fn decode_file(name: &str) -> rust_j2k::Result<rust_j2k::Image> {
    decode(&std::fs::read(dir().join(name)).unwrap())
}

/// Decodes, equal to the OpenJPEG output in `expected`.
fn assert_decodes(name: &str, expected: &str) {
    let image = decode_file(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.components.len(), 1, "{name}");
    assert_eq!(image.components[0].samples, pgm_samples(expected), "{name}");
}

#[test]
fn psot_zero_with_eoc_decodes() {
    assert_decodes("single_psot0_eoc.j2k", "single.expected.pgm");
    assert_decodes("tiled_psot0_eoc.j2k", "tiled.expected.pgm");
    assert_decodes("tileparts_psot0_eoc.j2k", "tileparts.expected.pgm");
}

/// Two junk bytes where the EOC should be: OpenJPEG warns and decodes.
#[test]
fn psot_zero_with_junk_in_place_of_eoc_decodes() {
    assert_decodes("single_psot0_junk_tail.j2k", "single.expected.pgm");
    assert_decodes("tiled_psot0_junk_tail.j2k", "tiled.expected.pgm");
}

/// No EOC: the last two data bytes are taken for it. OpenJPEG fails with
/// "segment too long" while decoding the tile; here the packet walk runs out
/// of data, a typed codestream error.
#[test]
fn psot_zero_without_eoc_fails_in_the_tile() {
    for name in [
        "single_psot0_no_eoc.j2c",
        "single_psot0_no_eoc_trunc100.j2c",
        "tiled_psot0_no_eoc.j2c",
        "tileparts_psot0_no_eoc.j2c",
    ] {
        match decode_file(name) {
            Err(Error::Codestream(m)) => {
                assert!(m.contains("overruns the tile-part"), "{name}: {m}");
            }
            other => panic!("{name}: expected a codestream error, got {other:?}"),
        }
    }
}

/// Fewer than two bytes after the tile-part header: OpenJPEG fails with
/// "Tile part length size inconsistent with stream length".
#[test]
fn psot_zero_with_no_room_for_the_tail_is_codestream() {
    assert!(matches!(
        decode_file("single_psot0_one_byte_after_sod.j2c"),
        Err(Error::Codestream(m)) if m.contains("fewer than two bytes")
    ));
}
