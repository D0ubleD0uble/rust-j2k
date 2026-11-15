//! Input that ends before the codestream's main header is whole is
//! `Error::Incomplete`, apart from a header that is all there but damaged,
//! which keeps its own error.
//!
//! Each committed codestream, and each committed JP2 file, is cut at every
//! byte from the empty input to just before the first `SOT` marker is whole:
//! for a JP2 file that takes in its signature, its boxes before `jp2c`, the
//! `jp2c` box header and the codestream's main header. The bounds come from
//! a marker and box walk written here (A.4, A.6, I.4), not from the decoder.

use std::path::{Path, PathBuf};

use rust_j2k::{DecodeOptions, Error, decode, decode_with, read_header};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
    ])))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<usize> {
    let b = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// The offset of the first `SOT` marker of a codestream, found by stepping
/// over the main header's marker segments; `None` if `cs` is not a
/// codestream whose main header walks to a `SOT`.
fn first_sot(cs: &[u8]) -> Option<usize> {
    walk(cs).map(|(sot, _)| sot)
}

/// Walk a codestream's main header: the offset of its first `SOT` marker,
/// and the end of its first `COD` or `COC` segment whose code-block style
/// sets an HTJ2K bit (`0x40` or `0x80`, ISO/IEC 15444-15 A.4), if any.
fn walk(cs: &[u8]) -> Option<(usize, Option<usize>)> {
    if !cs.starts_with(&[0xFF, 0x4F]) {
        return None;
    }
    let (mut at, mut csiz, mut ht) = (2, 0, None);
    loop {
        let code = u16_at(cs, at)?;
        match code {
            0xFF90 => return Some((at, ht)),
            // The reserved range carries no length (A.1.3).
            0xFF30..=0xFF3F => at += 2,
            _ if code >> 8 == 0xFF => {
                let length = u16_at(cs, at + 2)?;
                if length < 2 {
                    return None;
                }
                // A body past the input or too short for its field holds no
                // style; the walk goes on, as it did before it read styles.
                let body = cs.get(at + 4..at + 2 + length).unwrap_or(&[]);
                // The style byte: after `Scod`, `SGcod` and three `SPcod`
                // bytes in COD; after the component index, `Scoc` and three
                // bytes in COC (A.6.1, A.6.2).
                let style = match code {
                    0xFF51 => {
                        csiz = u16_at(body, 34).unwrap_or(0);
                        None
                    }
                    0xFF52 => body.get(8),
                    0xFF53 => body.get(if csiz < 257 { 5 } else { 6 }),
                    _ => None,
                };
                at += 2 + length;
                if ht.is_none() && style.is_some_and(|s| s & 0xC0 != 0) {
                    ht = Some(at);
                }
            }
            _ => return None,
        }
    }
}

/// The offsets of the first top-level `jp2c` box's payload and of the box's
/// end in a JP2 file.
fn jp2c_payload(file: &[u8]) -> Option<(usize, usize)> {
    if !file.starts_with(&[0, 0, 0, 12, b'j', b'P', b' ', b' ']) {
        return None;
    }
    let mut at = 0;
    while at + 8 <= file.len() {
        let (header, length) = match u32_at(file, at)? {
            0 => (8, file.len() - at),
            1 => {
                let high = u32_at(file, at + 8)?;
                let low = u32_at(file, at + 12)?;
                (16, (high << 32) | low)
            }
            n => (8, n),
        };
        if &file[at + 4..at + 8] == b"jp2c" {
            return Some((at + header, at.saturating_add(length)));
        }
        if length < header {
            return None;
        }
        at = at.checked_add(length)?;
    }
    None
}

fn lenient() -> DecodeOptions {
    DecodeOptions::default().with_strict(false)
}

/// Every prefix of `bytes` that ends before the whole first `SOT` marker of
/// the codestream which starts at `cs_at`, with that `SOT` at `cs_at + sot`,
/// is `Incomplete`: from `read_header`, a strict decode and a partial one.
/// That includes the empty prefix and, in a JP2 file, every prefix that ends
/// in the boxes before the codestream. With `boundary`, the prefix that holds
/// the whole `SOT` marker is not.
fn assert_cuts_are_incomplete(name: &str, bytes: &[u8], cs_at: usize, sot: usize, boundary: bool) {
    for length in 0..cs_at + sot + 2 {
        let prefix = &bytes[..length];
        let what = format!("{name} cut at {length}");
        match read_header(prefix) {
            Err(Error::Incomplete(_)) => {}
            other => panic!("{what}: read_header gave {other:?}"),
        }
        match decode(prefix) {
            Err(Error::Incomplete(_)) => {}
            other => panic!("{what}: decode gave {:?}", other.map(|_| ())),
        }
        match decode_with(prefix, &lenient()) {
            Err(Error::Incomplete(_)) => {}
            other => panic!("{what}: partial decode gave {:?}", other.map(|_| ())),
        }
    }
    if !boundary {
        return;
    }
    // The main header ends with the first SOT marker: a header that reads in
    // full reads from this prefix too. (A JP2 file's boxes after the
    // codestream are not in the prefix, so only the image is compared.)
    let at_sot = &bytes[..cs_at + sot + 2];
    match (read_header(bytes), read_header(at_sot)) {
        (Ok(whole), Ok(cut)) => assert_eq!(
            (
                whole.width,
                whole.height,
                whole.components,
                whole.max_reduction
            ),
            (cut.width, cut.height, cut.components, cut.max_reduction),
            "{name}"
        ),
        (Ok(_), other) => panic!(
            "{name}: the header through SOT gave {:?}",
            other.map(|_| ())
        ),
        (Err(_), other) => assert!(
            !matches!(other, Err(Error::Incomplete(_))),
            "{name}: the header through SOT is still incomplete"
        ),
    }
}

/// `bytes` is refused for its HTJ2K block coder by `read_header`, a strict
/// decode and a partial one.
fn assert_refused_for_ht(what: &str, bytes: &[u8]) {
    let results = [
        read_header(bytes).map(|_| ()),
        decode(bytes).map(|_| ()),
        decode_with(bytes, &lenient()).map(|_| ()),
    ];
    for result in results {
        assert!(
            matches!(&result, Err(Error::Unsupported(m)) if m.contains("code-block style (HTJ2K")),
            "{what}: {result:?}"
        );
    }
}

#[test]
fn every_cut_inside_the_main_header_is_incomplete() {
    let mut paths = Vec::new();
    files(&fixtures(), &mut paths);
    paths.sort();
    let (mut codestreams, mut ht_codestreams, mut jp2s) = (0, 0, 0);
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let name = path.strip_prefix(fixtures()).unwrap().display().to_string();
        if let Some((sot, ht)) = walk(&bytes) {
            match ht {
                // An HTJ2K block coder is refused as soon as the COD or COC
                // that selects it is read: a cut before it is `Incomplete`,
                // and every cut that holds it, up to the whole file, is
                // refused for the coder.
                Some(ht_end) => {
                    assert_cuts_are_incomplete(&name, &bytes, 0, ht_end - 2, false);
                    for length in (ht_end..sot + 2).chain([bytes.len()]) {
                        assert_refused_for_ht(&format!("{name} cut at {length}"), &bytes[..length]);
                    }
                    ht_codestreams += 1;
                }
                None => assert_cuts_are_incomplete(&name, &bytes, 0, sot, true),
            }
            codestreams += 1;
        } else if let Some((cs_at, box_end)) = jp2c_payload(&bytes) {
            // Only files whose box layer reads, since a fault there is
            // reported before the codestream is reached.
            let Some(sot) = first_sot(&bytes[cs_at..]) else {
                continue;
            };
            if read_header(&bytes).is_err() {
                continue;
            }
            // A `jp2c` box that ends inside the main header
            // (`psot0/jp2_p0eoc_cut_in_com.jp2`) is still `Incomplete` at the
            // prefix through the `SOT` marker. Where its codestream ends is
            // the box layer's call (`src/jp2/after.rs`); the whole file
            // decodes.
            let boundary = box_end >= cs_at + sot + 2;
            assert_cuts_are_incomplete(&name, &bytes, cs_at, sot, boundary);
            jp2s += 1;
        }
    }
    // The fixtures that ship with the crate alone give more than these.
    eprintln!("cut {codestreams} codestreams ({ht_codestreams} HT) and {jp2s} JP2 files");
    assert!(codestreams >= 100, "only {codestreams} codestreams cut");
    // The 41 HT conformance codestreams and the bare OpenJPH ones.
    assert!(
        ht_codestreams >= 41,
        "only {ht_codestreams} HT codestreams cut"
    );
    assert!(jp2s >= 30, "only {jp2s} JP2 files cut");
}

