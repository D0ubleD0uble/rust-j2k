//! A tile-part header that runs past the tile-part's own `Psot`.
//!
//! `Psot` counts the tile-part's header (A.4.2), so where the tile-part
//! ends inside the input its header is read no further than that end. A
//! marker or segment that runs past it is refused at once, with `strict`
//! on or off, as a `Psot` shorter than its header, whatever its fields
//! hold: a prefix goes from `Ok` to that one error, never from one error
//! to another, and a whole file with a segment length damaged past the
//! end is refused, not read as a cut. A code judged by the markers after
//! it keeps its own refusal.
//!
//! OpenJPEG 2.5.0 refuses each whole file ("Tile part length size
//! inconsistent with stream length"), and with `-allow-partial` decodes it
//! with that warning. It refuses each prefix cut inside the header ("Stream
//! too short"), and each damaged-length file, with or without
//! `-allow-partial`.

#[path = "support/synth.rs"]
mod synth;

use std::time::{Duration, Instant};

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, decode_with, read_header_with};
use synth::{cod_body, qcd_none_body, seg, sot, tile_part};

/// The main header of [`file`]: one component, two 8x8 tiles.
fn small() -> Vec<u8> {
    [
        vec![0xFF, 0x4F],
        seg(
            0xFF51,
            &synth::siz_body([16, 8, 0, 0, 8, 8, 0, 0], &[[7, 1, 1]]),
        ),
        seg(0xFF52, &cod_body(1, 0, 4, 0)),
        seg(0xFF5C, &qcd_none_body(2, &[8])),
    ]
    .concat()
}

/// Two 8x8 tiles. Tile 0 has one tile-part (or two, for `later`); then the
/// tile-part under test: tile 1's first, or tile 0's second for `later`,
/// with `Psot` 14 and a header of `before`, a 37-byte `COM`, then `extra`,
/// then `SOD`. The second value is where that tile-part starts.
fn file(later: bool, before: &[u8], extra: &[u8]) -> (Vec<u8>, usize) {
    let mut bytes = small();
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
/// `Psot` 32 and a header of a `COD` that brings it within the cap and a
/// `COM` that holds all of tile 1's tile-part, `SOT` to `SOD`, then `SOD`.
/// The `Psot` chain goes from tile 0's tile-part on to tile 1's, which is
/// over the cap. Tile 0's header runs past its `Psot`, so the file is
/// refused there, and `read_header_with` gives that refusal, not tile 1's
/// over the cap. The second value is where tile 0's tile-part starts.
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

/// The refusal of tile `tile`'s tile-part `part`, of `Psot` `psot`.
fn past(tile: u16, part: u8, psot: u32) -> String {
    format!(
        "Codestream(\"tile {tile} tile-part {part} declares `Psot` {psot}, shorter than its \
         header\")"
    )
}

/// The files, each with where its tile-part under test starts, how many
/// bytes of it the first prefix that fails holds, and the error its whole
/// file gives. Each fails once the prefix holds the tile-part's `Psot`
/// bytes, but the unrecognized marker, which fails once it holds the code.
fn cases() -> Vec<(&'static str, Vec<u8>, usize, usize, String)> {
    let (com, com_at) = file(false, &[], &[]);
    // A 1-level `COD` with a `QCD` of one step, where it needs four: the
    // field fault lies past the `Psot`, so the `Psot` is refused first.
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
    // force, past the `Psot`.
    let (cod, cod_at) = file(true, &[], &seg(0xFF52, &cod_body(2, 0, 4, 0)));
    // An unrecognized marker inside the `Psot`, judged by the markers after
    // it, which the tile-part ends before they reach `SOD`.
    let (unknown, unknown_at) = file(false, &[0xFF, 0x7F, 0, 2], &[]);
    let (over, over_at) = over_the_cap();
    let (inside, inside_at) = a_tile_inside_a_comment();
    let unrecognized = "Codestream(\"0xFF7F in a tile-part header is not followed by marker \
                        segments that reach `SOD` within the tile-part\")";
    vec![
        ("a COM", com, com_at, 14, past(1, 0, 14)),
        ("a bad QCD", qcd, qcd_at, 14, past(1, 0, 14)),
        ("a differing COD", cod, cod_at, 14, past(0, 1, 14)),
        (
            "an unrecognized marker",
            unknown,
            unknown_at,
            14,
            unrecognized.to_string(),
        ),
        (
            "a COD under a main header over the cap",
            over,
            over_at,
            14,
            past(1, 0, 14),
        ),
        (
            "a tile-part inside a COM",
            inside,
            inside_at,
            32,
            past(0, 0, 32),
        ),
    ]
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// With `strict` off, every prefix from the tile-part on decodes until one
/// fails, the first that holds the tile-part's `Psot` bytes, and every
/// longer one fails with the whole file's error. (A prefix that ends inside
/// the first `SOT` is still `Incomplete`.) The header of each prefix, from
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
            #[cfg(feature = "__fuzz")]
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
        assert_eq!(failed.as_deref(), Some(whole.as_str()), "{name}");
    }
}

