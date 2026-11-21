//! A tile-part header that runs past the tile-part's own `Psot`, with
//! `strict` off, on every prefix of the file.
//!
//! The header's marker segments are read before their end is checked
//! against `Psot`, as OpenJPEG reads them. So input that ends inside one of
//! them is read as the input cut short, whatever the `Psot`, and the error
//! comes once the input holds what it depends on: a bad field in a segment,
//! a later tile-part's `COD` that differs, or, for a header with no other
//! fault, `Psot` shorter than the header. A prefix goes from `Ok` to that
//! one error and never from one error to another.
//!
//! OpenJPEG 2.5.0 refuses each whole file ("Tile part length size
//! inconsistent with stream length"), and with `-allow-partial` decodes it
//! with that warning. It refuses each prefix cut inside the header ("Stream
//! too short"), with or without `-allow-partial`. This decoder refuses each
//! whole file, with `strict` on or off.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{
    DecodeOptions, Error, IncrementalDecoder, ReportEntry, decode_with, read_header_with,
};
use synth::{cod_body, qcd_none_body, seg, sot, tile_part};

/// Two 8x8 tiles. Tile 0 has one tile-part (or two, for `later`); then the
/// tile-part under test: tile 1's first, or tile 0's second for `later`,
/// with `Psot` 14 and a header of `before`, a 37-byte `COM`, then `extra`,
/// then `SOD`. The second value is where that tile-part starts.
fn file(later: bool, before: &[u8], extra: &[u8]) -> (Vec<u8>, usize) {
    let mut bytes = [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([16, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &cod_body(1, 0, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8])),
    ]
    .concat();
    bytes.extend(tile_part(0, 0, if later { 2 } else { 1 }, &[], &[0; 4]));
    let at = bytes.len();
    bytes.extend(if later {
        sot(0, 14, 1, 2)
    } else {
        sot(1, 14, 0, 1)
    });
    bytes.extend(before);
    bytes.extend(comment());
    bytes.extend(extra);
    bytes.extend([0xFF, 0x93, 0, 0, 0, 0, 0xFF, 0xD9]);
    (bytes, at)
}

/// Two 1040x512 tiles under a main `COD` of 1x1 precincts, which takes
/// each over the precinct cap, so `read_header_with` reads each tile's own
/// header. Each tile-part has a `COD` of the default precincts, which
/// brings its tile within the cap. Tile 1's has `Psot` 14 and a header of
/// a 37-byte `COM` and that `COD`. The second value is where it starts.
fn over_the_cap() -> (Vec<u8>, usize) {
    let mut bytes = main_over_the_cap();
    let own = seg(0xFF52, &cod_body(1, 1, 4, 0));
    bytes.extend(tile_part(0, 0, 1, &own, &[0; 4]));
    let at = bytes.len();
    bytes.extend(sot(1, 14, 0, 1));
    bytes.extend(comment());
    bytes.extend(own);
    bytes.extend([0xFF, 0x93, 0, 0, 0, 0, 0xFF, 0xD9]);
    (bytes, at)
}

