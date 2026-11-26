//! A `SOT` marker segment whose `Lsot` is not 10 is refused as soon as the
//! input holds its `Lsot`, with `strict` on and off. The reader of the
//! segment admits only `Lsot` 10, so the error it gives for the whole
//! segment depends on `Lsot` alone (a length below 2, a body too short for
//! the fields, or bytes left after them), and no later byte changes it.
//!
//! With `strict` off, a prefix that ended inside a longer `SOT`, past its
//! first 12 bytes, used to give `Codestream("truncated marker segment")`,
//! which more bytes turned into the `Marker` error for the trailing bytes:
//! a refine that stopped on it gave an error the whole file does not. A cut
//! before `Lsot`, or inside a `SOT` of `Lsot` 10, still ends the codestream
//! there.
//!
//! The files are built here: a 4x4 image, one tile in two tile-parts, the
//! odd `SOT` first or second. OpenJPEG 2.5 (`opj_decompress`, with and
//! without `-allow-partial`) refuses each whole file: "Error reading SOT
//! marker", or "Marker size inconsistent with stream length" where `Lsot`
//! runs past the end of the file.

#![cfg(feature = "__fuzz")]

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, Image, IncrementalDecoder, ReportEntry, decode_with, read_header_with,
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

const SOD: [u8; 2] = [0xFF, 0x93];
const EOC: [u8; 2] = [0xFF, 0xD9];

/// A `SOT` marker segment of `lsot` that states tile 0, `psot` and part
/// `tpsot` of `tnsot`, then `extra`, whatever `lsot` says.
fn sot(lsot: u16, psot: u32, tpsot: u8, tnsot: u8, extra: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, 0x90];
    out.extend(lsot.to_be_bytes());
    out.extend(0u16.to_be_bytes());
    out.extend(psot.to_be_bytes());
    out.extend([tpsot, tnsot]);
    out.extend(extra);
    out
}

/// A tile-part of tile 0, part `tpsot` of 2, whose `SOT` has `extra` bytes
/// after its fields (`Lsot` 10 plus their count) and `Psot` `psot`, then
/// `SOD` and `data`.
fn part(tpsot: u8, psot: u32, extra: &[u8], data: &[u8]) -> Vec<u8> {
    let lsot = u16::try_from(10 + extra.len()).unwrap();
    [
        sot(lsot, psot, tpsot, 2, extra),
        SOD.to_vec(),
        data.to_vec(),
    ]
    .concat()
}

/// The issue's file: the first tile-part's `SOT` has `Lsot` 11 and one
/// trailing byte, which `Psot` covers. Its `SOT` is at 65, so a prefix of
/// 77 bytes ends inside it, past its first 12 bytes.
fn long_first() -> Vec<u8> {
    [
        main_header(),
        part(0, 16, &[0xAB], &[0]),
        part(1, 14, &[], &[]),
        EOC.to_vec(),
    ]
    .concat()
}

/// The same with the long `SOT` second, at 80.
fn long_later() -> Vec<u8> {
    [
        main_header(),
        part(0, 15, &[], &[0]),
        part(1, 15, &[0xAB], &[]),
        EOC.to_vec(),
    ]
    .concat()
}

/// `Lsot` 65535 in the first tile-part, past the end of the file, which
/// ends in an `EOC` the `Psot` chain reaches.
fn first_l65535() -> Vec<u8> {
    [
        main_header(),
        sot(65535, 16, 0, 2, &[0xAB]),
        SOD.to_vec(),
        vec![0],
        part(1, 14, &[], &[]),
        EOC.to_vec(),
    ]
    .concat()
}

/// `Lsot` 65535 in the second tile-part.
fn later_l65535() -> Vec<u8> {
    [
        main_header(),
        part(0, 15, &[], &[0]),
        sot(65535, 15, 1, 2, &[0xAB]),
        SOD.to_vec(),
        EOC.to_vec(),
    ]
    .concat()
}

/// A `SOT` with `Lsot` 20 that runs past its tile-part: its `Psot` of 16
/// ends at an `FF D9` inside the segment's trailing bytes.
fn past_psot() -> Vec<u8> {
    let extra = [0xAB, 0xAB, 0xAB, 0xAB, 0xFF, 0xD9, 0xAB, 0xAB, 0xAB, 0xAB];
    [
        main_header(),
        sot(20, 16, 0, 1, &extra),
        SOD.to_vec(),
        vec![0],
        EOC.to_vec(),
    ]
    .concat()
}

/// A last tile-part with `Psot` 0 and `Lsot` 11.
fn psot_zero() -> Vec<u8> {
    [
        main_header(),
        part(0, 15, &[], &[0]),
        sot(11, 0, 1, 2, &[0xAB]),
        SOD.to_vec(),
        vec![0],
        EOC.to_vec(),
    ]
    .concat()
}

