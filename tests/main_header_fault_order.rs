//! A damaged main-header segment before a structural fault in the main
//! header is reported as on any prefix that holds the segment.
//!
//! The structural faults are a length below 2, a byte that is not a
//! marker, `SOD` before any `SOT`, and the cap on main-header segments.
//! Each follows a `SIZ` that is refused. The `SIZ` fault is reported on
//! the whole file and on every prefix that holds the `SIZ`, by
//! `read_header_with`, `decode_with` and `IncrementalDecoder::refine`,
//! with `strict` on and off.
//!
//! OpenJPEG 2.5.0 reads the main header in order and fails on the first
//! segment it refuses: a `SIZ` with `XRsiz = 0` before any of these
//! faults gives "Invalid values for comp = 1" in `opj_dump` and
//! `opj_decompress`. It does not refuse a component with no sample in the
//! image while it reads `SIZ`, so on those files it reports the structural
//! fault (see "A tile with no sample of a component" in
//! `docs/openjpeg-departures.md`).

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Refined, decode_with, read_header_with};
use synth::{cod_body, qcd_none_body, seg, siz_body, tile_part};

/// The image `[1, 2) x [0, 1)` of two components, where component 1,
/// sub-sampled 2 across, holds no sample (ISO B.2).
fn siz_no_sample() -> Vec<u8> {
    siz_body([2, 1, 1, 0, 1, 1, 1, 0], &[[7, 1, 1], [7, 2, 1]])
}

/// The same image with component 1's `XRsiz` 0, which A.5.1 does not
/// allow.
fn siz_dx_zero() -> Vec<u8> {
    siz_body([2, 1, 1, 0, 1, 1, 1, 0], &[[7, 1, 1], [7, 0, 1]])
}

/// The same image with both components whole.
fn siz_valid() -> Vec<u8> {
    siz_body([2, 1, 1, 0, 1, 1, 1, 0], &[[7, 1, 1], [7, 1, 1]])
}

/// The structural faults, by name.
fn faults() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("a length below 2", vec![0xFF, 0x64, 0x00, 0x01]),
        ("a byte that is not a marker", vec![0x12, 0x34]),
        ("`SOD` before `SOT`", vec![0xFF, 0x93]),
    ]
}

/// `SOC`, a `SIZ` of body `siz`, `COD` and `QCD` if `coding`, the bytes
/// `fault`, then a tile-part and `EOC`. Also the end of the `SIZ`.
fn codestream(siz: &[u8], coding: bool, fault: &[u8]) -> (Vec<u8>, usize) {
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(seg(0xFF51, siz));
    let siz_end = bytes.len();
    if coding {
        bytes.extend(seg(0xFF52, &cod_body(1, 0, 4, 0)));
        bytes.extend(seg(0xFF5C, &qcd_none_body(2, &[8, 8])));
    }
    bytes.extend(fault);
    bytes.extend(tile_part(0, 0, 1, &[], &[0]));
    bytes.extend([0xFF, 0xD9]);
    (bytes, siz_end)
}

/// `result`'s error as its variant and message: [`Error`] has no
/// `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> String {
    match result {
        Ok(_) => "Ok".to_owned(),
        Err(error) => format!("{error:?}"),
    }
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// The errors of `prefix` from `read_header_with` and `decode_with`, with
/// `strict` on and off, each with a label.
fn errors(prefix: &[u8]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (strict, header, decode) in [
        (true, "read_header_with, strict", "decode_with, strict"),
        (false, "read_header_with, lenient", "decode_with, lenient"),
    ] {
        out.push((header, keyed(read_header_with(prefix, &options(strict)))));
        out.push((decode, keyed(decode_with(prefix, &options(strict)))));
    }
    out
}

const NO_SAMPLE: &str = "Marker(\"component 1 holds no sample in the image\")";
const DX_ZERO: &str = "Marker(\"component 1 declares a zero sub-sampling factor (0, 1)\")";

