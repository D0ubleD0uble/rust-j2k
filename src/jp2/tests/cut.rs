//! A [`BoxesCut`] walk that goes on from where it stopped gives what
//! [`parse_boxes`] gives from the start: on the prefixes of every JP2
//! fixture up to just past its `jp2c` box header, and of files built here
//! with each fault of the boxes before the codestream.
//!
//! The rules themselves are one walk that both use, so these tests guard
//! the state a [`BoxesCut`] carries between calls: where it stopped and the
//! first `ihdr`. What the rules give is checked by the reject matrix and
//! the oracle tests.

use super::super::{BoxFault, BoxesCut, parse_boxes, read_file_type};
use super::*;

/// The error [`parse_boxes`] gives on `bytes` where a [`BoxesCut`] is kept:
/// [`Error::Incomplete`], and not open.
fn fresh_cut(bytes: &[u8]) -> Option<String> {
    match parse_boxes(bytes, &DecodeOptions::default().packet_work_limit) {
        Err(BoxFault {
            error: error @ Error::Incomplete(_),
            open: false,
        }) => Some(format!("{error:?}")),
        _ => None,
    }
}

/// Where each top-level box of `bytes` starts, in order, up to and
/// including the first `jp2c`, with where that `jp2c` starts. The walk
/// stops at a box of length 0, at one whose length is short of its header,
/// and at one that runs past the end of `bytes`.
fn top_level_boxes(bytes: &[u8]) -> (Vec<usize>, Option<usize>) {
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(header) = bytes.get(at..at + 8) {
        starts.push(at);
        let lbox = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        if &header[4..] == b"jp2c" {
            return (starts, Some(at));
        }
        let length = match lbox {
            1 => match bytes.get(at + 8..at + 16) {
                Some(xl) => u64::from_be_bytes(xl.try_into().unwrap()),
                None => break,
            },
            n => u64::from(n),
        };
        match usize::try_from(length) {
            Ok(length) if length >= 8 && at + length <= bytes.len() => at += length,
            _ => break,
        }
    }
    (starts, None)
}

/// A file whose bytes before its `jp2c` box this many or fewer has every
/// prefix checked; a longer one, only those near a box boundary.
const DENSE: usize = 4096;

/// How near a box boundary, either side, a prefix of a long file is
/// checked: the box's header and its first and last bytes.
const NEAR: usize = 24;

/// Check prefixes of `bytes` up to 64 bytes past its first top-level
/// `jp2c`: [`BoxesCut::start`] keeps a walk where `parse_boxes` gives a
/// kept error after a whole `ftyp`, and a walk started on the prefix, fed
/// one byte at a time and then in growing steps, gives what `parse_boxes`
/// gives on each longer prefix until it stops. Every prefix of a short
/// file is checked, and those within [`NEAR`] bytes of a box boundary of a
/// long one, so that the work does not grow with the size of a box.
/// Returns how many resumed walks it checked.
fn check(name: &str, bytes: &[u8]) -> usize {
    let (starts, jp2c) = top_level_boxes(bytes);
    let end = jp2c.map_or(bytes.len(), |at| bytes.len().min(at + 64));
    let near = |a: usize| {
        end <= DENSE || a + NEAR >= end || {
            // The box starts either side of `a`.
            let after = starts.partition_point(|&start| start < a);
            let either = [after.checked_sub(1), Some(after)];
            either
                .into_iter()
                .flatten()
                .filter_map(|index| starts.get(index))
                .any(|&start| start.abs_diff(a) <= NEAR)
        }
    };
    let mut checked = 0;
    for a in (0..=end).filter(|&a| near(a)) {
        let prefix = &bytes[..a];
        let started = BoxesCut::start(prefix);
        let expected = fresh_cut(prefix).is_some() && read_file_type(prefix).is_ok();
        assert_eq!(started.is_some(), expected, "{name}: start at {a}");
        let Some(mut cut) = started else {
            continue;
        };
        let mut b = a;
        let mut step = 1;
        while b < end {
            b = end.min(b + step);
            if b - a >= 16 {
                step += step.div_ceil(2);
            }
            let got = cut.resume(&bytes[..b]).map(|error| format!("{error:?}"));
            assert_eq!(got, fresh_cut(&bytes[..b]), "{name}: from {a} to {b}");
            checked += 1;
            if got.is_none() {
                break;
            }
        }
    }
    checked
}