/// Empty input, and the first byte of `SOC` or of the JP2 signature, may
/// still become JPEG 2000, so they are `Incomplete`. A short input that can
/// no longer become either is `Codestream`, as text is.
#[test]
fn input_shorter_than_a_magic_is_incomplete_while_it_could_still_match() {
    let incomplete: [&[u8]; 4] = [&[], &[0xFF], &[0x00], &[0, 0, 0, 12, b'j', b'P', b' ']];
    for prefix in incomplete {
        for result in [
            read_header(prefix).map(|_| ()),
            decode(prefix).map(|_| ()),
            decode_with(prefix, &lenient()).map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(Error::Incomplete(_))),
                "{prefix:02X?}: {result:?}"
            );
        }
    }
    let not_jpeg_2000: [&[u8]; 4] = [&[0x41], &[0xFF, 0x4E], &[0x00, 0x01], b"hello"];
    for bytes in not_jpeg_2000 {
        for result in [
            read_header(bytes).map(|_| ()),
            decode(bytes).map(|_| ()),
            decode_with(bytes, &lenient()).map(|_| ()),
        ] {
            assert_eq!(
                result,
                Err(Error::Codestream(
                    "does not start with the `SOC` marker".into()
                )),
                "{bytes:02X?}"
            );
        }
    }
}

