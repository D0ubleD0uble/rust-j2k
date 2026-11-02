#![no_main]
//! Fuzz target over the public [`rust_j2k::read_header_with`] entry point.
//!
//! Same contract as `decode`: a typed result for every input, never a panic,
//! an allocation sized from an unchecked field, or a hang. The last input
//! byte picks the options (`raw_channels`, sRGB output, a resolution
//! reduction of 0 to 3); the rest is the file. (The last byte, not the first, so a plain
//! `.j2k` or `.jp2` seed still opens with its own signature.) It also checks that
//! `read_header_with` and `decode_with` agree under those options, since
//! `decode_with` runs the same header checks before it reads any tile data:
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

fuzz_target!(|input: &[u8]| {
    let Some((&selector, data)) = input.split_last() else {
        return;
    };
    let mut options = rust_j2k::DecodeOptions::default()
        .with_raw_channels(selector & 1 != 0)
        .with_resolution_reduction((selector >> 2) & 3);
    if selector & 2 != 0 {
        options = options.with_colour(rust_j2k::ColourOutput::Srgb);
    }
    let header = rust_j2k::read_header_with(data, &options);
    if let Ok(header) = &header {
        let samples = (u64::from(header.width) * u64::from(header.height))
            .saturating_mul(header.components.len() as u64);
        if data.len() > MAX_DECODED_INPUT || samples > MAX_DECODED_SAMPLES {
            return;
        }
    }
    let decoded = rust_j2k::decode_with(data, &options);
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
});
