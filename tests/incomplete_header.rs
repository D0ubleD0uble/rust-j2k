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
    if !cs.starts_with(&[0xFF, 0x4F]) {
        return None;
    }
    let mut at = 2;
    loop {
        let code = u16_at(cs, at)?;
        match code {
            0xFF90 => return Some(at),
            // The reserved range carries no length (A.1.3).
            0xFF30..=0xFF3F => at += 2,
            _ if code >> 8 == 0xFF => {
                let length = u16_at(cs, at + 2)?;
                if length < 2 {
                    return None;
                }
                at += 2 + length;
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

#[test]
fn every_cut_inside_the_main_header_is_incomplete() {
    let mut paths = Vec::new();
    files(&fixtures(), &mut paths);
    paths.sort();
    let (mut codestreams, mut jp2s) = (0, 0);
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        let name = path.strip_prefix(fixtures()).unwrap().display().to_string();
        if let Some(sot) = first_sot(&bytes) {
            assert_cuts_are_incomplete(&name, &bytes, 0, sot, true);
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
    eprintln!("cut {codestreams} codestreams and {jp2s} JP2 files");
    assert!(codestreams >= 100, "only {codestreams} codestreams cut");
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

/// A whole file whose main header has a damaged field is `Codestream`, not
/// `Incomplete`: every byte of the header is there, so more bytes would not
/// help. The fixture's main header is SIZ at 2, COD at 45, QCD at 59, COM at
/// 77 and the first SOT at 116.
#[test]
fn a_damaged_main_header_is_codestream() {
    let whole = std::fs::read(fixtures().join("jpeg2000_regular_latlon.j2k")).unwrap();
    assert_eq!(first_sot(&whole), Some(116));
    let cases: [(&str, &[(usize, u8)]); 9] = [
        ("SOC is not SOC", &[(1, 0x4E)]),
        ("Lsiz below 2", &[(5, 1)]),
        ("Lsiz one short, so the walk loses sync", &[(5, 40)]),
        ("Csiz past the SIZ segment's end", &[(41, 2)]),
        (
            "COD declares precinct sizes it does not carry",
            &[(49, 0x01)],
        ),
        ("COD's marker code is not a marker", &[(45, 0x00)]),
        ("SOD in the main header", &[(46, 0x93)]),
        ("a second SIZ", &[(46, 0x51)]),
        ("QCD missing (its code made COM)", &[(60, 0x64)]),
    ];
    let mut messages = Vec::new();
    for (what, edits) in cases {
        let mut bytes = whole.clone();
        for &(at, value) in edits {
            assert_ne!(bytes[at], value, "{what}: the edit changes nothing");
            bytes[at] = value;
        }
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
        messages.len() >= 8,
        "the cases are not distinct: {messages:?}"
    );
}
