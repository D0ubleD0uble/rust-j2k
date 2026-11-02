#![no_main]
//! Fuzz target over the public [`rust_j2k::read_header_with`] entry point.
//!
//! Same contract as `decode`: a typed result for every input, never a panic,
//! an allocation sized from an unchecked field, or a hang. Every input is run
//! under a fixed list of option sets (the defaults, `raw_channels`, a
//! resolution reduction of 1, sRGB output, and reduction 1 with sRGB), so a
//! plain `.j2k` or `.jp2` seed is used as it is. The header is read under all
//! five; the full decode that follows a header that reads runs under one,
//! picked by a hash of the input, to keep the target fast. For each set it checks that
//! `read_header_with` and `decode_with` agree, since `decode_with` runs the
//! same header checks before it reads any tile data:
//!
//! - when `read_header_with` fails, `decode_with` fails with the same error;
//! - when `decode_with` succeeds, `read_header_with` reports the decoded
//!   image's area, each channel's format, and the same JP2 metadata.
//!
//! `decode_with` may still fail where `read_header_with` succeeds: a fault in
//! the tile data is only found by decoding it.
//!
//! A full decode costs far more than the header, so when the header reads and
//! the input or the image is large, the decode is skipped and only
//! `read_header` is exercised. The `decode` target covers large decodes.

use libfuzzer_sys::fuzz_target;

/// Inputs above this many bytes are not decoded once their header reads.
const MAX_DECODED_INPUT: usize = 1 << 16;
/// Images above this many samples, over all channels, are not decoded.
const MAX_DECODED_SAMPLES: u64 = 1 << 16;

fuzz_target!(|data: &[u8]| {
    let default = rust_j2k::DecodeOptions::default();
    let srgb = || default.clone().with_colour(rust_j2k::ColourOutput::Srgb);
    let option_sets = [
        default.clone(),
        default.clone().with_raw_channels(true),
        default.clone().with_resolution_reduction(1),
        srgb(),
        srgb().with_resolution_reduction(1),
    ];
    // A full decode costs far more than a header read. A header that fails
    // is compared with a decode that stops just as early, so every set gets
    // that check; a header that reads is compared with a full decode under
    // one set only, picked by a hash of the whole input so that seeds (which
    // share their first and last bytes) still spread over all five.
    let picked = fnv1a(data) as usize % option_sets.len();
    for (i, options) in option_sets.iter().enumerate() {
        check(data, options, i == picked);
    }
});

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn check(data: &[u8], options: &rust_j2k::DecodeOptions, decode_when_read: bool) {
    let header = rust_j2k::read_header_with(data, options);
    if let Ok(header) = &header {
        if !decode_when_read {
            return;
        }
        let samples = (u64::from(header.width) * u64::from(header.height))
            .saturating_mul(header.components.len() as u64);
        if data.len() > MAX_DECODED_INPUT || samples > MAX_DECODED_SAMPLES {
            return;
        }
    }
    let decoded = rust_j2k::decode_with(data, options);
    match (&header, &decoded) {
        (Err(h), _) => assert_eq!(decoded.as_ref().err(), Some(h), "the same error"),
        (Ok(header), Ok(image)) => {
            assert_eq!((header.width, header.height), (image.width, image.height));
            assert_eq!(header.components.len(), image.components.len());
            for (info, component) in header.components.iter().zip(&image.components) {
                assert_eq!(
                    (
                        info.bit_depth,
                        info.signed,
                        info.x_sampling,
                        info.y_sampling,
                        info.width,
                        info.height
                    ),
                    (
                        component.bit_depth,
                        component.signed,
                        component.x_sampling,
                        component.y_sampling,
                        component.width,
                        component.height
                    )
                );
            }
            assert_eq!(header.jp2, image.jp2);
        }
        (Ok(_), Err(_)) => {}
    }
}
