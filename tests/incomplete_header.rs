//! Input that ends inside the main header is `Error::Incomplete`, apart from
//! a main header that is all there but damaged, which stays
//! `Error::Codestream`.
//!
//! Each committed codestream, and the codestream of each committed JP2 file,
//! is cut at every byte inside its main header: from just after the `SOC`
//! marker to just before the first `SOT` marker is whole. The header bounds
//! come from a marker walk written here (A.4, A.6), not from the decoder.

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

/// Every prefix of `bytes` that ends inside the codestream main header which
/// starts at `cs_at` and whose first `SOT` is at `cs_at + sot` is
/// `Incomplete`: from `read_header`, a strict decode and a partial one. With
/// `boundary`, the prefix that holds the whole `SOT` marker is not.
fn assert_cuts_are_incomplete(name: &str, bytes: &[u8], cs_at: usize, sot: usize, boundary: bool) {
    for length in cs_at + 2..cs_at + sot + 2 {
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

/// Input too short to hold the `SOC` marker is not recognizably JPEG 2000,
/// so it is `Codestream`, as empty input always was.
#[test]
fn input_shorter_than_soc_is_codestream() {
    for prefix in [&[][..], &[0xFF][..]] {
        assert!(matches!(read_header(prefix), Err(Error::Codestream(_))));
        assert!(matches!(decode(prefix), Err(Error::Codestream(_))));
        assert!(matches!(
            decode_with(prefix, &lenient()),
            Err(Error::Codestream(_))
        ));
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
