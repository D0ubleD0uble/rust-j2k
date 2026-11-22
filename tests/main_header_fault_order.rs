//! Which error a main header gives when a structural fault follows another
//! fault, on the whole file and on every prefix.
//!
//! The structural faults are a length below 2, a byte that is not a
//! marker, `SOC`, `SOD` or `EOC` before any `SOT`, and the cap on
//! main-header segments.
//!
//! - After a `SIZ` that is refused, the `SIZ`'s error is reported on the
//!   whole file and on every prefix that holds the `SIZ`.
//! - After a Part 2 or unknown marker, the structural fault is reported:
//!   the walk never reached `SOT`, so the code was likely damage. A prefix
//!   that ends before the fault is cut short (`Incomplete`). A Part 2 or
//!   unknown marker in a header that reaches `SOT` is `Unsupported`, from
//!   the prefix that holds the first `SOT` marker on.
//! - The segments after a Part 2 or unknown marker are not judged: they may
//!   use values that the marker's part defines. A `COD` with 33 levels
//!   there gives the structural fault after it, or the marker's
//!   `Unsupported` where the header reaches `SOT`, not the `COD`'s error.
//!   Without the marker the `COD`'s error is given.
//!
//! Each is checked by `read_header_with`, `decode_with`,
//! `IncrementalDecoder::header` and `IncrementalDecoder::refine`, with
//! `strict` on and off, on the codestream and on it in a JP2 file.
//!
//! OpenJPEG 2.5.0 reads the main header in order and fails on the first
//! segment it refuses: a `SIZ` with `XRsiz = 0` before any of these faults
//! gives "Invalid values for comp = 1" in `opj_dump` and `opj_decompress`.
//! It does not refuse a component with no sample in the image while it
//! reads `SIZ`, so on those files it reports the structural fault (see "A
//! tile with no sample of a component" in `docs/openjpeg-departures.md`).
//! It skips a Part 2 or unknown marker with a warning and keeps reading, so
//! it reports a refused segment after the marker, such as that `COD`
//! ("Invalid value for numresolutions : 34"), and otherwise the fault after
//! it. This decoder differs there (see "An unknown marker in the main
//! header" in `docs/openjpeg-departures.md`).

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Refined, decode_with, read_header_with};
use synth::{bx, cod_body, colr_enum, ihdr_for, jp2_file, qcd_none_body, seg, siz_body, tile_part};

/// The image `[1, 2) x [0, 1)` of two components, the second with the
/// sub-sampling `(dx, dy)`.
fn siz(dx: u8, dy: u8) -> Vec<u8> {
    siz_body([2, 1, 1, 0, 1, 1, 1, 0], &[[7, 1, 1], [7, dx, dy]])
}

/// Component 1, sub-sampled 2 across, holds no sample (ISO B.2).
fn siz_no_sample() -> Vec<u8> {
    siz(2, 1)
}

/// Component 1's `XRsiz` is 0, which A.5.1 does not allow.
fn siz_dx_zero() -> Vec<u8> {
    siz(0, 1)
}

fn siz_valid() -> Vec<u8> {
    siz(1, 1)
}

/// The structural faults, by name, each with its error after a valid
/// `SIZ`, `COD` and `QCD`.
fn faults() -> Vec<(&'static str, Vec<u8>, &'static str)> {
    let unexpected =
        |code: &str| format!("Codestream(\"unexpected marker {code} before any tile-part\")");
    let leak = |s: String| -> &'static str { s.leak() };
    vec![
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
        ("`SOC`", vec![0xFF, 0x4F], leak(unexpected("0xFF4F"))),
        ("`SOD`", vec![0xFF, 0x93], leak(unexpected("0xFF93"))),
        ("`EOC`", vec![0xFF, 0xD9], leak(unexpected("0xFFD9"))),
    ]
}

/// A marker this decoder does not read, by name, with its error where the
/// header reaches `SOT`.
fn unread_markers() -> Vec<(&'static str, Vec<u8>, &'static str)> {
    vec![
        (
            "an unknown marker",
            seg(0xFF6F, &[1, 2]),
            "Unsupported(\"unrecognized marker 0xFF6F in the main header\")",
        ),
        (
            "`MCT`",
            seg(0xFF74, &[0, 0]),
            "Unsupported(\"`MCT` (0xFF74) is a JPEG 2000 Part 2 marker, outside the Part 1 subset\")",
        ),
    ]
}

