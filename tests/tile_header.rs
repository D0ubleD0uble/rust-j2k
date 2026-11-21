//! Tile headers cost their own markers, not the component count: many 1x1
//! tiles over many components, or behind a large TLM, are read in time
//! proportional to the file. See `tests/fixtures/tile_header/README.md`.
//!
//! The committed files are written by `scripts/gen-tile-header-fixtures.py`.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{Error, decode};

fn read(name: &str) -> Vec<u8> {
    synth::read_file(synth::fixtures("tile_header").join(name))
}

/// 256 1x1 tiles over 256 components, 255 of them subsampled 255 times both
/// ways, so with no sample in the image: refused with `SIZ`, as a header
/// fault, before any tile header is read. OpenJPEG 2.5.0 fails to decode
/// them. Under the light header, the heavy one (32 levels, 97 steps), and
/// with a COD and a QCD in every tile, which land on every component.
#[test]
fn many_tiles_over_components_with_no_sample_are_refused_at_siz() {
    for name in [
        "tilecomp_16_256.j2k",
        "tilecomp_heavy_16_256.j2k",
        "tilecomp_override_16_256.j2k",
    ] {
        match decode(&read(name)) {
            Err(Error::Marker(m)) => {
                assert_eq!(
                    m.message(),
                    "component 1 holds no sample in the image",
                    "{name}"
                )
            }
            other => panic!("{name}: expected component 1 refused, got {other:?}"),
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
