//! A reduction checked against each component's merged level count
//! (docs/openjpeg-departures.md, "A reduction checked against the merged
//! levels").
//!
//! The `COD` of `coc_levels/levels5.j2k` is edited to 1 decomposition level
//! and a `COC` on every component puts the 5 back, so the file is the same
//! image. At a reduction of 3 `opj_decompress` 2.5.0 refuses the edit while
//! it reads the `COD` ("The number of resolutions to remove (3) is greater
//! or equal than the number of resolutions of this component (2)"). Every
//! component has 6 resolutions, so this decoder accepts it. The image is
//! compared with `opj_decompress -r 3` on the unedited file, whose `COD` has
//! 5 levels itself.

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Refined, decode_with, read_header_with};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/coc_levels/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The fixture with its `COD` at 1 level; with `coc`, a `COC` per component
/// restores the 5 levels.
fn edited(coc: bool) -> Vec<u8> {
    let bytes = fixture("levels5.j2k");
    let cod = bytes.windows(2).position(|w| w == [0xFF, 0x52]).unwrap();
    let end = cod + 2 + usize::from(u16::from_be_bytes([bytes[cod + 2], bytes[cod + 3]]));
    assert_eq!(bytes[cod + 9], 5, "levels in the source COD");
    let mut out = bytes[..end].to_vec();
    out[cod + 9] = 1;
    if coc {
        for component in 0..3 {
            // Lcoc 9, Ccoc, Scoc 0, then the source's SPcod.
            out.extend_from_slice(&[0xFF, 0x53, 0x00, 0x09, component, 0]);
            out.extend_from_slice(&bytes[cod + 9..cod + 14]);
        }
    }
    out.extend_from_slice(&bytes[end..]);
    out
}

fn options(reduction: u8) -> DecodeOptions {
    DecodeOptions::default().with_resolution_reduction(reduction)
}

/// `opj_decompress -r 3` on `levels5.j2k`: an 8x8 P6 image, as three planes.
fn oracle() -> Vec<Vec<i32>> {
    let ppm = fixture("levels5_r3_opj.ppm");
    let header = b"P6\n8 8\n255\n";
    assert_eq!(&ppm[..header.len()], header);
    let data = &ppm[header.len()..];
    (0..3)
        .map(|c| {
            data.iter()
                .skip(c)
                .step_by(3)
                .map(|&v| i32::from(v))
                .collect()
        })
        .collect()
}

fn planes(image: &rust_j2k::Image) -> Vec<Vec<i32>> {
    assert_eq!((image.width, image.height), (8, 8));
    image.components.iter().map(|c| c.samples.clone()).collect()
}

#[test]
fn the_unedited_file_matches_opj_at_a_reduction_of_3() {
    let image = decode_with(&fixture("levels5.j2k"), &options(3)).unwrap();
    assert_eq!(planes(&image), oracle());
}

#[test]
fn a_cod_too_short_for_the_reduction_is_accepted_when_every_coc_raises_it() {
    let bytes = edited(true);
    read_header_with(&bytes, &options(3)).unwrap();
    let image = decode_with(&bytes, &options(3)).unwrap();
    assert_eq!(planes(&image), oracle());

    let mut decoder = IncrementalDecoder::new(options(3)).unwrap();
    decoder.feed(&bytes);
    match decoder.refine().unwrap() {
        Refined::Image(image) => assert_eq!(planes(&image), oracle()),
        other => panic!("expected an image, got {other:?}"),
    }
    assert_eq!(planes(&decoder.finish().unwrap()), oracle());
}

#[test]
fn the_edit_is_the_same_image_at_full_size() {
    let source = decode_with(&fixture("levels5.j2k"), &DecodeOptions::default()).unwrap();
    let image = decode_with(&edited(true), &DecodeOptions::default()).unwrap();
    assert_eq!(image.components, source.components);
}

#[test]
fn without_the_cocs_the_same_reduction_is_refused() {
    let bytes = edited(false);
    match decode_with(&bytes, &options(3)) {
        Err(Error::InvalidOptions(m)) => {
            assert!(m.message().contains("resolution reduction 3"), "{m}")
        }
        other => panic!("expected InvalidOptions, got {other:?}"),
    }
    assert!(matches!(
        read_header_with(&bytes, &options(3)),
        Err(Error::InvalidOptions(_))
    ));
}