/// `SOC`, a `SIZ` of body `siz`, `COD` and `QCD` if `coding`, the bytes
/// `then`, a tile-part and `EOC`; with the offsets where the `SIZ` and
/// `then` end.
fn codestream(siz: &[u8], coding: bool, then: &[u8]) -> (Vec<u8>, usize, usize) {
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(seg(0xFF51, siz));
    let siz_end = bytes.len();
    if coding {
        bytes.extend(seg(0xFF52, &cod_body(1, 0, 4, 0)));
        bytes.extend(seg(0xFF5C, &qcd_none_body(2, &[8, 8])));
    }
    bytes.extend(then);
    let then_end = bytes.len();
    bytes.extend(tile_part(0, 0, 1, &[], &[0]));
    bytes.extend([0xFF, 0xD9]);
    (bytes, siz_end, then_end)
}

/// `codestream` in a JP2 file, with the offset it starts at.
fn in_jp2(codestream: &[u8]) -> (Vec<u8>, usize) {
    let header = [ihdr_for(codestream), bx(b"colr", &colr_enum(17))];
    let file = jp2_file(&header, &[], codestream, &[]);
    let start = file.len() - codestream.len();
    (file, start)
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

/// Check the prefixes of `bytes` from `from` bytes on: up to `settled`
/// bytes the main header is cut short, and from there on every path gives
/// `want`.
///
/// Each prefix goes to `read_header_with` and `decode_with`, and to a new
/// `IncrementalDecoder` fed it, whose `header` and first `refine` (which
/// always decodes) are checked, with `strict` on and off. Then the whole
/// file is fed to a decoder one byte at a time. A `refine` decodes only
/// when the bytes fed have doubled since it last did, so no refine before
/// `settled` refuses, the first that decodes from there on (if the file is
/// long enough for one) and every later one give `want`, and so does
/// `finish`; `header` is `Ok(None)` before `settled` and `want` from it.
fn check(name: &str, bytes: &[u8], from: usize, settled: usize, want: &str) {
    for length in from..=bytes.len() {
        let prefix = &bytes[..length];
        let at = format!("{name}, {length} of {} bytes", bytes.len());
        for strict in [true, false] {
            let header = read_header_with(prefix, &options(strict));
            let decoded = decode_with(prefix, &options(strict));
            let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
            decoder.feed(prefix);
            let fed_header = decoder.header();
            let refined = decoder.refine();
            if length < settled {
                for (path, error) in [
                    ("read_header_with", keyed(header)),
                    ("decode_with", keyed(decoded)),
                ] {
                    assert!(
                        error.starts_with("Incomplete("),
                        "{at}, {path}, strict {strict}: {error}"
                    );
                }
                assert!(
                    matches!(fed_header, Ok(None)),
                    "{at}, header: {fed_header:?}"
                );
                assert!(
                    matches!(refined, Ok(Refined::Waiting(_))),
                    "{at}, refine: {refined:?}"
                );
            } else {
                assert_eq!(
                    keyed(header),
                    want,
                    "{at}, read_header_with, strict {strict}"
                );
                assert_eq!(keyed(decoded), want, "{at}, decode_with, strict {strict}");
                assert_eq!(keyed(fed_header), want, "{at}, header, strict {strict}");
                assert_eq!(keyed(refined), want, "{at}, refine, strict {strict}");
            }
        }
    }
    for strict in [true, false] {
        let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
        let mut refused = false;
        for length in 1..=bytes.len() {
            let at = format!("{name}, {length} bytes fed one at a time, strict {strict}");
            decoder.feed(&bytes[length - 1..length]);
            let header = decoder.header();
            if length < settled {
                assert!(matches!(header, Ok(None)), "{at}, header: {header:?}");
            } else {
                assert_eq!(keyed(header), want, "{at}, header");
            }
            match decoder.refine() {
                Ok(Refined::Waiting(_)) if !refused => {}
                refined => {
                    assert!(length >= settled, "{at}: {refined:?}");
                    refused = true;
                    assert_eq!(keyed(refined), want, "{at}, refine");
                }
            }
        }
        assert_eq!(keyed(decoder.finish()), want, "{name}, finish");
    }
}

/// `check` on `codestream` from `from` and in a JP2 file from the same
/// codestream offset.
fn check_both(name: &str, codestream: &[u8], from: usize, settled: usize, want: &str) {
    check(name, codestream, from, settled, want);
    let (file, start) = in_jp2(codestream);
    check(
        &format!("{name}, in JP2"),
        &file,
        start + from,
        start + settled,
        want,
    );
}

const NO_SAMPLE: &str = "Marker(\"component 1 holds no sample in the image\")";
const DX_ZERO: &str = "Marker(\"component 1 declares a zero sub-sampling factor (0, 1)\")";

/// A `SIZ` that is refused, then each structural fault, with and without
/// `COD` and `QCD` between: the `SIZ`'s error from the prefix that holds
/// the `SIZ` on.
#[test]
fn a_refused_siz_beats_a_later_structural_fault_on_every_prefix() {
    for (siz_name, siz, want) in [
        ("no sample", siz_no_sample(), NO_SAMPLE),
        ("XRsiz 0", siz_dx_zero(), DX_ZERO),
    ] {
        for (fault_name, fault, _) in faults() {
            for coding in [false, true] {
                let name = format!("{siz_name}, {fault_name}, coding {coding}");
                let (bytes, siz_end, _) = codestream(&siz, coding, &fault);
                check_both(&name, &bytes, siz_end, siz_end, want);
            }
        }
    }
}

/// With a valid `SIZ` the structural fault is the error, once the input
/// holds it.
#[test]
fn with_a_valid_siz_the_structural_fault_is_reported() {
    for (fault_name, fault, want) in faults() {
        let (bytes, siz_end, fault_end) = codestream(&siz_valid(), true, &fault);
        check_both(fault_name, &bytes, siz_end, fault_end, want);
    }
}

/// A Part 2 or unknown marker, then a structural fault: the fault, once the
/// input holds it. Before that the header is cut short, as the marker is
/// refused only in a header that reaches `SOT`.
#[test]
fn a_structural_fault_beats_an_earlier_unread_marker() {
    for (marker_name, marker, _) in unread_markers() {
        for (fault_name, fault, want) in faults() {
            let name = format!("{marker_name}, {fault_name}");
            let (bytes, siz_end, fault_end) =
                codestream(&siz_valid(), true, &[marker.clone(), fault].concat());
            check_both(&name, &bytes, siz_end, fault_end, want);
        }
    }
}

/// A Part 2 or unknown marker in a main header that reaches `SOT` is
/// `Unsupported`, from the prefix that holds the first `SOT` marker on:
/// a file streamed in is refused once its main header is whole.
#[test]
fn an_unread_marker_is_refused_once_the_header_reaches_sot() {
    for (marker_name, marker, want) in unread_markers() {
        let (bytes, siz_end, marker_end) = codestream(&siz_valid(), true, &marker);
        check_both(marker_name, &bytes, siz_end, marker_end + 2, want);
    }
}

/// `p0_07` (ISO/IEC 15444-4) with its first `SOT` damaged to `0xFF12`: the
/// walk steps over it as an unknown marker and stops at the `SOD` after it,
/// so the file is refused as damaged, not as an unknown marker. OpenJPEG
/// 2.5.0 warns of an unknown marker and fails to read the header.
#[test]
fn a_damaged_first_sot_is_a_structural_fault() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/conformance/codestreams/p0_07.j2k"
    );
    let mut bytes = std::fs::read(path).unwrap();
    assert_eq!(bytes[98..100], [0xFF, 0x90], "p0_07's first SOT");
    bytes[99] = 0x12;
    let want = "Codestream(\"unexpected marker 0xFF93 before any tile-part\")";
    for strict in [true, false] {
        assert_eq!(keyed(read_header_with(&bytes, &options(strict))), want);
        assert_eq!(keyed(decode_with(&bytes, &options(strict))), want);
        let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
        decoder.feed(&bytes);
        assert_eq!(keyed(decoder.header()), want);
        assert_eq!(keyed(decoder.refine()), want);
    }
}