/// `read_header_with` reads tile 1's header for the cap, and refuses the
/// whole file with the decode's error, strict on and off, where a prefix
/// that ends before tile 1's `Psot` does reads.
#[test]
fn the_header_of_a_tile_over_the_cap_gives_the_decode_error() {
    let (bytes, at) = over_the_cap();
    for strict in [false, true] {
        let error = read_header_with(&bytes, &options(strict)).unwrap_err();
        assert_eq!(format!("{error:?}"), past(1, 0, 14), "{strict}");
    }
    assert!(read_header_with(&bytes[..at + 13], &options(false)).is_ok());
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

/// Whole files that end in `EOC`, each with a correct `Psot` and one
/// segment length in a tile-part header damaged to run past the end of the
/// input, and a JP2 whose codestream has a `COM` that runs past its `Psot`
/// and past the end of the `jp2c` box but not of the file. Each is refused
/// by the decode with `strict` on and off, as OpenJPEG 2.5.0 refuses them
/// with or without `-allow-partial`: none is read as the input cut short.
/// (`read_header_with` reads no tile-part header where no tile is over a
/// cap.)
#[test]
fn a_length_damaged_past_the_end_is_refused() {
    let mut files: Vec<(&str, Vec<u8>, String)> = Vec::new();
    for tile in [0u16, 1] {
        let mut bytes = small();
        for t in 0..2u16 {
            let com = seg(0xFF64, &[0, 1, b'x', b'y', b'z', b'w']);
            let mut part = tile_part(t, 0, 1, &com, &[0; 4]);
            if t == tile {
                part[14..16].copy_from_slice(&0x0200u16.to_be_bytes());
            }
            bytes.extend(part);
        }
        bytes.extend([0xFF, 0xD9]);
        files.push(("a COM", bytes, past(tile, 0, 28)));
    }
    let mut bytes = small();
    let mut part = tile_part(0, 0, 1, &seg(0xFF5C, &qcd_none_body(2, &[8])), &[0; 4]);
    part[14..16].copy_from_slice(&0x0300u16.to_be_bytes());
    bytes.extend(part);
    bytes.extend([0xFF, 0xD9]);
    files.push(("a QCD", bytes, past(0, 0, 24)));
    let mut codestream = small();
    codestream.extend(tile_part(0, 0, 1, &[], &[0; 4]));
    codestream.extend(sot(1, 14, 0, 1));
    codestream.extend([0xFF, 0x64, 0x01, 0x00, 0, 1, b'a', b'b']);
    let ihdr = synth::ihdr_for(&codestream);
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    let trailing = synth::bx(b"xml ", &[b'z'; 300]);
    let jp2 = synth::jp2_file(&[ihdr, colr], &[], &codestream, &[trailing]);
    files.push(("a JP2 with a box after the codestream", jp2, past(1, 0, 14)));
    for (name, bytes, error) in &files {
        for strict in [false, true] {
            let decoded = decode_with(bytes, &options(strict)).map(|_| ());
            assert_eq!(
                format!("{decoded:?}"),
                format!("Err({error})"),
                "{name}, {strict}"
            );
        }
    }
}

/// `count` tile-parts of `Psot` 16, each a `SOT` and a `COM` whose body is
/// the next tile-part's `SOT`, then 12 bytes, `SOD` and `EOC`: each header
/// steps over the `COM`s of every later tile-part to that `SOD`.
fn chained(count: usize) -> Vec<u8> {
    let mut bytes = small();
    for _ in 0..count {
        bytes.extend(sot(0, 16, 0, 0));
        bytes.extend([0xFF, 0x64, 0x00, 0x0E]);
    }
    bytes.extend([0x20; 12]);
    bytes.extend([0xFF, 0x93, 0, 0, 0xFF, 0xD9]);
    bytes
}

/// `count` tile-parts of `Psot` 14 whose `COM`s each run to one run of
/// `segments` small `COM`s that ends in `SOD`.
fn nested(count: usize, segments: usize) -> Vec<u8> {
    let mut bytes = small();
    let start = bytes.len();
    for _ in 0..count {
        bytes.extend(sot(0, 14, 0, 0));
        bytes.extend([0xFF, 0x64]);
    }
    bytes.extend([0xFF, 0x90]);
    bytes.resize(start + 65438, 0x20);
    for _ in 0..segments {
        bytes.extend([0xFF, 0x64, 0x00, 0x05, 1, 2, 3]);
    }
    bytes.extend([0xFF, 0x93, 0, 0, 0xFF, 0xD9]);
    bytes
}

/// Files whose tile-part headers run past their `Psot` into the headers
/// of the tile-parts after them are refused at the first, without reading
/// on: a 320 KB file of 20000 such tile-parts, and one of 4600 that land in
/// 100000 small segments. Read on to the end of the input, as each header
/// was, they took seconds.
#[test]
fn a_header_past_its_psot_is_refused_without_reading_on() {
    for bytes in [chained(20_000), nested(4600, 100_000)] {
        for strict in [false, true] {
            let start = Instant::now();
            let decoded = decode_with(&bytes, &options(strict)).map(|_| ());
            let header = read_header_with(&bytes, &options(strict)).map(|_| ());
            let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
            for chunk in bytes.chunks(65536) {
                decoder.feed(chunk);
                let _ = decoder.header();
            }
            let took = start.elapsed();
            assert!(
                took < Duration::from_secs(1),
                "{} bytes: {took:?}",
                bytes.len()
            );
            let error = format!("{:?}", decoded.unwrap_err());
            assert!(error.contains("shorter than its header"), "{error}");
            assert!(header.is_ok(), "{header:?}");
        }
    }
}

/// A `Psot` that ends inside the tile-part's 12-byte `SOT` is refused as
/// shorter than its `SOT`, for a first tile-part and a later one, with
/// `strict` on and off.
#[test]
fn a_psot_inside_the_sot_is_refused_as_shorter_than_it() {
    for later in [false, true] {
        let mut bytes = small();
        bytes.extend(tile_part(0, 0, if later { 2 } else { 1 }, &[], &[0; 4]));
        let (tile, part) = if later { (0, 1) } else { (1, 0) };
        bytes.extend(sot(tile, 10, part, if later { 2 } else { 1 }));
        bytes.extend([0xFF, 0x93, 0, 0, 0xFF, 0xD9]);
        for strict in [false, true] {
            let error = decode_with(&bytes, &options(strict)).unwrap_err();
            assert_eq!(
                format!("{error:?}"),
                format!(
                    "Codestream(\"tile {tile} tile-part {part} declares `Psot` 10, shorter than \
                     its `SOT`\")"
                ),
                "{later}, {strict}"
            );
        }
    }
}

/// A tile-part of tile 1 whose header is `bad`, a segment with a faulty
/// field that lies within `Psot`, then a `COM` that runs past it.
fn bad_then_overrun(bad: &[u8]) -> Vec<u8> {
    let mut bytes = small();
    bytes.extend(tile_part(0, 0, 1, &[], &[0; 4]));
    bytes.extend(sot(1, (12 + bad.len() + 10) as u32, 0, 1));
    bytes.extend(bad);
    bytes.extend(comment());
    bytes.extend([0xFF, 0x93, 0, 0, 0, 0, 0xFF, 0xD9]);
    bytes
}

/// The header is read in order, so a faulty field in a segment before the
/// one that runs past `Psot` keeps its own `Marker` error, with `strict` on
/// and off. Only a check made once the header is read gives way to the
/// `Psot` refusal.
#[test]
fn a_bad_field_before_the_overrun_keeps_its_marker_error() {
    // A `QCC` for component 5 where `Csiz` is 1, and a `COD` with
    // progression order 9.
    let mut qcc_body = vec![5];
    qcc_body.extend(qcd_none_body(2, &[8]));
    let mut cod = cod_body(1, 0, 4, 0);
    cod[1] = 9;
    for (name, bad, message) in [
        ("QCC", seg(0xFF5D, &qcc_body), "`QCC` targets component 5"),
        ("COD", seg(0xFF52, &cod), "reserved progression order 9"),
    ] {
        let bytes = bad_then_overrun(&bad);
        for strict in [false, true] {
            let error = decode_with(&bytes, &options(strict)).unwrap_err();
            let Error::Marker(detail) = &error else {
                panic!("{name}, {strict}: {error:?}");
            };
            assert!(detail.to_string().contains(message), "{name}: {detail}");
        }
    }
}
