//! Two `PPM` shapes OpenJPEG 2.5.0 refuses: an `Nppm` field split across
//! `PPM` segments, and a `PPM` whose `Nppm`s are all 0. Two it reads: a
//! segment that ends right after a whole `Nppm`, and one that ends partway
//! through a packet header. Each is checked on the whole file and on every
//! prefix, by `read_header_with`, `decode_with` and `IncrementalDecoder`,
//! with `strict` on and off.
//!
//! ISO/IEC 15444-1 A.7.4 (`Zppm`) reads: "Every marker segment in this
//! series shall end with a completed packet header. However, the series of
//! Ippm parameters described by the Nppm does not have to be complete in a
//! given marker segment. Therefore, it is possible that the next PPM marker
//! segment will not have a Nppm parameter after Zppm, but the continuation
//! of the Ippm series from the last PPM marker segment." (ITU-T T.800
//! (08/2002).) A segment that ends inside an `Nppm` field is refused as
//! `Marker`, as OpenJPEG refuses it. Table A.38 gives `Nppm` the range 0 to
//! 2^32 - 1, so an `Nppm` of 0 is read as written: that tile-part has no packet header,
//! and its data holds none either (A.7.4: with a `PPM` present, "packets
//! distributed in the bit stream of the tile-parts are disallowed"). See "A
//! `PPM` whose `Nppm`s are all 0" in `docs/openjpeg-departures.md`.
//!
//! The files are built here: the main header of
//! `tests/fixtures/packed_overrun/ppm_whole.j2k` (one layer) or
//! `ppm_two_parts.j2k` (two layers), then the `PPM` segments under test,
//! the tile-parts and `EOC`. The packet header is `C4 FF 00`, as in those
//! fixtures, and the body seven 0x00 bytes; OpenJPEG decodes it to 176.

#[path = "support/synth.rs"]
mod synth;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder, Refined, decode_with, read_header_with};
use synth::{bx, colr_enum, ihdr_for, jp2_file, seg, tile_part};

/// The packet header of the fixtures' one packet.
const HEADER: [u8; 3] = [0xC4, 0xFF, 0x00];

/// The fixtures' tile body.
const BODY: [u8; 7] = [0; 7];

/// `SOC`, `SIZ`, `COD` and `QCD` of the fixture `name`: the bytes before its
/// `PPM`.
fn main_header(name: &str) -> Vec<u8> {
    let file = synth::read_file(synth::fixtures("packed_overrun").join(name));
    let ppm = file.windows(2).position(|w| w == [0xFF, 0x60]).unwrap();
    file[..ppm].to_vec()
}

/// A `PPM` segment for each payload in `payloads`, numbered by `Zppm` from 0.
fn ppms(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for (zppm, payload) in payloads.iter().enumerate() {
        let body = [&[u8::try_from(zppm).unwrap()][..], payload].concat();
        out.extend(seg(0xFF60, &body));
    }
    out
}

/// One layer in one tile-part, its body `body`.
fn one_part(payloads: &[Vec<u8>], body: &[u8]) -> Vec<u8> {
    let mut out = main_header("ppm_whole.j2k");
    out.extend(ppms(payloads));
    out.extend(tile_part(0, 0, 1, &[], body));
    out.extend([0xFF, 0xD9]);
    out
}

/// Two layers in two tile-parts: the fixtures' body, then an empty part.
fn two_parts(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = main_header("ppm_two_parts.j2k");
    out.extend(ppms(payloads));
    out.extend(tile_part(0, 0, 2, &[], &BODY));
    out.extend(tile_part(0, 1, 2, &[], &[]));
    out.extend([0xFF, 0xD9]);
    out
}

/// `n` as an `Nppm` field.
fn nppm(n: u32) -> Vec<u8> {
    n.to_be_bytes().to_vec()
}

/// Where the first `SOT` marker starts.
fn sot_offset(bytes: &[u8]) -> usize {
    bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap()
}