/// Every `.jp2` file under `tests/fixtures/`.
fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut dirs = vec![synth::fixtures("")];
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("fixture directory") {
            let path = entry.expect("fixture entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == "jp2") {
                let bytes = std::fs::read(&path).expect("fixture");
                files.push((path.display().to_string(), bytes));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn a_resumed_box_walk_matches_a_read_from_the_start_on_every_fixture() {
    let files = fixtures();
    // The packaged fixtures alone hold more than 20.
    assert!(files.len() > 20, "{} fixtures", files.len());
    let checked: usize = files.iter().map(|(name, bytes)| check(name, bytes)).sum();
    println!("{} files, {checked} resumed walks", files.len());
}

/// A file of the signature and `boxes`.
fn file(boxes: &[Vec<u8>]) -> Vec<u8> {
    [SIGNATURE.to_vec(), boxes.concat()].concat()
}

/// Files with each fault of the boxes before the codestream, and the
/// shapes around them that a walk reads past, by name.
fn fault_files() -> Vec<(&'static str, Vec<u8>)> {
    let ftyp = synth::ftyp_jp2();
    let ihdr = synth::ihdr_for(CODESTREAM);
    let colr = bx(b"colr", &synth::colr_enum(16));
    let jp2h = bx(b"jp2h", &[ihdr.clone(), colr.clone()].concat());
    let no_ihdr = bx(b"jp2h", &colr);
    let jp2c = bx(b"jp2c", CODESTREAM);
    let xl_jp2c = xl_box(b"jp2c", CODESTREAM);
    let free = bx(b"free", &[]);
    let xml = bx(b"xml ", b"<a/>");
    let rreq = bx(b"rreq", &[1, 0xFF, 0x80, 0, 0, 0, 0]);
    vec![
        ("plain", file(&[ftyp.clone(), jp2h.clone(), jp2c.clone()])),
        (
            // Past `DENSE` bytes, with the type of the codestream box
            // inside a payload.
            "a long xml box that holds jp2c",
            file(&[
                ftyp.clone(),
                bx(
                    b"xml ",
                    &[&b"<a>"[..], &b"jp2c"[..], &[b' '; 6000], b"</a>"].concat(),
                ),
                jp2h.clone(),
                jp2c.clone(),
            ]),
        ),
        (
            "boxes around the jp2h",
            file(&[
                ftyp.clone(),
                free.clone(),
                xml.clone(),
                jp2h.clone(),
                free.clone(),
                xml.clone(),
                jp2c.clone(),
            ]),
        ),
        (
            "top-level header boxes and repeats",
            file(&[
                ftyp.clone(),
                rreq.clone(),
                jp2h.clone(),
                colr.clone(),
                bx(b"ihdr", &ihdr[8..]),
                rreq.clone(),
                bx(b"uuid", &[7; 20]),
                jp2c.clone(),
            ]),
        ),
        (
            "a jp2c with a 64-bit length",
            file(&[ftyp.clone(), jp2h.clone(), free.clone(), xl_jp2c.clone()]),
        ),
        (
            "a second jp2h",
            file(&[
                ftyp.clone(),
                jp2h.clone(),
                free.clone(),
                jp2h.clone(),
                jp2c.clone(),
            ]),
        ),
        (
            "a jp2h with no ihdr",
            file(&[ftyp.clone(), free.clone(), no_ihdr.clone(), jp2c.clone()]),
        ),
        (
            "a second jp2h with no ihdr",
            file(&[
                ftyp.clone(),
                jp2h.clone(),
                free.clone(),
                no_ihdr.clone(),
                jp2c.clone(),
            ]),
        ),
        (
            "an ihdr of 13 bytes",
            file(&[
                ftyp.clone(),
                bx(b"jp2h", &bx(b"ihdr", &[0; 13])),
                jp2c.clone(),
            ]),
        ),
        (
            // Not read: only the first `ihdr` of the file is.
            "a second jp2h with an ihdr of 13 bytes",
            file(&[
                ftyp.clone(),
                jp2h.clone(),
                bx(b"jp2h", &bx(b"ihdr", &[0; 13])),
                jp2c.clone(),
            ]),
        ),
        (
            "a jp2h child past its parent",
            file(&[
                ftyp.clone(),
                bx(b"jp2h", &[ihdr.clone(), header_only(40, b"colr")].concat()),
                jp2c.clone(),
            ]),
        ),
        (
            "a box of length 4",
            file(&[
                ftyp.clone(),
                jp2h.clone(),
                header_only(4, b"free"),
                jp2c.clone(),
            ]),
        ),
        (
            "an XLBox below 16",
            file(&[
                ftyp.clone(),
                jp2h.clone(),
                xl_header_only(8, b"free"),
                jp2c.clone(),
            ]),
        ),
        (
            "a box of length 0",
            file(&[ftyp.clone(), jp2h.clone(), open_box(b"free", &jp2c)]),
        ),
        (
            "a box of length 0 before the jp2h",
            file(&[
                ftyp.clone(),
                open_box(b"free", &[jp2h.clone(), jp2c.clone()].concat()),
            ]),
        ),
        (
            "a jp2h of length 0",
            file(&[
                ftyp.clone(),
                free.clone(),
                open_box(b"jp2h", &[ihdr.clone(), colr.clone()].concat()),
            ]),
        ),
        (
            "a jp2c with no jp2h",
            file(&[ftyp.clone(), free.clone(), jp2c.clone(), jp2h.clone()]),
        ),
        (
            "a jp2c with a 64-bit length and no jp2h",
            file(&[ftyp.clone(), free.clone(), xl_jp2c.clone(), jp2h.clone()]),
        ),
        (
            "an ftyp not compatible with jp2",
            file(&[bx(b"ftyp", b"jpx \0\0\0\0jpx "), jp2h.clone(), jp2c.clone()]),
        ),
        (
            "an ftyp of length 0",
            file(&[open_box(
                b"ftyp",
                &[&b"jp2 \0\0\0\0jp2 "[..], &jp2h, &jp2c].concat(),
            )]),
        ),
        (
            "a jp2h before the ftyp",
            file(&[jp2h.clone(), ftyp.clone(), jp2c.clone()]),
        ),
    ]
}

#[test]
fn a_resumed_box_walk_matches_a_read_from_the_start_on_each_box_fault() {
    for (name, bytes) in &fault_files() {
        // A walk is kept on some prefix of each, but of a bad `ftyp`.
        let kept = check(name, bytes) > 0;
        assert_eq!(kept, !name.contains("ftyp"), "{name}: kept a walk");
    }
}