/// A first `SOT` of `Lsot` `lsot`, under 10, with the fields of `Lsot` 10
/// after it.
fn short(lsot: u16) -> Vec<u8> {
    [
        main_header(),
        sot(lsot, 15, 0, 2, &[]),
        SOD.to_vec(),
        vec![0],
        part(1, 14, &[], &[]),
        EOC.to_vec(),
    ]
    .concat()
}

/// Each file, the offset of its odd `SOT`, and the error it gives.
fn files() -> Vec<(&'static str, Vec<u8>, usize, String)> {
    let trailing =
        |n: u32| format!("Marker(\"`SOT` segment has {n} unexpected trailing byte(s)\")");
    vec![
        ("first", long_first(), 65, trailing(1)),
        ("later", long_later(), 80, trailing(1)),
        ("first, Lsot 65535", first_l65535(), 65, trailing(65525)),
        ("later, Lsot 65535", later_l65535(), 80, trailing(65525)),
        ("past Psot", past_psot(), 65, trailing(10)),
        ("Psot 0", psot_zero(), 80, trailing(1)),
        (
            "Lsot 0",
            short(0),
            65,
            "Marker(\"marker segment length below 2\")".into(),
        ),
        (
            "Lsot 9",
            short(9),
            65,
            "Marker(\"truncated marker segment\")".into(),
        ),
    ]
}

fn in_jp2(codestream: &[u8]) -> Vec<u8> {
    let ihdr = synth::ihdr_for(codestream);
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    synth::jp2_file(&[ihdr, colr], &[], codestream, &[])
}

fn missing_eoc(image: &Image) -> bool {
    image
        .report
        .entries
        .iter()
        .any(|entry| matches!(entry, ReportEntry::MissingEoc { .. }))
}

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
fn a_refine_error_is_the_whole_files() {
    for (name, bytes, _, _) in files() {
        check_prefixes(name, &bytes);
    }
}

/// In a JP2 file the decoder holds a fault of the tile-parts until
/// `finish`; the one-shot calls on a prefix give the raw codestream's.
#[test]
fn a_jp2_file_reads_as_a_raw_codestream() {
    for (name, bytes, _, _) in files() {
        check_prefixes(&format!("{name}, in JP2"), &in_jp2(&bytes));
    }
}

/// Every prefix that holds the odd `SOT`'s `Lsot` gives the file's error,
/// with `strict` on and off, raw and in a JP2 file, and so does the raw
/// file with bytes after it. With `strict` off a prefix that ends before
/// `Lsot` reads as the input cut short; with `strict` on it is refused as
/// truncated.
#[test]
fn an_odd_lsot_is_refused_once_the_input_holds_it() {
    for (name, bytes, sot, expected) in files() {
        let jp2 = in_jp2(&bytes);
        let start = jp2.len() - bytes.len();
        for (form, file, at) in [("raw", &bytes, sot), ("JP2", &jp2, start + sot)] {
            for strict in [true, false] {
                let context = format!("{name}, {form}, strict {strict}");
                let options = DecodeOptions::default().with_strict(strict);
                for len in at + 4..=file.len() {
                    let error = error_of(&decode_with(&file[..len], &options));
                    assert_eq!(error.as_ref(), Some(&expected), "{context}, {len} bytes");
                }
                if form == "raw" {
                    for appended in [&[0u8; 3][..], &[0xFF; 70000]] {
                        let longer = [file.as_slice(), appended].concat();
                        let error = error_of(&decode_with(&longer, &options));
                        assert_eq!(error.as_ref(), Some(&expected), "{context}, longer");
                    }
                }
                let cut = decode_with(&file[..at + 3], &options);
                if strict {
                    assert_eq!(
                        error_of(&cut).as_deref(),
                        Some("Codestream(\"truncated marker segment\")"),
                        "{context}"
                    );
                } else {
                    let image = cut.unwrap_or_else(|e| panic!("{context}: {e}"));
                    assert!(missing_eoc(&image), "{context}");
                }
            }
        }
    }
}

/// A whole file whose `Lsot` of 65535 runs past its end is refused, with
/// `strict` on and off, though its `Psot` chain reaches an `EOC`: it used
/// to decode with `strict` off, as the input cut short.
#[test]
fn a_sot_longer_than_the_file_is_refused() {
    for bytes in [first_l65535(), later_l65535()] {
        for strict in [true, false] {
            let options = DecodeOptions::default().with_strict(strict);
            assert_eq!(
                error_of(&decode_with(&bytes, &options)).as_deref(),
                Some("Marker(\"`SOT` segment has 65525 unexpected trailing byte(s)\")"),
                "strict {strict}"
            );
        }
    }
}

/// A cut inside a `SOT` of `Lsot` 10 still ends the codestream with
/// `strict` off, as it did.
#[test]
fn a_cut_inside_a_sot_of_lsot_10_ends_the_codestream() {
    let bytes = long_later();
    for len in 67..77 {
        let image = decode_with(&bytes[..len], &DecodeOptions::default().with_strict(false))
            .unwrap_or_else(|e| panic!("{len} bytes: {e}"));
        assert!(missing_eoc(&image), "{len} bytes");
    }
}
