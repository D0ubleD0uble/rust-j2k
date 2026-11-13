//! A packet header packed in a `PPM` or `PPT` marker that reads past the end
//! of its packed bytes is refused, with `strict` on or off: the bits it needs
//! are not in the codestream. See `tests/fixtures/README.md`, "Packed header
//! overrun fixtures".

use std::path::PathBuf;

use rust_j2k::{DecodeOptions, Error, decode_with};

const MESSAGE: &str = "a packet header runs past the end of its PPM or PPT packed headers";

fn read(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/packed_overrun")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn options(strict: bool, reduction: u8) -> DecodeOptions {
    DecodeOptions::default()
        .with_strict(strict)
        .with_resolution_reduction(reduction)
}

/// `bytes` is refused as a packed header overrun, strict and not.
fn assert_refused(bytes: &[u8], what: &str) {
    for strict in [true, false] {
        match decode_with(bytes, &options(strict, 0)) {
            Err(Error::Codestream(message)) => assert_eq!(message, MESSAGE, "{what}"),
            other => panic!("{what}, strict {strict}: expected Codestream, got {other:?}"),
        }
    }
}

/// A `PPT` header one bit short. OpenJPEG reads the missing bit as 0 and
/// decodes the sample to 128; reading it as a 1 gives 176. Neither is in the
/// codestream.
#[test]
fn a_ppt_header_one_bit_short_is_refused() {
    assert_refused(&read("ppt_short.j2k"), "ppt_short");
}

/// A `PPT` header whose last byte is 0xFF, with no stuffed byte after it.
/// OpenJPEG refuses it too.
#[test]
fn a_ppt_header_ending_in_ff_without_its_stuffed_byte_is_refused() {
    assert_refused(&read("ppt_ff_nostuff.j2k"), "ppt_ff_nostuff");
}

/// A `PPM` header one bit short.
#[test]
fn a_ppm_header_one_bit_short_is_refused() {
    assert_refused(&read("ppm_short.j2k"), "ppm_short");
}

/// The same header whole, `C4 FF 00`, decodes to OpenJPEG's sample, 176,
/// strict and not: only the overrun is refused.
#[test]
fn the_same_header_whole_decodes_as_openjpeg() {
    for name in ["ppt_whole.j2k", "ppm_whole.j2k"] {
        let bytes = read(name);
        let strict =
            decode_with(&bytes, &options(true, 0)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let partial =
            decode_with(&bytes, &options(false, 0)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(strict.components[0].samples, [176], "{name}");
        assert_eq!(strict, partial, "{name}");
        assert!(partial.report.is_complete(), "{name}");
    }
}

/// A packed header whose fill bits make the parse fail is still refused as
/// an overrun with `strict` off, not taken as the data ending: past its one
/// byte, the 1-bit fill reads as an Lblock indicator that never ends.
#[test]
fn a_packed_header_that_faults_past_its_end_is_refused() {
    let mut bytes = read("ppt_short.j2k");
    let ppt = bytes.windows(2).position(|w| w == [0xFF, 0x61]).unwrap();
    // Lppt (2 bytes), Zppt, then the one header byte.
    bytes[ppt + 5] = 0xFE;
    assert_refused(&bytes, "0xFE");
}
