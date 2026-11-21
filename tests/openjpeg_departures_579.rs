//! Two refusals OpenJPEG 2.5.0 does not make, pinned through `decode`
//! (docs/openjpeg-departures.md: "An unknown marker in the main header" and
//! "A color transform over mixed wavelets"). Both start from
//! `multicomponent_lossless.j2k`; `opj_decompress` decodes each edit, and this
//! decoder refuses it. The refusals are this decoder's choice, so a change of
//! either variant is a decision, not a fix.

use rust_j2k::{Error, decode};

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/multicomponent_lossless.j2k"
    ))
    .unwrap()
}

fn find(bytes: &[u8], marker: [u8; 2]) -> usize {
    bytes.windows(2).position(|w| w == marker).unwrap()
}

#[test]
fn the_fixture_decodes_before_any_edit() {
    decode(&fixture()).unwrap();
}

#[test]
fn an_unknown_marker_in_the_main_header_is_unsupported() {
    let bytes = fixture();
    let cod = find(&bytes, [0xFF, 0x52]);
    let mut edited = bytes[..cod].to_vec();
    edited.extend_from_slice(&[0xFF, 0x7F, 0x00, 0x02]);
    edited.extend_from_slice(&bytes[cod..]);
    match decode(&edited) {
        Err(Error::Unsupported(m)) => assert!(
            m.message()
                .contains("unrecognized marker 0xFF7F in the main header"),
            "{m}"
        ),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn a_color_transform_over_mixed_wavelets_is_marker() {
    let mut bytes = fixture();
    let cod = find(&bytes, [0xFF, 0x52]);
    bytes[cod + 8] = 1; // SGcod's MCT byte
    let qcd = find(&bytes, [0xFF, 0x5C]);
    let lqcd = usize::from(u16::from_be_bytes([bytes[qcd + 2], bytes[qcd + 3]]));
    let end = qcd + 2 + lqcd;
    // A COC for component 1 with the 9/7, then a QCC with the derived step.
    let mut inserted = vec![
        0xFF, 0x53, 0x00, 0x09, 0x01, 0x00, 0x02, 0x02, 0x02, 0x00, 0x00,
    ];
    inserted.extend_from_slice(&[0xFF, 0x5D, 0x00, 0x06, 0x01, 0x41, 0x40, 0x00]);
    bytes.splice(end..end, inserted);
    match decode(&bytes) {
        Err(Error::Marker(m)) => assert!(m.message().contains("different wavelet"), "{m}"),
        other => panic!("expected Marker, got {other:?}"),
    }
}

/// The same mix, put in a tile-part header: a tile `COC` and `QCC` move
/// component 1 to the 9/7 under the main header's MCT. The error names the tile.
#[test]
fn a_tile_coc_that_mixes_the_wavelets_under_mct_names_the_tile() {
    let mut bytes = fixture();
    let cod = find(&bytes, [0xFF, 0x52]);
    bytes[cod + 8] = 1;
    let sot = find(&bytes, [0xFF, 0x90]);
    let mut inserted = vec![
        0xFF, 0x53, 0x00, 0x09, 0x01, 0x00, 0x02, 0x02, 0x02, 0x00, 0x00,
    ];
    inserted.extend_from_slice(&[0xFF, 0x5D, 0x00, 0x06, 0x01, 0x41, 0x40, 0x00]);
    let psot = u32::from_be_bytes(bytes[sot + 6..sot + 10].try_into().unwrap());
    assert_ne!(psot, 0, "the fixture's Psot is explicit");
    let psot = psot + u32::try_from(inserted.len()).unwrap();
    bytes[sot + 6..sot + 10].copy_from_slice(&psot.to_be_bytes());
    let at = sot + 12;
    bytes.splice(at..at, inserted);
    match decode(&bytes) {
        Err(Error::Marker(m)) => {
            assert!(
                m.message().starts_with("in tile 0, the color transform"),
                "{m}"
            );
            assert!(m.message().contains("different wavelet"), "{m}");
        }
        other => panic!("expected Marker, got {other:?}"),
    }
}