/// Every prefix that holds the refused `SIZ`, and the whole file, gives
/// the `SIZ`'s error, from every path and with `strict` on and off. An
/// `IncrementalDecoder` fed a byte at a time refines to that error from the
/// byte that completes the `SIZ` on.
#[test]
fn a_refused_siz_beats_a_later_structural_fault_on_every_prefix() {
    for (siz_name, siz, want) in [
        ("no sample", siz_no_sample(), NO_SAMPLE),
        ("XRsiz 0", siz_dx_zero(), DX_ZERO),
    ] {
        for (fault_name, fault) in faults() {
            for coding in [false, true] {
                let name = format!("{siz_name}, {fault_name}, coding {coding}");
                let (bytes, siz_end) = codestream(&siz, coding, &fault);
                for length in siz_end..=bytes.len() {
                    for (path, error) in errors(&bytes[..length]) {
                        assert_eq!(error, want, "{name}, {length} bytes, {path}");
                    }
                }
                for strict in [true, false] {
                    // The first refine of a decoder decodes.
                    for length in siz_end..=bytes.len() {
                        let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
                        decoder.feed(&bytes[..length]);
                        let refined = keyed(decoder.refine());
                        assert_eq!(refined, want, "{name}, {length} bytes, strict {strict}");
                    }
                    // Fed a byte at a time, a refine waits or gives the
                    // error, which is final.
                    let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
                    let mut refused = false;
                    for length in 1..=bytes.len() {
                        decoder.feed(&bytes[length - 1..length]);
                        match decoder.refine() {
                            Ok(Refined::Waiting(_)) if !refused => {}
                            refined => {
                                refused = true;
                                assert_eq!(
                                    keyed(refined),
                                    want,
                                    "{name}, {length} bytes fed, strict {strict}"
                                );
                            }
                        }
                    }
                    assert!(refused, "{name}, strict {strict}: no refine refused");
                    assert_eq!(keyed(decoder.finish()), want, "{name}, finish");
                }
            }
        }
    }
}

/// With a valid `SIZ` the structural fault is the error of the whole
/// file, as in OpenJPEG for the length and the non-marker. (OpenJPEG
/// skips a `SOD` in the main header as an unknown marker.)
#[test]
fn with_a_valid_siz_the_structural_fault_is_reported() {
    for (fault_name, fault, want) in [
        (
            "a length below 2",
            vec![0xFF, 0x64, 0x00, 0x01],
            "Marker(\"marker segment length below 2\")",
        ),
        (
            "a byte that is not a marker",
            vec![0x12, 0x34],
            "Codestream(\"expected a marker in the main header, found 0x1234\")",
        ),
        (
            "`SOD` before `SOT`",
            vec![0xFF, 0x93],
            "Codestream(\"unexpected marker 0xFF93 before any tile-part\")",
        ),
    ] {
        for coding in [false, true] {
            let (bytes, _) = codestream(&siz_valid(), coding, &fault);
            for (path, error) in errors(&bytes) {
                assert_eq!(error, want, "{fault_name}, coding {coding}, {path}");
            }
        }
    }
}

/// A `SIZ` that is refused, then more segment-less markers than the main
/// header may hold: the `SIZ`'s error on prefixes before, at and past the
/// cap, and on the whole file. With a valid `SIZ` the cap is the error.
#[test]
fn a_refused_siz_beats_the_segment_cap() {
    let cap = 1usize << 16;
    let reserved = [0xFF, 0x30].repeat(cap + 1);
    let (bytes, siz_end) = codestream(&siz_no_sample(), false, &reserved);
    // The `SIZ` is segment 1 of the cap, so the cap is met at the marker
    // that starts at `siz_end + 2 * (cap - 1)`.
    let at_cap = siz_end + 2 * (cap - 1);
    for length in [siz_end, siz_end + 2, at_cap, at_cap + 2, bytes.len()] {
        for (path, error) in errors(&bytes[..length]) {
            assert_eq!(error, NO_SAMPLE, "{length} bytes, {path}");
        }
        let mut decoder = IncrementalDecoder::new(options(false)).unwrap();
        decoder.feed(&bytes[..length]);
        assert_eq!(keyed(decoder.refine()), NO_SAMPLE, "{length} bytes, refine");
    }

    let (bytes, _) = codestream(&siz_valid(), false, &reserved);
    let want = format!("Codestream(\"main header has more than {cap} marker segments\")");
    for (path, error) in errors(&bytes) {
        assert_eq!(error, want, "valid SIZ, {path}");
    }
}
