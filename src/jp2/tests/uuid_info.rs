//! The intellectual property (`jp2i`) and UUID info (`uinf`) boxes
//! (ISO/IEC 15444-1 I.6 and I.7.3). OpenJPEG skips both, so the samples never
//! change; the expected metadata is read off the bytes built here, and the
//! rejects follow the standard (`docs/correctness.md`).

use super::*;
use crate::{UuidInfo, read_header_with};

/// A `ulst` box with this `NU` and these UUIDs.
fn ulst(count: u16, ids: &[[u8; 16]]) -> Vec<u8> {
    let mut payload = count.to_be_bytes().to_vec();
    for id in ids {
        payload.extend_from_slice(id);
    }
    bx(b"ulst", &payload)
}

/// A `url ` box: `VERS`, `FLAG`, then `LOC`.
fn url(version: u8, flags: [u8; 3], location: &[u8]) -> Vec<u8> {
    bx(b"url ", &[&[version][..], &flags, location].concat())
}

fn uinf(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"uinf", &children.concat())
}

/// A well-formed `uinf` with one UUID.
fn good_uinf() -> Vec<u8> {
    uinf(&[ulst(1, &[[7; 16]]), url(0, [0; 3], b"a\0")])
}

/// The metadata `decode`, `read_header` and `read_header_with` (under
/// `raw_channels`) all report for `bytes`, after checking they agree and the
/// samples are the bare codestream's.
#[track_caller]
fn metadata_of(bytes: &[u8]) -> Jp2Metadata {
    assert_decodes_like_raw(bytes);
    let metadata = decode(bytes).unwrap().jp2.unwrap();
    assert_eq!(read_header(bytes).unwrap().jp2.as_ref(), Some(&metadata));
    let raw = DecodeOptions::default().with_raw_channels(true);
    assert_eq!(
        read_header_with(bytes, &raw).unwrap().jp2.as_ref(),
        Some(&metadata)
    );
    metadata
}

/// `decode`, `read_header` and `read_header_with` all reject `bytes` with a
/// `FileFormat` error mentioning `fragment`.
#[track_caller]
fn rejected(bytes: &[u8], fragment: &str) {
    file_format(bytes, fragment);
    let header = read_header(bytes);
    assert!(
        matches!(&header, Err(Error::FileFormat(m)) if m.contains(fragment)),
        "read_header: {header:?}"
    );
    let raw = DecodeOptions::default().with_raw_channels(true);
    let header = read_header_with(bytes, &raw);
    assert!(
        matches!(&header, Err(Error::FileFormat(m)) if m.contains(fragment)),
        "read_header_with: {header:?}"
    );
}

/// A file whose only extra box is `extra`, between `jp2h` and `jp2c`.
fn with(extra: Vec<u8>) -> Vec<u8> {
    file(&[jp2_ftyp(), good_jp2h(), extra, jp2c()])
}

/// Every top-level `jp2i` box comes back verbatim in file order, before and
/// after the codestream, empty ones included. One inside `jp2h` or another
/// superbox is not top-level and is not collected.
#[test]
fn ipr_boxes_are_returned_in_file_order() {
    let bytes = file(&[
        jp2_ftyp(),
        bx(b"jp2i", b"<rights>first</rights>"),
        jp2h(&[good_ihdr(), colr(), bx(b"jp2i", b"in jp2h")]),
        bx(b"asoc", &bx(b"jp2i", b"nested")),
        xl_box(b"jp2i", b""),
        jp2c(),
        bx(b"jp2i", &[0xFF, 0, 0x80]),
    ]);
    let metadata = metadata_of(&bytes);
    assert_eq!(
        metadata.ipr,
        vec![
            b"<rights>first</rights>".to_vec(),
            Vec::new(),
            vec![0xFF, 0, 0x80]
        ]
    );
    assert!(metadata.uuid_info.is_empty());
    assert!(metadata.xml.is_empty() && metadata.uuid.is_empty());
}

/// Every top-level `uinf` comes back in file order, before and after the
/// codestream: one UUID with a NUL-terminated URL, two UUIDs with a URL that
/// has no NUL, and none with a version, flags and location kept as stored.
/// A box of another type inside a `uinf` is skipped, and a `uinf` inside
/// `jp2h` is not top-level and is not read, however malformed.
#[test]
fn uuid_info_boxes_are_returned_in_file_order() {
    let (a, b, c) = ([0x11; 16], [0x22; 16], core::array::from_fn(|i| i as u8));
    let bytes = file(&[
        jp2_ftyp(),
        uinf(&[ulst(1, &[a]), url(0, [0; 3], b"http://x/a\0")]),
        jp2h(&[good_ihdr(), colr(), bx(b"uinf", b"not read")]),
        uinf(&[
            bx(b"free", b"skipped"),
            ulst(2, &[b, c]),
            bx(b"free", b""),
            url(0, [0; 3], b"file:b.lbl"),
        ]),
        jp2c(),
        uinf(&[ulst(0, &[]), url(3, [1, 2, 3], &[0xC3, 0x28, 0])]),
    ]);
    let metadata = metadata_of(&bytes);
    let info = |ids: Vec<[u8; 16]>, version, flags, location: &[u8]| UuidInfo {
        ids,
        version,
        flags,
        location: location.to_vec(),
    };
    assert_eq!(
        metadata.uuid_info,
        vec![
            info(vec![a], 0, [0; 3], b"http://x/a\0"),
            info(vec![b, c], 0, [0; 3], b"file:b.lbl"),
            info(Vec::new(), 3, [1, 2, 3], &[0xC3, 0x28, 0]),
        ]
    );
    let locations: Vec<_> = metadata
        .uuid_info
        .iter()
        .map(UuidInfo::location_str)
        .collect();
    assert_eq!(locations, [Some("http://x/a"), Some("file:b.lbl"), None]);
    assert!(metadata.ipr.is_empty());
}

