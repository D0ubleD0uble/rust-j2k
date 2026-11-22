//! Input too short to tell a codestream from a JP2 file waits, through the
//! incremental decoder and the one-shot calls alike, and input that is
//! already neither is refused at once, with the error the whole file gives.
//!
//! A JP2 file is input that carries the signature box's type (`jP  `) in
//! bytes 4 to 8, whatever bytes 0 to 4 hold, so a damaged signature box is
//! reported as one. Until 8 bytes are in, or a byte from 4 on differs from
//! that type, input that does not open with `SOC` may still be one.

use std::path::PathBuf;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, Refined, Waiting, decode_with, read_header_with,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn lenient() -> DecodeOptions {
    DecodeOptions::default().with_strict(false)
}

/// `result` with its error as its `Debug` text, so that it compares.
fn shown<T>(result: Result<T, Error>) -> Result<(), String> {
    result.map(|_| ()).map_err(|error| format!("{error:?}"))
}

/// The JP2 signature box (I.5.1).
const SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// What each call gives on `bytes`, fed to a fresh decoder in one piece,
/// under `options`: `refine`, `header()`, `finish`, `read_header_with` and
/// `decode_with`.
struct Calls {
    refine: Result<Refined, Error>,
    header: Result<bool, Error>,
    finish: Result<(), String>,
    read_header: Result<(), String>,
    decode: Result<(), String>,
}

fn calls(bytes: &[u8], options: &DecodeOptions) -> Calls {
    let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
    decoder.feed(bytes);
    Calls {
        header: decoder.header().map(|header| header.is_some()),
        refine: decoder.refine(),
        finish: shown(decoder.finish()),
        read_header: shown(read_header_with(bytes, options)),
        decode: shown(decode_with(bytes, options)),
    }
}

/// `bytes` wait: the decoder's refine and header read wait on the header,
/// the one-shot calls are `Incomplete`, and `finish` reports a truncation.
fn assert_waits(bytes: &[u8], what: &str) {
    for options in [DecodeOptions::default(), lenient()] {
        let strict = options.strict();
        let at = format!("{what} {bytes:02X?}, strict {strict}");
        let calls = calls(bytes, &options);
        match calls.refine {
            Ok(Refined::Waiting(Waiting::HeaderIncomplete { .. })) => {}
            // A refine waits for the first byte before it tries.
            Ok(Refined::Waiting(Waiting::MoreBytes { until: 1, .. })) if bytes.is_empty() => {}
            other => panic!("{at}: refine gives {other:?}"),
        }
        assert!(
            matches!(calls.header, Ok(false)),
            "{at}: {:?}",
            calls.header
        );
        for (call, result) in [
            ("read_header_with", &calls.read_header),
            ("decode_with", &calls.decode),
        ] {
            assert!(
                matches!(result, Err(e) if e.starts_with("Incomplete(")),
                "{at}: {call} gives {result:?}"
            );
        }
        assert!(
            matches!(&calls.finish, Err(e) if e.contains("the input is truncated")),
            "{at}: finish gives {:?}",
            calls.finish
        );
    }
}

/// `bytes` are refused at once: `refine`, `header()`, `finish` and the
/// one-shot calls all give the error `decode_with` gives on `whole`, which
/// `bytes` start.
fn assert_refused(bytes: &[u8], whole: &[u8], what: &str) {
    for options in [DecodeOptions::default(), lenient()] {
        let strict = options.strict();
        let at = format!("{what} {bytes:02X?}, strict {strict}");
        let expected = shown(decode_with(whole, &lenient()));
        assert!(
            matches!(&expected, Err(e) if !e.starts_with("Incomplete(")),
            "{at}: {expected:?}"
        );
        let calls = calls(bytes, &options);
        assert_eq!(shown(calls.refine), expected, "{at}: refine");
        assert_eq!(shown(calls.header), expected, "{at}: header");
        assert_eq!(calls.finish, expected, "{at}: finish");
        assert_eq!(calls.read_header, expected, "{at}: read_header_with");
        assert_eq!(calls.decode, expected, "{at}: decode_with");
    }
}

/// A codestream and a JP2 file, cut anywhere in their first 12 bytes, wait.
#[test]
fn a_file_cut_in_its_first_12_bytes_waits() {
    for (name, file) in [
        ("codestream", fixture("regular_latlon_lossy.j2k")),
        ("JP2 file", fixture("jp2_signed16.jp2")),
    ] {
        for length in 0..=12 {
            assert_waits(&file[..length], name);
        }
    }
}

/// Input whose first bytes differ from the signature box but whose bytes 4
/// to 8 may still give its type waits, and the error a refine then gives is
/// the whole file's: here, the file's damaged signature box.
#[test]
fn a_damaged_signature_box_waits_until_its_type_is_in() {
    let mut whole = fixture("jp2_signed16.jp2");
    whole[0] = 0x41;
    let error = shown(decode_with(&whole, &lenient()));
    assert!(
        matches!(&error, Err(e) if e.starts_with("FileFormat(")
            && e.contains("the file must open with the 12-byte JP2 signature box")),
        "{error:?}"
    );
    for length in 0..8 {
        assert_waits(&whole[..length], "damaged signature");
    }
    for length in 8..=12 {
        assert_refused(&whole[..length], &whole, "damaged signature");
    }
    // Fed a byte at a time, with a refine after each, the first error a
    // refine gives is the whole file's.
    for options in [DecodeOptions::default(), lenient()] {
        let mut decoder = IncrementalDecoder::new(options).unwrap();
        let mut failed = None;
        for &byte in &whole {
            decoder.feed(&[byte]);
            if let Err(e) = decoder.refine() {
                failed = Some(format!("{e:?}"));
                break;
            }
        }
        assert_eq!(failed.map(Err), Some(error.clone()));
    }
}

/// Short input that could still open a codestream or a JP2 file waits;
/// input that already opens neither is refused at once, as the whole file
/// is.
#[test]
fn junk_is_refused_once_it_can_no_longer_be_either() {
    let waits: [&[u8]; 9] = [
        &[0x41],
        &[0x41, 0x00],
        &[0xFF, 0x00],
        &[0xFF, 0x4E],
        &[0x00, 0x00, 0x00, 0x0D],
        &[0x00, 0x00, 0x00, 0x0D, b'j', b'P'],
        &[0xFF, 0x4E, 0x00, 0x00, b'j', b'P', b' '],
        &SIGNATURE[..3],
        &SIGNATURE[..7],
    ];
    for bytes in waits {
        assert_waits(bytes, "short");
    }
    let refused: [&[u8]; 5] = [
        &[0x41, 0x00, 0x00, 0x0C, b'j', 0x00],
        &[0xFF, 0x00, 0x00, 0x00, 0x00],
        &[0x00, 0x00, 0x00, 0x0D, b'j', b'P', b' ', b'!'],
        // A damaged signature box: its type is in, its length wrong.
        &[0x00, 0x00, 0x00, 0x0D, b'j', b'P', b' ', b' '],
        b"hello",
    ];
    for bytes in refused {
        // The error holds on any longer input.
        let longer = [bytes, &[0u8; 64][..]].concat();
        assert_refused(bytes, &longer, "junk");
    }
}
