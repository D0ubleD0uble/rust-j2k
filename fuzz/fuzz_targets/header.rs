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

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let header = rust_j2k::read_header(data);
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