/// The incomplete messages say where the input ends.
#[test]
fn incomplete_before_the_main_header_says_where_the_input_ends() {
    let file = signed16();
    let cases: [(usize, &str); 7] = [
        (
            0,
            "the input ends before the codestream's `SOC` marker is whole",
        ),
        (5, "the input ends inside the JP2 signature box"),
        (12, "the input ends after the JP2 signature box"),
        (
            15,
            "the input ends inside a box header, after 3 of its 8 bytes",
        ),
        (
            50,
            "the input ends inside box `jp2h`, which is 45 bytes, after 18 of them",
        ),
        (77, "the input ends before the `jp2c` codestream box"),
        (
            86,
            "the input ends before the codestream's `SOC` marker is whole",
        ),
    ];
    for (length, message) in cases {
        assert_eq!(
            read_header(&file[..length]).map(|_| ()),
            Err(Error::Incomplete(message.into())),
            "cut at {length}"
        );
    }
}

/// The JP2 signature box (I.5.1).
const SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// `jp2_signed16.jp2`: the signature at 0, `ftyp` at 12 (20 bytes), `jp2h`
/// at 32 (45 bytes, holding `ihdr` at 40 and `colr` at 62) and `jp2c` at 77,
/// its codestream from 85.
fn signed16() -> Vec<u8> {
    let whole = std::fs::read(fixtures().join("jp2_signed16.jp2")).unwrap();
    assert_eq!(&whole[16..20], b"ftyp");
    assert_eq!(&whole[36..40], b"jp2h");
    assert_eq!(&whole[44..48], b"ihdr");
    assert_eq!(&whole[66..70], b"colr");
    assert_eq!(&whole[81..87], b"jp2c\xFF\x4F");
    whole
}