/// A `SIZ` that is refused, then more segment-less markers than the main
/// header may hold: the `SIZ`'s error on prefixes before, at and past the
/// cap, and on the whole file. With a valid `SIZ` the cap is the error.
#[test]
fn a_refused_siz_beats_the_segment_cap() {
    let cap = 1usize << 16;
    let reserved = [0xFF, 0x30].repeat(cap + 1);
    let (bytes, siz_end, _) = codestream(&siz_no_sample(), false, &reserved);
    // The `SIZ` is segment 1 of the cap, so the cap is met at the marker
    // that starts at `siz_end + 2 * (cap - 1)`.
    let at_cap = siz_end + 2 * (cap - 1);
    for length in [siz_end, siz_end + 2, at_cap, at_cap + 2, bytes.len()] {
        let prefix = &bytes[..length];
        for strict in [true, false] {
            let at = format!("{length} bytes, strict {strict}");
            assert_eq!(
                keyed(read_header_with(prefix, &options(strict))),
                NO_SAMPLE,
                "{at}"
            );
            assert_eq!(
                keyed(decode_with(prefix, &options(strict))),
                NO_SAMPLE,
                "{at}"
            );
            let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
            decoder.feed(prefix);
            assert_eq!(keyed(decoder.header()), NO_SAMPLE, "{at}, header");
            assert_eq!(keyed(decoder.refine()), NO_SAMPLE, "{at}, refine");
        }
    }

    let (bytes, _, _) = codestream(&siz_valid(), false, &reserved);
    let want = format!("Codestream(\"main header has more than {cap} marker segments\")");
    for strict in [true, false] {
        assert_eq!(keyed(read_header_with(&bytes, &options(strict))), want);
        assert_eq!(keyed(decode_with(&bytes, &options(strict))), want);
    }
}

