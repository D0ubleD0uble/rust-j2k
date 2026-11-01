#![no_main]
//! Fuzz target over the public [`rust_j2k::read_header`] entry point.
//!
//! Same contract as `decode`: a typed result for every input, never a panic,
//! an allocation sized from an unchecked field, or a hang. It also checks that
//! `read_header` and `decode` agree, since `decode` runs the same header checks
//! before it reads any tile data:
//!
//! - when `read_header` fails, `decode` fails with the same error;
//! - when `decode` succeeds, `read_header` reports the decoded image's area,
//!   each channel's format, and the same JP2 metadata.
//!
//! `decode` may still fail where `read_header` succeeds: a fault in the tile
//! data is only found by decoding it.
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
    let header = rust_j2k::read_header(data);
    if let Ok(header) = &header {
        let samples = (u64::from(header.width) * u64::from(header.height))
            .saturating_mul(header.components.len() as u64);
        if data.len() > MAX_DECODED_INPUT || samples > MAX_DECODED_SAMPLES {
            return;
        }
    }
    let decoded = rust_j2k::decode(data);
    match (&header, &decoded) {
        (Err(h), _) => assert_eq!(decoded.as_ref().err(), Some(h), "the same error"),
        (Ok(header), Ok(image)) => {
            assert_eq!((header.width, header.height), (image.width, image.height));
            assert_eq!(header.components.len(), image.components.len());
            for (info, component) in header.components.iter().zip(&image.components) {
                assert_eq!(
                    (info.bit_depth, info.signed, info.x_sampling, info.y_sampling),
                    (
                        component.bit_depth,
                        component.signed,
                        component.x_sampling,
                        component.y_sampling
                    )
                );
            }
            assert_eq!(header.jp2, image.jp2);
        }
        (Ok(_), Err(_)) => {}
    }
});