/// Damaged JP2 box headers and fields before the codestream, each with the
/// error the whole file gives today and the length of the shortest prefix
/// that holds the damage.
fn damaged_boxes() -> Vec<(&'static str, Vec<u8>, Error, usize)> {
    let whole = signed16();
    let file_format = |message: &str| Error::FileFormat(message.into());
    vec![
        (
            "signature contents wrong",
            edited(&whole, &[(11, 0x0B)]),
            file_format(
                "the file must open with the 12-byte JP2 signature box (`jP  ` holding \
                 0D 0A 87 0A)",
            ),
            12,
        ),
        (
            "signature length wrong",
            edited(&whole, &[(3, 0x0D)]),
            file_format(
                "the file must open with the 12-byte JP2 signature box (`jP  ` holding \
                 0D 0A 87 0A)",
            ),
            8,
        ),
        (
            "ftyp not second",
            edited(&whole, &[(19, b'q')]),
            file_format("the `ftyp` box must follow the signature, not `ftyq`"),
            20,
        ),
        (
            "ftyp length below 8",
            edited(&whole, &[(15, 3)]),
            file_format("box `ftyp` has length 3, shorter than its 8-byte header"),
            20,
        ),
        (
            "ftyp of 4 bytes",
            edited(&whole, &[(15, 12)]),
            file_format(
                "the `ftyp` box holds 4 bytes; it needs a brand, a version, and whole 4-byte \
                 compatibility codes",
            ),
            24,
        ),
        (
            "no jp2 in ftyp",
            edited(&whole, &[(23, b'x'), (31, b'x')]),
            Error::Unsupported(
                "file brand `jp2x` without `jp2 ` compatibility (a JPX, JPM or MJ2 file that \
                 does not claim to be readable as JP2)"
                    .into(),
            ),
            32,
        ),
        (
            "jp2h length below 8",
            edited(&whole, &[(35, 5)]),
            file_format("box `jp2h` has length 5, shorter than its 8-byte header"),
            40,
        ),
        (
            "jp2h XLBox below 16",
            {
                // `LBox` 1, then an `XLBox` of 15 over `ihdr`'s header.
                let mut bytes = edited(&whole, &[(35, 1)]);
                bytes[40..48].copy_from_slice(&15u64.to_be_bytes());
                bytes
            },
            file_format("box `jp2h` has `XLBox` 15, shorter than its 16-byte header"),
            48,
        ),
        (
            "ihdr past the end of jp2h",
            edited(&whole, &[(43, 0x30)]),
            file_format("box `ihdr` is 48 bytes but only 37 remain in the `jp2h` box"),
            77,
        ),
        (
            "ihdr with no components",
            edited(&whole, &[(57, 0)]),
            file_format("the `ihdr` box declares a 24x16 image of 0 components; none may be 0"),
            77,
        ),
        (
            "no jp2h before jp2c",
            edited(&whole, &[(39, b'i')]),
            file_format("the file has no `jp2h` header box before its `jp2c` codestream box"),
            85,
        ),
    ]
}

/// Damaging a complete box header or field before the codestream gives the
/// same error as before `Incomplete` reached the box layer, from
/// `read_header`, a strict decode and a partial one. So does every prefix
/// that holds the damage; a shorter one is `Incomplete`, or, under 8 bytes
/// that are not the start of the signature, `Codestream`, since such input
/// is not yet known to be JP2.
#[test]
fn a_damaged_box_before_the_codestream_keeps_its_error() {
    for (what, bytes, error, holds) in damaged_boxes() {
        for length in 0..=bytes.len() {
            let prefix = &bytes[..length];
            for result in [
                read_header(prefix).map(|_| ()),
                decode(prefix).map(|_| ()),
                decode_with(prefix, &lenient()).map(|_| ()),
            ] {
                if length >= holds {
                    assert_eq!(result, Err(error.clone()), "{what}, cut at {length}");
                } else if length < 8 && !SIGNATURE.starts_with(prefix) {
                    assert!(
                        matches!(result, Err(Error::Codestream(_))),
                        "{what}, cut at {length}: {result:?}"
                    );
                } else {
                    assert!(
                        matches!(result, Err(Error::Incomplete(_))),
                        "{what}, cut at {length}: {result:?}"
                    );
                }
            }
        }
    }
}

/// The fixture's main header: SIZ at 2, COD at 45, QCD at 59, COM at 77 and
/// the first SOT at 116.
fn latlon() -> Vec<u8> {
    let whole = std::fs::read(fixtures().join("jpeg2000_regular_latlon.j2k")).unwrap();
    assert_eq!(first_sot(&whole), Some(116));
    whole
}

