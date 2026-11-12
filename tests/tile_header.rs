//! Tile headers cost their own markers, not the component count: many 1x1
//! tiles over many components, or behind a large TLM, are read in time
//! proportional to the file. See `tests/fixtures/tile_header/README.md`.
//!
//! The committed files are written by `scripts/gen-tile-header-fixtures.py`.

use std::path::PathBuf;

use rust_j2k::{Error, decode};

fn read(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tile_header")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// 256 1x1 tiles over 256 components, 255 of them subsampled 255 times both
/// ways, so empty on tile 0: refused there, as OpenJPEG 2.5.0 refuses them.
/// Under the light header, the heavy one (32 levels, 97 steps), and with a
/// COD and a QCD in every tile, which land on every component.
#[test]
fn many_tiles_over_many_components_are_refused_at_tile_0() {
    for name in [
        "tilecomp_16_256.j2k",
        "tilecomp_heavy_16_256.j2k",
        "tilecomp_override_16_256.j2k",
    ] {
        match decode(&read(name)) {
            Err(Error::Codestream(m)) => {
                assert_eq!(m, "tile 0 has an empty area on component 1", "{name}")
            }
            other => panic!("{name}: expected the empty tile 0, got {other:?}"),
        }
    }
}

/// 1024 1x1 tiles of one component after 16 KB of TLM entries decode to the
/// flat 128 OpenJPEG 2.5.0 decodes: the entries are checked and dropped, not
/// copied for each tile.
#[test]
fn a_large_tlm_decodes() {
    let image = decode(&read("tlm_32_16k.j2k")).expect("decodes");
    assert_eq!(image.components.len(), 1);
    let component = &image.components[0];
    assert_eq!((component.width, component.height), (32, 32));
    assert!(component.samples.iter().all(|&s| s == 128), "not flat 128");
}
