//! With `strict` off, input that ends inside a `SOT` marker segment ends the
//! codestream there, whatever `Lsot` says, so a `SOT` longer than 12 bytes
//! is judged only once the input holds all of it.
//!
//! A `SOT` with `Lsot` 11 and one trailing byte is refused once it is all
//! in, as a `Marker` error. A prefix that ended inside it, past its first 12
//! bytes, used to give `Codestream("truncated marker segment")`, which more
//! bytes turned into the `Marker` error: a refine that stopped on it gave
//! an error the whole file does not.
//!
//! The files are built here: a 4x4 image, one tile in two tile-parts, the
//! long `SOT` first or second. OpenJPEG 2.5 (`opj_decompress`) refuses both
//! with "Error reading SOT marker".

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, ReportEntry, decode_with, read_header_with,
};

/// `result`'s error as its `Debug` text, or `None`: [`Error`] has no
/// `PartialEq`.
fn error_of<T>(result: &Result<T, Error>) -> Option<String> {
    result.as_ref().err().map(|error| format!("{error:?}"))
}

/// `SOC`, then `SIZ` (4x4, one tile, one 8-bit component), `COD` (no
/// levels, one layer) and `QCD`: 65 bytes, so the first `SOT` is at 65.
const MAIN_HEADER: &str = "ff4fff510029000000000004000000040000000000000000000000040000000400\
000000000000000001070101ff52000c00000001000000000001ff5c00044040";

fn main_header() -> Vec<u8> {
    (0..MAIN_HEADER.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&MAIN_HEADER[at..at + 2], 16).unwrap())
        .collect()
}

/// A tile-part of tile 0, part `tpsot` of 2, whose `SOT` has `extra` bytes
/// after its fields (`Lsot` 10 plus their count) and `Psot` `psot`, then
/// `SOD` and `data`.
fn part(tpsot: u8, psot: u32, extra: &[u8], data: &[u8]) -> Vec<u8> {
    let mut body = 0u16.to_be_bytes().to_vec();
    body.extend(psot.to_be_bytes());
    body.extend([tpsot, 2]);
    body.extend(extra);
    let mut out = synth::seg(0xFF90, &body);
    out.extend([0xFF, 0x93]);
    out.extend(data);
    out
}

/// The issue's file: the first tile-part's `SOT` has `Lsot` 11 and one
/// trailing byte, which `Psot` covers. Its `SOT` is at 65, so a prefix of
/// 77 bytes ends inside it, past its first 12 bytes.
fn long_first() -> Vec<u8> {
    [
        main_header(),
        part(0, 16, &[0xAB], &[0]),
        part(1, 14, &[], &[]),
        vec![0xFF, 0xD9],
    ]
    .concat()
}

/// The same with the long `SOT` second, at 80: a prefix of 92 bytes ends
/// inside it.
fn long_later() -> Vec<u8> {
    [
        main_header(),
        part(0, 15, &[], &[0]),
        part(1, 15, &[0xAB], &[]),
        vec![0xFF, 0xD9],
    ]
    .concat()
}

/// A `SOT` with `Lsot` 20 that runs past its tile-part: its `Psot` of 16
/// ends at an `FF D9` inside the segment's trailing bytes.
fn past_psot() -> Vec<u8> {
    let extra = [0xAB, 0xAB, 0xAB, 0xAB, 0xFF, 0xD9, 0xAB, 0xAB, 0xAB, 0xAB];
    let mut bytes = [main_header(), part(0, 16, &extra, &[0])].concat();
    bytes[65 + 11] = 1; // TNsot 1
    bytes.extend([0xFF, 0xD9]);
    bytes
}

fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let ihdr = synth::ihdr_for(codestream);
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    synth::jp2_file(&[ihdr, colr], &[], codestream, &[])
}

const TRAILING: &str = "Marker(\"`SOT` segment has 1 unexpected trailing byte(s)\")";

/// The options the per-prefix checks run under: the default, and a
/// reduction of 1, which the tile's `COD` of no levels refuses, so the
/// header read walks the tile-parts too.
fn option_sets() -> [DecodeOptions; 2] {
    [
        DecodeOptions::default(),
        DecodeOptions::default().with_resolution_reduction(1),
    ]
}