/// `whole` with each `(offset, value)` written over it.
fn edited(whole: &[u8], edits: &[(usize, u8)]) -> Vec<u8> {
    let mut bytes = whole.to_vec();
    for &(at, value) in edits {
        assert_ne!(bytes[at], value, "the edit at {at} changes nothing");
        bytes[at] = value;
    }
    bytes
}

/// Damaged main headers, each with the length of the shortest prefix that
/// holds the damage: from there on, the segments the prefix holds in full
/// show it. The missing QCD is only known once the whole header is there.
fn damaged() -> Vec<(&'static str, Vec<u8>, usize)> {
    let whole = latlon();
    vec![
        ("SOC is not SOC", edited(&whole, &[(1, 0x4E)]), 2),
        ("Lsiz below 2", edited(&whole, &[(5, 1)]), 6),
        (
            "Lsiz one short, so the walk loses sync",
            edited(&whole, &[(5, 40)]),
            46,
        ),
        (
            "Csiz past the SIZ segment's end",
            edited(&whole, &[(41, 2)]),
            45,
        ),
        (
            "COD declares precinct sizes it does not carry",
            edited(&whole, &[(49, 0x01)]),
            59,
        ),
        (
            "COD's marker code is not a marker",
            edited(&whole, &[(45, 0x00)]),
            47,
        ),
        ("SOD in the main header", edited(&whole, &[(46, 0x93)]), 47),
        ("a second SIZ", edited(&whole, &[(46, 0x51)]), 59),
        (
            "SIZ after COD",
            [&whole[..2], &whole[45..59], &whole[2..45], &whole[59..]].concat(),
            16,
        ),
        (
            "QCD missing (its code made COM)",
            edited(&whole, &[(60, 0x64)]),
            118,
        ),
    ]
}

/// `whole` with `segment` inserted after its SIZ segment, which ends at 45.
fn after_siz(whole: &[u8], segment: &[u8]) -> Vec<u8> {
    [&whole[..45], segment, &whole[45..]].concat()
}

/// Main headers refused with a variant other than `Codestream`, each with the
/// length of the shortest prefix that holds the refused segment and the
/// variant the whole file gives. An `Unsupported`, `Marker` or `Limit` held
/// in full outranks the cut the way `Codestream` damage does.
#[allow(clippy::type_complexity)]
fn refused() -> Vec<(&'static str, Vec<u8>, usize, fn(&Error) -> bool)> {
    let whole = latlon();
    // The COD's style byte is at 57 and its wavelet at 58; the segment ends
    // at 59.
    vec![
        (
            "an HTJ2K code-block style in COD",
            edited(&whole, &[(57, 0x40)]),
            59,
            |e| matches!(e, Error::Unsupported(m) if m.contains("code-block style (HTJ2K")),
        ),
        (
            "a reserved wavelet in COD",
            edited(&whole, &[(58, 2)]),
            59,
            |e| matches!(e, Error::Marker(_)),
        ),
        (
            "a CAP that signals MULTIHT",
            after_siz(&whole, &[0xFF, 0x50, 0, 8, 0, 2, 0, 0, 0x20, 0x03]),
            55,
            |e| matches!(e, Error::Unsupported(m) if m.contains("MULTIHT")),
        ),
        (
            "a CAP one Ccap word short",
            after_siz(&whole, &[0xFF, 0x50, 0, 6, 0, 2, 0, 0]),
            53,
            |e| matches!(e, Error::Marker(m) if m.contains("`CAP`")),
        ),
    ]
}

/// Each whole file of [`refused`] gives its variant.
#[test]
fn a_refused_main_header_gives_its_variant() {
    for (what, bytes, _, expected) in refused() {
        let header = read_header(&bytes);
        assert!(header.as_ref().is_err_and(expected), "{what}: {header:?}");
    }
}

