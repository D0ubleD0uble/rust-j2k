//! A 16-bit 9/7 codestream: how far this decoder is from OpenJPEG, and why.
//!
//! The inverse 9/7 here scales high-pass samples by the exact `2/K`. OpenJPEG
//! 2.5.0 uses `two_invK = 1.625732422`, 3.3e-5 short of it. The error that
//! constant adds grows with the sample range, so at 16 bits the two decoders
//! differ by more than 1 (see "When the oracle and the standard disagree" and
//! the bound per bit depth in `docs/correctness.md`).
//!
//! `irreversible_16bit.j2k` is a full-swing 16-bit image coded by
//! `opj_compress -I` with every bit-plane and five decomposition levels
//! (`scripts/gen-irreversible-16bit-fixture.py`). The corpus test
//! (`tests/conformance.rs`) already grades it within the snapshot's tolerance.
//! The tests here pin the rest: the difference reaches that tolerance, so the
//! number the docs state stays measured, and this decoder's output is closer to
//! the image the encoder was given than OpenJPEG's is.

mod support;

use std::path::PathBuf;

use support::{Expected, Tolerance};

const DEPTH: u32 = 16;

fn fixture() -> (Vec<i32>, Expected) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let bytes = std::fs::read(dir.join("irreversible_16bit.j2k")).unwrap();
    let text = std::fs::read_to_string(dir.join("irreversible_16bit.expected.json")).unwrap();
    let expected = Expected::from_json(&text).unwrap();
    let image = rust_j2k::decode(&bytes).expect("the 16-bit 9/7 fixture decodes");
    assert_eq!(image.components.len(), 1);
    let component = &image.components[0];
    assert_eq!((component.bit_depth, component.signed), (DEPTH as u8, false));
    (component.samples.clone(), expected)
}

/// The image the encoder was given: full-swing blocks, 3 wide and 5 tall. The
/// generator script computes the same values.
fn original(width: u32, height: u32) -> Vec<i32> {
    let top = (1i32 << DEPTH) - 1;
    (0..height)
        .flat_map(|y| (0..width).map(move |x| if (x / 3 + y / 5) % 2 == 1 { top } else { 0 }))
        .collect()
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

/// The largest difference from OpenJPEG is exactly the snapshot's tolerance:
/// the docs quote it, so a change to the 9/7 arithmetic that moves it has to
/// update the tolerance and the docs together.
#[test]
fn differs_from_openjpeg_by_the_stated_bound() {
    let (ours, expected) = fixture();
    let Tolerance::Absolute { max_abs_error } = expected.tolerance else {
        panic!("the 16-bit 9/7 snapshot must carry an absolute tolerance");
    };
    let opj = &expected.components[0].samples;
    let peak = ours
        .iter()
        .zip(opj)
        .map(|(&a, &b)| (a - b).abs())
        .max()
        .unwrap();
    assert_eq!(
        f64::from(peak),
        max_abs_error,
        "largest difference from opj_decompress; update the snapshot tolerance and \
         docs/correctness.md together"
    );
}

/// The difference is OpenJPEG's error, not this decoder's: against the image the
/// encoder was given, this decode is within 1 everywhere and has the lower
/// mean-squared error, by a wide margin. OpenJPEG's is off by up to 5.
#[test]
fn is_closer_to_the_encoded_image_than_openjpeg() {
    let (ours, expected) = fixture();
    let source = original(expected.image.width, expected.image.height);
    let peak = ours
        .iter()
        .zip(&source)
        .map(|(&a, &b)| (a - b).abs())
        .max()
        .unwrap();
    assert!(peak <= 1, "largest difference from the encoder's input: {peak}");
    let ours_mse = mse(&ours, &source);
    let opj_mse = mse(&expected.components[0].samples, &source);
    assert!(
        ours_mse * 4.0 < opj_mse,
        "MSE against the encoder's input: ours {ours_mse}, opj_decompress {opj_mse}"
    );
}
