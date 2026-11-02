//! Signed 32-bit components decode, although OpenJPEG 2.5.0 refuses them
//! (`j2k.c`: a precision over 31 is an error). Owner-approved deviation; see
//! `docs/correctness.md`. No oracle decodes such a file, so the expected
//! samples are derived by hand instead.
//!
//! The source is the signed 8-bit plane inside `jp2_palette_clamp.jp2`: a
//! lossless 5/3 codestream whose samples span -128..=127 and are already
//! checked against `opj_decompress` by `tests/jp2_synthetic.rs`. Changing only
//! `Ssiz` from "signed, 8 bits" to "signed, 32 bits" changes no coefficient:
//! the reversible path has no quantization, the code-block bit-plane counts
//! come from the `QCD` exponents (untouched), and a signed component has no DC
//! level shift. So the decoded samples must stay exactly as they were, with
//! only the declared depth changing.
//!
//! This proves that depth 32 is accepted and that the sign handling, level
//! shift and clamp are right at that depth for samples of small magnitude. It
//! does not prove the wavelet or the block decoder for coefficients near
//! 2^31: nothing here puts a large coefficient through them.

use rust_j2k::{DecodeOptions, Error, decode, decode_with, read_header};

fn codestream() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/jp2_palette_clamp.jp2");
    let file = std::fs::read(path).unwrap();
    let at = file
        .windows(4)
        .position(|w| w == b"jp2c")
        .expect("a jp2c box");
    // The box runs to the end of the file in this fixture.
    file[at + 4..].to_vec()
}

/// `Ssiz` of component 0: after SOC (2 bytes), the SIZ marker and length (4),
/// `Rsiz` (2), eight 4-byte geometry fields (32) and `Csiz` (2).
const SSIZ: usize = 42;

#[test]
fn a_signed_32_bit_component_decodes_to_the_same_samples() {
    let original = codestream();
    assert_eq!(original[SSIZ], 0x87, "signed, 8 bits");
    let eight = decode(&original).expect("the 8-bit codestream decodes");
    let samples = &eight.components[0].samples;
    assert_eq!(samples.iter().min(), Some(&-128));
    assert_eq!(samples.iter().max(), Some(&127));

    let mut wide = original.clone();
    wide[SSIZ] = 0x9F; // signed, 32 bits
    let image = decode(&wide).expect("signed 32-bit decodes");
    let component = &image.components[0];
    assert_eq!((component.bit_depth, component.signed), (32, true));
    assert_eq!(&component.samples, samples);
    // The header says the same.
    let header = read_header(&wide).unwrap();
    assert_eq!(
        (header.components[0].bit_depth, header.components[0].signed),
        (32, true)
    );
}

/// The container still refuses what it cannot hold: unsigned 32 bits and
/// anything above 32.
#[test]
fn unsigned_32_bit_and_deeper_stay_refused() {
    let original = codestream();
    for ssiz in [0x1F, 0x20, 0x25, 0xA0] {
        let mut bytes = original.clone();
        bytes[SSIZ] = ssiz;
        for result in [
            decode(&bytes).map(|_| ()),
            decode_with(&bytes, &DecodeOptions::default()).map(|_| ()),
            read_header(&bytes).map(|_| ()),
        ] {
            assert!(
                matches!(&result, Err(Error::Unsupported(m)) if m.contains("i32")),
                "Ssiz {ssiz:#04x}: {result:?}"
            );
        }
    }
}