/// A whole file whose main header has a damaged field is `Codestream`, not
/// `Incomplete`: every byte of the header is there, so more bytes would not
/// help.
#[test]
fn a_damaged_main_header_is_codestream() {
    let mut messages = Vec::new();
    for (what, bytes, _) in damaged() {
        let header = read_header(&bytes);
        let Err(Error::Codestream(message)) = &header else {
            panic!("{what}: read_header gave {header:?}");
        };
        for options in [DecodeOptions::default(), lenient()] {
            let decoded = decode_with(&bytes, &options).map(|_| ());
            assert_eq!(decoded, Err(header.clone().unwrap_err()), "{what}");
        }
        messages.push(message.clone());
    }
    messages.sort();
    messages.dedup();
    assert!(
        messages.len() >= 9,
        "the cases are not distinct: {messages:?}"
    );
}

/// A prefix that holds a damaged segment in full is an error, not
/// `Incomplete`, from `read_header`, a strict decode and a partial one: the
/// segments the input holds are judged before the cut is reported. A shorter
/// prefix does not yet show it, and is `Incomplete` or `Codestream`.
///
/// Each case here damages one place only, so the prefix's error is exactly
/// the whole file's. That does not hold in general: a whole file whose header
/// is also broken later, where the walk fails (a non-marker, `SOD`, a length
/// below 2), reports that walk error before it judges any segment.
#[test]
fn a_cut_after_a_damaged_segment_is_an_error_not_incomplete() {
    let refused = refused()
        .into_iter()
        .map(|(what, bytes, holds, _)| (what, bytes, holds));
    for (what, bytes, holds) in damaged().into_iter().chain(refused) {
        let whole = read_header(&bytes).map(|_| ());
        assert!(whole.is_err(), "{what}");
        for length in 0..=bytes.len() {
            let prefix = &bytes[..length];
            let results = [
                read_header(prefix).map(|_| ()),
                decode(prefix).map(|_| ()),
                decode_with(prefix, &lenient()).map(|_| ()),
            ];
            for result in results {
                if length >= holds {
                    assert_eq!(result, whole, "{what}, cut at {length}");
                } else if length >= 2 {
                    // Under 2 bytes there is no SOC, which some whole-file
                    // messages ("truncated marker segment") share.
                    assert_ne!(result, whole, "{what}, cut at {length}");
                    assert!(
                        matches!(result, Err(Error::Incomplete(_) | Error::Codestream(_))),
                        "{what}, cut at {length}: {result:?}"
                    );
                }
            }
        }
    }
}

/// The message names where the input ended, after the variant's prefix.
#[test]
fn incomplete_displays_where_the_input_ends() {
    let whole = latlon();
    let error = read_header(&whole[..40]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "incomplete: the input ends inside the main header, in its `SIZ` (0xFF51) segment"
    );
    let error = read_header(&whole[..2]).unwrap_err();
    assert_eq!(
        error.to_string(),
        "incomplete: the input ends inside the main header, before its first `SOT` marker"
    );
}

/// An `ftyp` of length 0 runs to the end of the file and swallows the boxes
/// meant to follow it. The file is then complete as declared, so it and
/// every longer one is `FileFormat`, never `Incomplete`, whatever its length.
#[test]
fn an_ftyp_that_swallows_the_file_is_file_format() {
    let mut bytes = edited(&signed16(), &[(15, 0)]);
    // Pad to whole compatibility codes, so the `ftyp` itself reads.
    while (bytes.len() - 20) % 4 != 0 {
        bytes.push(0);
    }
    for extra in 0..16 {
        let file = [&bytes[..], &vec![0; extra]].concat();
        for result in [
            read_header(&file).map(|_| ()),
            decode(&file).map(|_| ()),
            decode_with(&file, &lenient()).map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(Error::FileFormat(_))),
                "{extra} bytes past the padded file: {result:?}"
            );
        }
    }
    // With whole compatibility codes, it is the swallowed `jp2h` and `jp2c`.
    assert_eq!(
        read_header(&bytes).map(|_| ()),
        Err(Error::FileFormat(
            "the file has no `jp2h` header box and no `jp2c` codestream box: a box of length 0 \
             (to the end of the file) swallowed the rest"
                .into()
        ))
    );
}