/// `location_str` drops one terminating NUL, and refuses a NUL before it or
/// bytes that are not UTF-8.
#[test]
fn the_location_reads_as_text_up_to_its_terminating_nul() {
    let location = |bytes: &[u8]| {
        let metadata = metadata_of(&with(uinf(&[ulst(0, &[]), url(0, [0; 3], bytes)])));
        let info = metadata.uuid_info.into_iter().next().unwrap();
        assert_eq!(info.location, bytes);
        info.location_str().map(str::to_owned)
    };
    assert_eq!(location(b""), Some(String::new()));
    assert_eq!(location(b"\0"), Some(String::new()));
    assert_eq!(location("é/ü\0".as_bytes()).as_deref(), Some("é/ü"));
    assert_eq!(location(b"a\0\0"), None);
    assert_eq!(location(b"a\0b"), None);
    assert_eq!(location(&[0xFF]), None);
}

#[test]
fn a_uinf_without_a_ulst_is_rejected() {
    rejected(
        &with(uinf(&[url(0, [0; 3], b"x\0")])),
        "a `uinf` box has no `ulst` box",
    );
}

#[test]
fn a_uinf_with_two_ulst_boxes_is_rejected() {
    rejected(
        &with(uinf(&[ulst(0, &[]), ulst(0, &[]), url(0, [0; 3], b"x\0")])),
        "more than one `ulst` box",
    );
}

#[test]
fn a_uinf_without_a_url_is_rejected() {
    rejected(&with(uinf(&[ulst(0, &[])])), "has no `url ` box");
}

#[test]
fn a_uinf_with_two_url_boxes_is_rejected() {
    rejected(
        &with(uinf(&[
            ulst(0, &[]),
            url(0, [0; 3], b"x\0"),
            url(0, [0; 3], b"y\0"),
        ])),
        "more than one `url ` box",
    );
}

/// Figure I.20 puts the `ulst` first.
#[test]
fn a_uinf_with_its_url_before_its_ulst_is_rejected() {
    rejected(
        &with(uinf(&[url(0, [0; 3], b"x\0"), ulst(0, &[])])),
        "`url ` box before its `ulst` box",
    );
}

#[test]
fn a_ulst_too_short_for_its_count_is_rejected() {
    for payload in [&[][..], &[0]] {
        rejected(
            &with(uinf(&[bx(b"ulst", payload), url(0, [0; 3], b"x\0")])),
            "too short for its 2-byte count",
        );
    }
}

/// `NU` UUIDs, no more and no fewer.
#[test]
fn a_ulst_whose_count_does_not_match_its_length_is_rejected() {
    let cases = [
        (bx(b"ulst", &[&[0, 2][..], &[1; 16]].concat()), "counts 2"),
        (bx(b"ulst", &[&[0, 1][..], &[1; 17]].concat()), "counts 1"),
        (bx(b"ulst", &[0, 0, 9]), "counts 0"),
        (bx(b"ulst", &[0xFF, 0xFF]), "counts 65535"),
    ];
    for (list, fragment) in cases {
        rejected(&with(uinf(&[list, url(0, [0; 3], b"x\0")])), fragment);
    }
}

#[test]
fn a_url_shorter_than_its_version_and_flags_is_rejected() {
    for payload in [&[][..], &[0, 0, 0]] {
        rejected(
            &with(uinf(&[ulst(0, &[]), bx(b"url ", payload)])),
            "it needs at least 4 (VERS and FLAG)",
        );
    }
}

/// A child box that runs past the end of the `uinf` is a box-layer fault.
#[test]
fn a_uinf_child_overrunning_the_uinf_is_rejected() {
    let overrun = [ulst(0, &[]), header_only(100, b"url ")].concat();
    rejected(&with(bx(b"uinf", &overrun)), "in a `uinf` box");
}

/// The `uinf` boxes after the codestream are read as those before it are.
#[test]
fn a_malformed_uinf_after_the_codestream_is_rejected() {
    let bytes = [minimal(), uinf(&[ulst(1, &[]), url(0, [0; 3], b"")])].concat();
    rejected(&bytes, "counts 1");
    // Well formed there, it is reported.
    let bytes = [minimal(), good_uinf()].concat();
    assert_eq!(metadata_of(&bytes).uuid_info.len(), 1);
}