/// `codestream` in a JP2 file, with the offset it starts at.
fn in_jp2(codestream: &[u8]) -> (Vec<u8>, usize) {
    let header = [ihdr_for(codestream), bx(b"colr", &colr_enum(17))];
    let file = jp2_file(&header, &[], codestream, &[]);
    let start = file.len() - codestream.len();
    (file, start)
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// `result`'s error as its variant and message: [`Error`] has no
/// `PartialEq`.
fn keyed<T>(result: Result<T, Error>) -> String {
    match result {
        Ok(_) => "Ok".to_owned(),
        Err(error) => format!("{error:?}"),
    }
}

/// A decode's outcome: its samples and whether the report is complete, or
/// its error.
fn outcome(result: Result<rust_j2k::Image, Error>) -> String {
    match result {
        Ok(image) => format!(
            "Ok({:?}, complete {})",
            image.components[0].samples,
            image.report.is_complete()
        ),
        Err(error) => format!("{error:?}"),
    }
}

/// Check every prefix of `bytes`: before `settled` bytes the main header is
/// cut short, and from there on every path gives `want`. Then the file is
/// fed to a decoder one byte at a time: `header` gives `want` from `settled`
/// on, no `refine` before it refuses, the first that does and every later
/// one give `want` (a `refine` decodes only once the input has doubled), and
/// so does `finish`.
fn check_refused(name: &str, bytes: &[u8], settled: usize, want: &str) {
    for length in 0..=bytes.len() {
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
                assert_eq!(keyed(header), want, "{at}, read_header_with, {strict}");
                assert_eq!(keyed(decoded), want, "{at}, decode_with, {strict}");
                assert_eq!(keyed(fed_header), want, "{at}, header, {strict}");
                assert_eq!(keyed(refined), want, "{at}, refine, {strict}");
                assert_eq!(keyed(decoder.finish()), want, "{at}, finish, {strict}");
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

/// `check_refused` on `codestream` and on it in a JP2 file. The main header
/// is settled once the input holds the first `SOT` marker.
fn check_refused_both(name: &str, codestream: &[u8], want: &str) {
    let settled = sot_offset(codestream) + 2;
    check_refused(name, codestream, settled, want);
    let (file, start) = in_jp2(codestream);
    check_refused(&format!("{name}, in JP2"), &file, start + settled, want);
}

fn split_error(zppm: u8, into: usize) -> String {
    format!(
        "Marker(\"`PPM` with `Zppm` {zppm} ends {into} byte(s) into an `Nppm` field; an `Nppm` field may not continue in the next `PPM`\")"
    )
}

/// An `Nppm` field split across `PPM` segments, at each of its three inner
/// bytes and across three segments, and the second record's field in the
/// two-layer file: `Marker` from the prefix that settles the main header on,
/// with `strict` on and off.
///
/// opj_decompress 2.5.0 refuses each of these files: "[ERROR] Not enough
/// bytes to read Nppm" (`opj_j2k_merge_ppm`, j2k.c:3958), exit status 1.
#[test]
fn an_nppm_split_across_ppm_segments_is_refused_on_every_prefix() {
    let field = nppm(3);
    for cut in 1..4 {
        let bytes = one_part(
            &[field[..cut].to_vec(), [&field[cut..], &HEADER[..]].concat()],
            &BODY,
        );
        check_refused_both(&format!("cut {cut}"), &bytes, &split_error(0, cut));
    }
    let three = one_part(
        &[
            field[..2].to_vec(),
            field[2..3].to_vec(),
            [&field[3..], &HEADER[..]].concat(),
        ],
        &BODY,
    );
    check_refused_both("three segments", &three, &split_error(0, 2));

    let second = two_parts(&[
        [nppm(3), HEADER.to_vec(), nppm(1)[..2].to_vec()].concat(),
        [&nppm(1)[2..], &[0][..]].concat(),
    ]);
    check_refused_both("second record", &second, &split_error(0, 2));
}

/// The control: the same two-layer file with its `PPM` split between the
/// two records, where the first segment ends with a completed packet
/// header, decodes to 176, complete, with `strict` on and off.
/// opj_decompress 2.5.0 decodes it to 176 with no error.
#[test]
fn ppm_segments_split_between_records_decode() {
    let bytes = two_parts(&[
        [nppm(3), HEADER.to_vec()].concat(),
        [nppm(1), vec![0]].concat(),
    ]);
    for strict in [true, false] {
        assert_eq!(
            outcome(decode_with(&bytes, &options(strict))),
            "Ok([176], complete true)",
            "strict {strict}"
        );
    }
}

/// An `Nppm` of 0 beside one that is not: both layers' packet headers in
/// the first tile-part's record, and none in the second's. The second
/// tile-part holds no packet, so the file decodes to 176, complete, with
/// `strict` on and off. opj_decompress 2.5.0 decodes it to 176 with no
/// error.
#[test]
fn an_nppm_of_0_beside_one_that_is_not_decodes() {
    let bytes = two_parts(&[[nppm(4), HEADER.to_vec(), vec![0], nppm(0)].concat()]);
    for strict in [true, false] {
        assert_eq!(
            outcome(decode_with(&bytes, &options(strict))),
            "Ok([176], complete true)",
            "strict {strict}"
        );
    }
}

/// What `decode_with` gives on `bytes` with `strict` off, which is what a
/// `refine` that decodes gives.
fn partial(bytes: &[u8]) -> String {
    outcome(decode_with(bytes, &options(false)))
}

/// A `PPM` whose `Nppm`s are all 0 is read: no prefix gives a `PPM` error.
///
/// Until the input holds the first `SOT` marker the main header is cut
/// short. From there `read_header_with` and the incremental `header`
/// succeed. The tile-parts have no packet header, so a strict decode
/// refuses the whole file ("the packed packet headers of tile 0 end before
/// the last packet") and a decode with `strict` off returns the image with the packets reported missing,
/// its sample 128 (all coefficients 0). No prefix decodes to 176: the data
/// is not read for packet headers. From the settled prefix on, `finish`
/// gives what `decode_with` gives, and a `refine` that decodes gives what
/// `decode_with` gives with `strict` off, on each prefix and with the file
/// fed one byte at a time.
///
/// `in_band` puts the fixtures' packet header in the tile body, where a
/// decoder that took an empty `PPM` for none would read it and give 176.
///
/// opj_decompress 2.5.0 refuses each of these files: "[ERROR] Not enough
/// memory to read PPM marker", exit status 1. The sum of the `Nppm`s is
/// 0, and `opj_j2k_merge_ppm` allocates that many bytes; `opj_malloc(0)`
/// returns NULL (j2k.c:3985-3987).
#[test]
fn a_ppm_whose_nppms_are_all_zero_is_read_on_every_prefix() {
    let in_band = [&HEADER[..], &BODY].concat();
    let one_layer = one_part(&[nppm(0)], &BODY);
    let in_band = one_part(&[nppm(0)], &in_band);
    let two_segments = two_parts(&[nppm(0), nppm(0)]);
    let two_records = two_parts(&[[nppm(0), nppm(0)].concat()]);
    for (name, bytes) in [
        ("one layer", one_layer),
        ("in band", in_band),
        ("two segments", two_segments),
        ("two records", two_records),
    ] {
        let settled = sot_offset(&bytes) + 2;
        for length in 0..=bytes.len() {
            let prefix = &bytes[..length];
            let at = format!("{name}, {length} of {} bytes", bytes.len());
            for strict in [true, false] {
                let header = read_header_with(prefix, &options(strict));
                let decoded = outcome(decode_with(prefix, &options(strict)));
                let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
                decoder.feed(prefix);
                let fed_header = decoder.header();
                let refined = decoder.refine();
                let finished = outcome(decoder.finish());
                if length < settled {
                    let header = keyed(header);
                    assert!(header.starts_with("Incomplete("), "{at}: {header}");
                    assert!(decoded.starts_with("Incomplete("), "{at}: {decoded}");
                    assert!(matches!(fed_header, Ok(None)), "{at}: {fed_header:?}");
                    continue;
                }
                assert_eq!(finished, decoded, "{at}, finish, strict {strict}");
                assert!(header.is_ok(), "{at}, strict {strict}: {header:?}");
                assert!(
                    matches!(fed_header, Ok(Some(_))),
                    "{at}, strict {strict}: {fed_header:?}"
                );
                match refined {
                    Ok(Refined::Image(image)) => {
                        assert_eq!(outcome(Ok(image)), partial(prefix), "{at}, refine")
                    }
                    Ok(_) => {}
                    Err(error) => panic!("{at}, refine, strict {strict}: {error:?}"),
                }
                assert!(!decoded.contains("PPM"), "{at}, strict {strict}: {decoded}");
                assert!(
                    !decoded.starts_with("Marker("),
                    "{at}, strict {strict}: {decoded}"
                );
                if strict {
                    assert!(decoded.starts_with("Codestream("), "{at}: {decoded}");
                } else {
                    assert!(decoded.ends_with("complete false)"), "{at}: {decoded}");
                }
            }
        }
        for strict in [true, false] {
            let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
            for length in 1..=bytes.len() {
                let at = format!("{name}, {length} bytes fed one at a time, strict {strict}");
                decoder.feed(&bytes[length - 1..length]);
                match decoder.refine() {
                    Ok(Refined::Image(image)) => {
                        assert_eq!(outcome(Ok(image)), partial(&bytes[..length]), "{at}")
                    }
                    Ok(_) => {}
                    Err(error) => panic!("{at}: {error:?}"),
                }
            }
            assert_eq!(
                outcome(decoder.finish()),
                outcome(decode_with(&bytes, &options(strict))),
                "{name}, finish, strict {strict}"
            );
        }
        assert_eq!(
            outcome(decode_with(&bytes, &options(true))),
            "Codestream(\"the packed packet headers of tile 0 end before the last packet\")",
            "{name}"
        );
        assert_eq!(
            outcome(decode_with(&bytes, &options(false))),
            "Ok([128], complete false)",
            "{name}"
        );
    }
}

/// A decoder's `header` as `Ok(None)` (not ready), `Ok(Some)` or its error.
fn header_state<T>(result: Result<Option<T>, Error>) -> String {
    match result {
        Ok(None) => "Ok(None)".to_owned(),
        Ok(Some(_)) => "Ok(Some)".to_owned(),
        Err(error) => format!("{error:?}"),
    }
}

/// What each path gives on `prefix`, with `strict` as given: the header, the
/// decode, and an incremental decoder fed the prefix whole, after `header`,
/// `refine` and `finish`.
fn paths(prefix: &[u8], strict: bool) -> Vec<String> {
    let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
    decoder.feed(prefix);
    let fed_header = header_state(decoder.header());
    let refined = match decoder.refine() {
        Ok(Refined::Image(image)) => outcome(Ok(image)),
        Ok(_) => "no image".to_owned(),
        Err(error) => format!("{error:?}"),
    };
    vec![
        keyed(read_header_with(prefix, &options(strict))),
        outcome(decode_with(prefix, &options(strict))),
        fed_header,
        refined,
        outcome(decoder.finish()),
    ]
}

/// `shape` and `conforming` differ only in their `PPM` segments, so the bytes
/// from the first `SOT` marker on are the same. On every prefix of `shape`,
/// with `strict` on and off, each path gives what it gives on the conforming
/// file's prefix of the same tail. Before the main header is settled that is
/// input that runs out, for the one-shot calls and the incremental decoder
/// alike; from there on the header is ready and nothing is a `PPM` error. The
/// whole file decodes to 176, complete, also fed one byte at a time. Each is
/// run on the codestream and on it in a JP2 file.
fn check_same_as_conforming(name: &str, shape: &[u8], conforming: &[u8]) {
    check_same_as_conforming_at(name, shape, conforming, 0, 0);
    let (shape_file, shape_start) = in_jp2(shape);
    let (conforming_file, conforming_start) = in_jp2(conforming);
    check_same_as_conforming_at(
        &format!("{name}, in JP2"),
        &shape_file,
        &conforming_file,
        shape_start,
        conforming_start,
    );
}

/// `check_same_as_conforming` on files whose codestreams start at
/// `shape_start` and `conforming_start`.
fn check_same_as_conforming_at(
    name: &str,
    shape: &[u8],
    conforming: &[u8],
    shape_start: usize,
    conforming_start: usize,
) {
    let shape_sot = shape_start + sot_offset(&shape[shape_start..]);
    let conforming_sot = conforming_start + sot_offset(&conforming[conforming_start..]);
    assert_eq!(shape[shape_sot..], conforming[conforming_sot..], "{name}");
    for strict in [true, false] {
        for length in 0..=shape.len() {
            let at = format!("{name}, {length} of {} bytes, strict {strict}", shape.len());
            let got = paths(&shape[..length], strict);
            if length < shape_sot + 2 {
                assert!(got[0].starts_with("Incomplete("), "{at}: {got:?}");
                assert!(got[1].starts_with("Incomplete("), "{at}: {got:?}");
                assert_eq!(got[2], "Ok(None)", "{at}: {got:?}");
                assert_eq!(got[3], "no image", "{at}: {got:?}");
                // `FileFormat` while a JP2 box is cut short.
                assert!(
                    got[4].starts_with("Codestream(\"the input is truncated")
                        || got[4].starts_with("FileFormat(\"the input is truncated"),
                    "{at}: {got:?}"
                );
            } else {
                let tail = length - shape_sot;
                let want = paths(&conforming[..conforming_sot + tail], strict);
                assert_eq!(got, want, "{at}");
                assert!(!got[0].starts_with("Marker("), "{at}: {got:?}");
                assert!(!got[1].contains("PPM"), "{at}: {got:?}");
                assert_eq!(got[2], "Ok(Some)", "{at}: {got:?}");
            }
        }
        assert_eq!(
            outcome(decode_with(shape, &options(strict))),
            "Ok([176], complete true)",
            "{name}, strict {strict}"
        );
        // One byte at a time, then `finish`.
        let mut decoder = IncrementalDecoder::new(options(strict)).unwrap();
        for (index, byte) in shape.iter().enumerate() {
            let at = format!(
                "{name}, {} bytes fed one at a time, strict {strict}",
                index + 1
            );
            decoder.feed(std::slice::from_ref(byte));
            let header = header_state(decoder.header());
            if index + 1 < shape_sot + 2 {
                assert_eq!(header, "Ok(None)", "{at}");
            } else {
                assert_eq!(header, "Ok(Some)", "{at}");
            }
            if let Err(error) = decoder.refine() {
                panic!("{at}: refine: {error:?}");
            }
        }
        assert_eq!(
            outcome(decoder.finish()),
            "Ok([176], complete true)",
            "{name}, fed one byte at a time, strict {strict}"
        );
    }
}

/// A `PPM` segment that ends after a whole `Nppm` but before that packet
/// header (a), or partway through a packet header (b), breaks A.7.4: "Every
/// marker segment in this series shall end with a completed packet header."
/// No bytes are lost, so both are read, as OpenJPEG reads them. Each shape
/// decodes to 176, complete, as the conforming file does, with `strict` on
/// and off and on every prefix. The shapes are built on the one-layer and
/// the two-layer files, the second at either record.
///
/// opj_decompress 2.5.0 decodes every one of these files to 176, the same
/// as the conforming file, with no error.
#[test]
fn ppm_segments_ending_before_or_inside_a_packet_header_decode() {
    let whole = [nppm(3), HEADER.to_vec()].concat();
    let conforming = one_part(std::slice::from_ref(&whole), &BODY);
    // (a) segment 1 ends after the whole `Nppm`.
    let after_nppm = one_part(&[nppm(3), HEADER.to_vec()], &BODY);
    check_same_as_conforming("one layer, (a)", &after_nppm, &conforming);
    // (b) segment 1 ends partway through the packet header.
    for cut in 1..HEADER.len() {
        let inside = one_part(&[whole[..4 + cut].to_vec(), HEADER[cut..].to_vec()], &BODY);
        check_same_as_conforming(&format!("one layer, (b) cut {cut}"), &inside, &conforming);
    }

    let two = two_parts(&[
        [nppm(3), HEADER.to_vec()].concat(),
        [nppm(1), vec![0]].concat(),
    ]);
    for (name, segments) in [
        (
            "two layers, (a) first",
            vec![nppm(3), [HEADER.to_vec(), nppm(1), vec![0]].concat()],
        ),
        (
            "two layers, (a) second",
            vec![[nppm(3), HEADER.to_vec(), nppm(1)].concat(), vec![0]],
        ),
        (
            "two layers, (b) first",
            vec![
                [nppm(3), HEADER[..2].to_vec()].concat(),
                [HEADER[2..].to_vec(), nppm(1), vec![0]].concat(),
            ],
        ),
        (
            "two layers, three segments",
            vec![
                nppm(3),
                HEADER[..1].to_vec(),
                [&HEADER[1..], &nppm(1), &[0][..]].concat(),
            ],
        ),
    ] {
        check_same_as_conforming(name, &two_parts(&segments), &two);
    }
}