/// The main header of [`over_the_cap`].
fn main_over_the_cap() -> Vec<u8> {
    let mut main_cod = cod_body(1, 1, 4, 0);
    main_cod[0] = 1;
    main_cod.extend([0x00, 0x11]);
    [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([2080, 512, 0, 0, 1040, 512, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &main_cod),
        seg(0xFF5C, &qcd_none_body(2, &[8; 4])),
    ]
    .concat()
}

/// The main header of [`over_the_cap`], then tile 0's tile-part with
/// `Psot` 18 and a header of a `COD` that brings it within the cap and a
/// `COM` that holds all of tile 1's tile-part, `SOT` to `SOD`, then `SOD`.
/// The `Psot` chain goes from tile 0's tile-part on to tile 1's, which is
/// over the cap. A prefix that ends inside the `COM` is cut in tile 0's
/// header, so tile 1 is missing; `read_header_with` does not count it. The
/// second value is where tile 0's tile-part starts.
fn a_tile_inside_a_comment() -> (Vec<u8>, usize) {
    let mut bytes = main_over_the_cap();
    let at = bytes.len();
    let own = seg(0xFF52, &cod_body(1, 1, 4, 0));
    let inner = tile_part(1, 0, 1, &[], &[]);
    let mut body = vec![0, 1];
    body.extend(&inner);
    let psot = 12 + own.len() + 6;
    bytes.extend(sot(0, psot as u32, 0, 1));
    bytes.extend(&own);
    bytes.extend(seg(0xFF64, &body));
    assert_eq!(bytes[at + psot..at + psot + 2], [0xFF, 0x90]);
    bytes.extend([0xFF, 0x93, 0xFF, 0xD9]);
    (bytes, at)
}

/// A `COM` of 37 bytes.
fn comment() -> Vec<u8> {
    let mut body = vec![0, 1];
    body.extend((0..31).map(|i| b'a' + i));
    seg(0xFF64, &body)
}

/// The files, each with how many bytes of its tile-part the first prefix
/// that fails holds, and the error its whole file gives.
fn cases() -> Vec<(&'static str, Vec<u8>, usize, usize, &'static str)> {
    let (com, com_at) = file(false, &[], &[]);
    // A 1-level `COD` with a `QCD` of one step, where it needs four.
    let (qcd, qcd_at) = file(
        false,
        &[],
        &[
            seg(0xFF52, &cod_body(1, 1, 4, 0)),
            seg(0xFF5C, &qcd_none_body(2, &[8])),
        ]
        .concat(),
    );
    // Tile 0's second tile-part, with a `COD` of two layers where one is in
    // force.
    let (cod, cod_at) = file(true, &[], &seg(0xFF52, &cod_body(2, 0, 4, 0)));
    // An unrecognized marker inside the `Psot`, judged by the markers after
    // it, which the tile-part ends before they reach `SOD`: refused once the
    // input holds its code, as when the tile-part ends inside the input.
    let (unknown, unknown_at) = file(false, &[0xFF, 0x7F, 0, 2], &[]);
    let (over, over_at) = over_the_cap();
    let (inside, inside_at) = a_tile_inside_a_comment();
    vec![
        (
            "a COM",
            com,
            com_at,
            51,
            "Codestream(\"tile 1 tile-part 0 declares `Psot` 14, shorter than its 51-byte header\")",
        ),
        (
            "a bad QCD",
            qcd,
            qcd_at,
            71,
            "Marker(\"component 0 carries 1 quantization step entries but its 1 decomposition \
             levels need 4\")",
        ),
        (
            "a differing COD",
            cod,
            cod_at,
            65,
            "Codestream(\"a later tile-part of tile 0 carries a `COD` that differs from the one in \
             force; the coding-parameter overrides belong to the first tile-part of a tile\")",
        ),
        (
            "an unrecognized marker",
            unknown,
            unknown_at,
            14,
            "Codestream(\"0xFF7F in a tile-part header is not followed by marker segments that \
             reach `SOD` within the tile-part\")",
        ),
        (
            "a COD under a main header over the cap",
            over,
            over_at,
            65,
            "Codestream(\"tile 1 tile-part 0 declares `Psot` 14, shorter than its 65-byte header\")",
        ),
        (
            "a tile-part inside a COM",
            inside,
            inside_at,
            48,
            "Codestream(\"tile 0 tile-part 0 declares `Psot` 32, shorter than its 48-byte header\")",
        ),
    ]
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// With `strict` off, every prefix from the tile-part on decodes until one
/// fails, the first that holds what the error depends on, and every longer
/// one fails with the whole file's error. (A prefix that ends inside the
/// first `SOT` is still `Incomplete`.) The header of each prefix, from
/// `read_header_with` and from an `IncrementalDecoder` fed a byte at a
/// time, agree, and an error from it is the decode's. The two tile-part
/// walks agree on each prefix.
#[test]
fn a_prefix_moves_from_ok_to_one_final_error() {
    for (name, bytes, at, first, whole) in cases() {
        let mut decoder = IncrementalDecoder::new(options(false)).unwrap();
        decoder.feed(&bytes[..at - 1]);
        let mut failed: Option<String> = None;
        for length in at..=bytes.len() {
            let prefix = &bytes[..length];
            decoder.feed(&bytes[length - 1..length]);
            rust_j2k::fuzz::tile_parts_walks_agree(prefix);
            let decoded = decode_with(prefix, &options(false)).map(|_| ());
            match (decoded, &failed) {
                (Err(Error::Incomplete(_)), None) if length < at + 12 => {}
                (Ok(()), None) => {}
                (Ok(()), Some(error)) => panic!("{name}, {length}: Ok after {error}"),
                (Err(error), None) => {
                    assert_eq!(length, at + first, "{name}: {error}");
                    failed = Some(format!("{error:?}"));
                }
                (Err(error), Some(first)) => {
                    assert_eq!(&format!("{error:?}"), first, "{name}, {length}")
                }
            }
            let header = read_header_with(prefix, &options(false));
            if let (Err(error), Some(failed)) = (&header, &failed) {
                assert_eq!(&format!("{error:?}"), failed, "{name}, {length}");
            }
            let want = match header {
                Ok(header) => Ok(Some(header)),
                Err(Error::Incomplete(_)) => Ok(None),
                Err(error) => Err(error),
            };
            assert_eq!(
                format!("{:?}", decoder.header()),
                format!("{want:?}"),
                "{name}, {length}"
            );
        }
        assert_eq!(failed.as_deref(), Some(whole), "{name}");
    }
}

/// With `strict` off, input cut inside the `COM`, at or past the 14 bytes
/// `Psot` gives, decodes, and the report names the tile-part as cut in its
/// header. The unrecognized marker is refused before then.
#[test]
fn a_cut_past_psot_is_read_as_a_cut() {
    let cases = cases().into_iter();
    for (name, bytes, at, _, _) in cases.filter(|case| case.3 > 30 && case.3 != 48) {
        for present in [14, 30] {
            let image = decode_with(&bytes[..at + present], &options(false))
                .unwrap_or_else(|e| panic!("{name}, {present}: {e}"));
            let entry = image
                .report
                .entries
                .iter()
                .find(|entry| matches!(entry, ReportEntry::TruncatedTilePart { .. }))
                .unwrap_or_else(|| panic!("{name}, {present}: {:?}", image.report.entries));
            assert!(
                matches!(
                    entry,
                    ReportEntry::TruncatedTilePart { declared: 14, present: got, .. }
                        if *got == present as u64
                ),
                "{name}: {entry:?}"
            );
            assert_eq!(
                entry.to_string(),
                format!(
                    "tile {} tile-part {} is cut short in its header, which runs past its \
                     `Psot` of 14 bytes: {present} bytes of it are there",
                    if name == "a differing COD" { 0 } else { 1 },
                    if name == "a differing COD" { 1 } else { 0 },
                ),
                "{name}"
            );
        }
    }
}

/// With `strict` off, `read_header_with` reads tile 1's header for the
/// cap, and refuses the whole file with the decode's error, where an
/// earlier prefix cut inside that header reads.
#[test]
fn the_header_of_a_tile_over_the_cap_gives_the_decode_error() {
    let (bytes, at) = over_the_cap();
    let error = read_header_with(&bytes, &options(false)).unwrap_err();
    assert_eq!(
        format!("{error:?}"),
        "Codestream(\"tile 1 tile-part 0 declares `Psot` 14, shorter than its 65-byte header\")"
    );
    assert!(read_header_with(&bytes[..at + 30], &options(false)).is_ok());
}

/// With `strict` on, the whole file gives the same error as with it off,
/// and every prefix cut inside the header is refused.
#[test]
fn strict_refuses_the_file_and_each_cut() {
    for (name, bytes, at, _, whole) in cases() {
        let error = decode_with(&bytes, &options(true)).unwrap_err();
        assert_eq!(format!("{error:?}"), whole, "{name}");
        for length in at..bytes.len() {
            assert!(
                decode_with(&bytes[..length], &options(true)).is_err(),
                "{name}, {length}"
            );
        }
    }
}