/// A `COD` with 33 decomposition levels, which A.6.1 does not allow (the
/// most is 32), and a `QCD` after it.
fn bad_cod_and_qcd() -> Vec<u8> {
    [
        seg(0xFF52, &cod_body(1, 33, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8, 8])),
    ]
    .concat()
}

/// The offset where `then` starts in `codestream(siz, false, then)`.
fn then_start(siz: &[u8]) -> usize {
    2 + seg(0xFF51, siz).len()
}

const BAD_COD: &str = "Marker(\"`COD` sets reserved decomposition level count 33 (max 32)\")";

/// Without an unread marker the `COD` with 33 levels is the error, on the
/// whole file and on every prefix that holds it, with a structural fault or
/// `SOT` after it. This is the control for the two tests below.
#[test]
fn a_refused_cod_is_judged_when_no_marker_precedes_it() {
    let cod = bad_cod_and_qcd();
    let start = then_start(&siz_valid());
    let cod_end = start + seg(0xFF52, &cod_body(1, 33, 4, 0)).len();
    for (fault_name, fault, _) in faults() {
        let (bytes, _, _) = codestream(&siz_valid(), false, &[cod.clone(), fault].concat());
        check_both(fault_name, &bytes, start, cod_end, BAD_COD);
    }
    let (bytes, _, _) = codestream(&siz_valid(), false, &cod);
    check_both("reaching SOT", &bytes, start, cod_end, BAD_COD);
}

/// A Part 2 or unknown marker, then a `COD` that is refused, then a
/// structural fault: the fault, not the `COD`'s error. The walk stops
/// judging at the unread marker, as the segments after it may use values
/// that the marker's part defines. OpenJPEG skips the marker and refuses
/// the `COD`.
#[test]
fn segments_after_an_unread_marker_are_not_judged_before_a_structural_fault() {
    for (marker_name, marker, _) in unread_markers() {
        for (fault_name, fault, want) in faults() {
            let name = format!("{marker_name}, bad COD, {fault_name}");
            let then = [marker.clone(), bad_cod_and_qcd(), fault].concat();
            let (bytes, siz_end, fault_end) = codestream(&siz_valid(), false, &then);
            check_both(&name, &bytes, siz_end, fault_end, want);
        }
    }
}

/// The same where the header reaches `SOT`: `Unsupported` for the marker,
/// from the prefix that holds the first `SOT` marker on, not the `COD`'s
/// error.
#[test]
fn segments_after_an_unread_marker_are_not_judged_when_the_header_reaches_sot() {
    for (marker_name, marker, want) in unread_markers() {
        let name = format!("{marker_name}, bad COD");
        let then = [marker, bad_cod_and_qcd()].concat();
        let (bytes, siz_end, then_end) = codestream(&siz_valid(), false, &then);
        check_both(&name, &bytes, siz_end, then_end + 2, want);
    }
}
