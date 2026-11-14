//! COC and QCC apply to their component wherever they sit in the header
//! (A.6), in the main header and in a tile-part header.
//!
//! Each stream holds a COD and QCD that are wrong for every component, and a
//! COC and QCC per component that carry the right values. In the `_usual`
//! files the COC and QCC follow the COD and QCD. In the `_reordered` files
//! they come first. Both must decode to `multicomponent_lossless.j2k`'s image.
//! `opj_decompress` 2.5.0 decodes the `_reordered` files to a different image:
//! it resolves by position, so the COD and QCD that follow wipe the COC and
//! QCC out. See `tests/fixtures/override_order/README.md` and
//! docs/openjpeg-departures.md.
//!
//! The images are compared with `assert!(a == b)`, not `assert_eq!`: a failure
//! would print every sample.

use std::path::PathBuf;

use rust_j2k::{Image, decode};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn decode_file(path: PathBuf) -> Image {
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn decode_override(name: &str) -> Image {
    decode_file(fixtures().join("override_order").join(name))
}

fn source() -> Image {
    decode_file(fixtures().join("multicomponent_lossless.j2k"))
}

#[test]
fn the_usual_order_decodes_to_the_source() {
    let source = source();
    for name in ["main_usual.j2k", "tile_usual.j2k"] {
        assert!(decode_override(name) == source, "{name}");
    }
}

#[test]
fn coc_and_qcc_before_the_cod_and_qcd_in_the_main_header_still_apply() {
    assert!(decode_override("main_reordered.j2k") == source());
}

#[test]
fn coc_and_qcc_before_the_cod_and_qcd_in_a_tile_part_header_still_apply() {
    assert!(decode_override("tile_reordered.j2k") == source());
}