/// On every prefix, with `strict` off, `decode_with` and `read_header_with`
/// give an image or a header, the main header cut short, or the error they
/// give on the whole file; the two tile-part walks agree; and decoders fed
/// a byte at a time, or in two pieces, return from `refine` and `header` no
/// error but the one-shot call's on the whole file, and from `finish` what
/// `decode_with` gives under the options.
fn check_prefixes(name: &str, bytes: &[u8]) {
    rust_j2k::fuzz::tile_parts_walks_agree(bytes);
    for options in option_sets() {
        let lenient = options.clone().with_strict(false);
        let whole = error_of(&decode_with(bytes, &lenient));
        let whole_header = error_of(&read_header_with(bytes, &lenient));
        let allowed = |error: &Option<String>, whole: &Option<String>| {
            error.is_none()
                || error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("Incomplete"))
                || error == whole
        };
        for len in 0..bytes.len() {
            let prefix = &bytes[..len];
            let context = format!("{name}, {len} bytes, {options:?}");
            rust_j2k::fuzz::tile_parts_walks_agree(prefix);
            let decoded = error_of(&decode_with(prefix, &lenient));
            assert!(allowed(&decoded, &whole), "{context}: decode {decoded:?}");
            let header = error_of(&read_header_with(prefix, &lenient));
            assert!(
                allowed(&header, &whole_header),
                "{context}: header {header:?}"
            );
        }
        let finished = error_of(&decode_with(bytes, &options));
        let check = |decoder: &mut IncrementalDecoder, context: &str| {
            let header = error_of(&decoder.header());
            assert!(
                header.is_none() || header == whole_header,
                "{context}: header {header:?}"
            );
            let refined = error_of(&decoder.refine());
            assert!(
                refined.is_none() || refined == whole,
                "{context}: refine {refined:?}"
            );
        };
        let context = format!("{name}, a byte at a time, {options:?}");
        let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
        for byte in bytes {
            decoder.feed(std::slice::from_ref(byte));
            check(&mut decoder, &context);
        }
        assert_eq!(error_of(&decoder.finish()), finished, "{context}: finish");
        for split in 1..bytes.len() {
            let context = format!("{name}, split at {split}, {options:?}");
            let mut decoder = IncrementalDecoder::new(options.clone()).expect("valid options");
            decoder.feed(&bytes[..split]);
            check(&mut decoder, &context);
            decoder.feed(&bytes[split..]);
            check(&mut decoder, &context);
            assert_eq!(error_of(&decoder.finish()), finished, "{context}: finish");
        }
    }
}

#[test]
fn a_refine_error_is_the_whole_files_with_a_long_first_sot() {
    check_prefixes("first", &long_first());
}

#[test]
fn a_refine_error_is_the_whole_files_with_a_long_later_sot() {
    check_prefixes("later", &long_later());
}

/// In a JP2 file the decoder holds a fault of the tile-parts until
/// `finish`, so `refine` was never hit; the one-shot calls on a prefix are.
#[test]
fn a_jp2_file_with_a_long_sot_reads_as_a_raw_codestream() {
    check_prefixes("first, in JP2", &in_jp2(&long_first()));
    check_prefixes("later, in JP2", &in_jp2(&long_later()));
}

/// The whole files give the `Marker` error with `strict` on and off, raw or
/// in a JP2 file, and so does a prefix that holds the long `SOT`. A prefix
/// that ends inside it, past its first 12 bytes, reads as the input cut
/// short with `strict` off, and is refused as truncated with `strict` on,
/// as it was.
#[test]
fn a_cut_inside_a_long_sot_ends_the_codestream() {
    for (name, bytes, sot) in [("first", long_first(), 65), ("later", long_later(), 80)] {
        let jp2 = in_jp2(&bytes);
        let start = jp2.len() - bytes.len();
        for (form, file, at) in [("raw", &bytes, sot), ("JP2", &jp2, start + sot)] {
            let context = format!("{name}, {form}");
            for strict in [true, false] {
                let options = DecodeOptions::default().with_strict(strict);
                let whole = error_of(&decode_with(file, &options));
                assert_eq!(
                    whole.as_deref(),
                    Some(TRAILING),
                    "{context}, strict {strict}"
                );
                // All 13 bytes of the `SOT`: it is judged.
                let held = error_of(&decode_with(&file[..at + 13], &options));
                assert_eq!(held, whole, "{context}, strict {strict}");
                // 12 bytes of the 13-byte `SOT`.
                let cut = &file[..at + 12];
                let decoded = decode_with(cut, &options);
                if strict {
                    assert_eq!(
                        error_of(&decoded).as_deref(),
                        Some("Codestream(\"truncated marker segment\")"),
                        "{context}"
                    );
                } else {
                    let image = decoded.unwrap_or_else(|e| panic!("{context}: {e}"));
                    assert!(
                        image
                            .report
                            .entries
                            .iter()
                            .any(|entry| matches!(entry, ReportEntry::MissingEoc { .. })),
                        "{context}: {:?}",
                        image.report.entries
                    );
                }
            }
        }
    }
}

/// A `SOT` that runs past its tile-part's `Psot` breaks the tile-part
/// chain, so the decode of a JP2 file reads it from the whole `jp2c` box
/// and gives the error a raw codestream does, not the codestream cut at the
/// `FF D9` its `Psot` ends at.
#[test]
fn a_sot_past_its_psot_in_jp2_is_refused_as_in_a_raw_codestream() {
    let raw = past_psot();
    let jp2 = in_jp2(&raw);
    for strict in [true, false] {
        let options = DecodeOptions::default().with_strict(strict);
        let expected = Some("Marker(\"`SOT` segment has 10 unexpected trailing byte(s)\")");
        assert_eq!(error_of(&decode_with(&raw, &options)).as_deref(), expected);
        assert_eq!(error_of(&decode_with(&jp2, &options)).as_deref(), expected);
    }
    check_prefixes("past Psot", &raw);
    check_prefixes("past Psot, in JP2", &jp2);
}
