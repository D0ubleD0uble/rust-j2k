//! A maxshift whose background coefficients fall in the band
//! `2^(s-1)..2^s-1`, where ISO/IEC 15444-1 H.1 and OpenJPEG disagree.
//!
//! `roi_band_s7.j2k` is a lossless 8x8 encode with `SPrgn = 7` and no region:
//! every `|sample - 128|` is below `2^7`, so H.1 reads every coefficient as
//! background and the decode must equal `source.pgm`. Half the samples have
//! bit 6 set in `|sample - 128|`; `opj_decompress` 2.5.0 returns 128 for each
//! of them. See `tests/fixtures/roi_h1_band/README.md` and
//! docs/openjpeg-departures.md.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::decode;

#[test]
fn background_values_with_bit_s_minus_one_set_decode_as_the_source() {
    let dir = synth::fixtures("roi_h1_band");
    let source = std::fs::read(dir.join("source.pgm")).unwrap();
    let header = b"P5\n8 8\n255\n";
    assert_eq!(&source[..header.len()], header, "source.pgm header");
    let want: Vec<i32> = source[header.len()..]
        .iter()
        .map(|&b| i32::from(b))
        .collect();
    assert_eq!(want.len(), 64);
    let in_band = want.iter().filter(|&&v| (v - 128).abs() >= 64).count();
    assert_eq!(in_band, 32, "half the source is in the band");

    let image = decode(&std::fs::read(dir.join("roi_band_s7.j2k")).unwrap()).unwrap();
    assert_eq!((image.width, image.height), (8, 8));
    assert!(
        image.components[0].samples == want,
        "the decode differs from source.pgm"
    );
}

/// `roi_band_s31.j2k` is the same image with `SPrgn = 31`. H.1 keeps every
/// coefficient as background from `s = 31` on, so the decode is again the
/// source. `opj_decompress` 2.5.0 zeroes the block and returns 128 for all 64
/// samples.
#[test]
fn a_maxshift_of_31_keeps_every_coefficient() {
    let dir = synth::fixtures("roi_h1_band");
    let source = std::fs::read(dir.join("source.pgm")).unwrap();
    let want: Vec<i32> = source[11..].iter().map(|&b| i32::from(b)).collect();
    assert_eq!(want.len(), 64);
    assert!(want.iter().any(|&v| v != 128), "the source is not flat");

    let image = decode(&std::fs::read(dir.join("roi_band_s31.j2k")).unwrap()).unwrap();
    assert_eq!((image.width, image.height), (8, 8));
    assert!(
        image.components[0].samples == want,
        "the decode differs from source.pgm"
    );
}
