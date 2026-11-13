//! A 16-bit 9/7 codestream: how far this decoder is from OpenJPEG, and why.
//!
//! The inverse 9/7 here scales high-pass samples by the exact `2/K`. OpenJPEG
//! 2.5.0 uses `two_invK = 1.625732422`, 5.4e-5 (3.3e-5 relative) short of it.
//! The error that constant adds grows with the sample range, so at 16 bits the
//! two decoders differ by more than 1 (see "When the oracle and the standard
//! disagree" and the measured gaps per bit depth in `docs/correctness.md`).
//!
//! `irreversible_16bit.j2k` holds two 16-bit components of 3x5 blocks, coded by
//! `opj_compress -I` with every bit-plane and five decomposition levels
//! (`scripts/gen-irreversible-16bit-fixture.py`). Component 0 swings over the
//! full range; the output clamp hides any overshoot there, so it is a weaker
//! check against the input. Component 1 swings from a quarter to three quarters
//! of the range and never clamps. The corpus test (`tests/conformance.rs`)
//! grades both within the snapshot's tolerance. The tests here pin the rest:
//! the difference is more than 1 and within that tolerance, and this decoder's
//! output is closer to the image the encoder was given than OpenJPEG's is.

mod support;

use std::path::PathBuf;

use support::{Expected, Tolerance};

const DEPTH: u32 = 16;
const COMPONENTS: usize = 2;

fn fixture() -> (Vec<Vec<i32>>, Expected) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let bytes = std::fs::read(dir.join("irreversible_16bit.j2k")).unwrap();
    let text = std::fs::read_to_string(dir.join("irreversible_16bit.expected.json")).unwrap();
    let expected = Expected::from_json(&text).unwrap();
    let image = rust_j2k::decode(&bytes).expect("the 16-bit 9/7 fixture decodes");
    assert_eq!(image.components.len(), COMPONENTS);
    assert_eq!(expected.components.len(), COMPONENTS);
    let samples = image
        .components
        .iter()
        .map(|component| {
            assert_eq!(
                (component.bit_depth, component.signed),
                (DEPTH as u8, false)
            );
            component.samples.clone()
        })
        .collect();
    (samples, expected)
}

/// The image the encoder was given: blocks 3 wide and 5 tall, full swing on
/// component 0 and a quarter to three quarters of the range on component 1.
/// The generator script computes the same values.
fn original(component: usize, width: u32, height: u32) -> Vec<i32> {
    let top = (1i32 << DEPTH) - 1;
    let (low, high) = if component == 0 {
        (0, top)
    } else {
        (top / 4, 3 * top / 4)
    };
    (0..height)
        .flat_map(|y| (0..width).map(move |x| if (x / 3 + y / 5) % 2 == 1 { high } else { low }))
        .collect()
}

fn peak(a: &[i32], b: &[i32]) -> i32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(&x, &y)| (x - y).abs()).max().unwrap()
}

fn mse(a: &[i32], b: &[i32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
        .sum();
    sum / a.len() as f64
}

/// Each component differs from OpenJPEG by more than 1 and by no more than the
/// snapshot's tolerance. Moving closer to OpenJPEG does not fail this; a
/// change that brings the two to within 1 means the `2/K` difference this
/// fixture documents is gone, and the docs need updating.
#[test]
fn differs_from_openjpeg_by_more_than_one_within_the_tolerance() {
    let (ours, expected) = fixture();
    let Tolerance::Absolute { max_abs_error } = expected.tolerance else {
        panic!("the 16-bit 9/7 snapshot must carry an absolute tolerance");
    };
    for (c, samples) in ours.iter().enumerate() {
        let gap = peak(samples, &expected.components[c].samples);
        assert!(
            1 < gap && f64::from(gap) <= max_abs_error,
            "component {c}: largest difference from opj_decompress is {gap}, \
             expected more than 1 and at most {max_abs_error}"
        );
    }
}

/// The difference is OpenJPEG's error, not this decoder's: against the image the
/// encoder was given, this decode is within 1 everywhere and has the lower
/// mean-squared error, by a wide margin.
#[test]
fn is_closer_to_the_encoded_image_than_openjpeg() {
    let (ours, expected) = fixture();
    for (c, samples) in ours.iter().enumerate() {
        let source = original(c, expected.image.width, expected.image.height);
        let opj = &expected.components[c].samples;
        let gap = peak(samples, &source);
        assert!(
            gap <= 1,
            "component {c}: largest difference from the encoder's input: {gap}"
        );
        let ours_mse = mse(samples, &source);
        let opj_mse = mse(opj, &source);
        assert!(
            ours_mse * 4.0 < opj_mse,
            "component {c}: MSE against the encoder's input: ours {ours_mse}, \
             opj_decompress {opj_mse}"
        );
    }
}
