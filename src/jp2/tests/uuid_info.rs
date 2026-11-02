//! The intellectual property (`jp2i`) and UUID info (`uinf`) boxes
//! (ISO/IEC 15444-1 I.6 and I.7.3). OpenJPEG skips both, so the samples never
//! change; the expected metadata is read off the bytes built here. A
//! malformed `uinf` is skipped as a whole, as OpenJPEG skips every `uinf`
//! (owner decision, 2026-11-01; `docs/correctness.md`).

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

/// A well-formed `ulst` and `url ` (no UUIDs, empty location), for the box
/// faults to follow.
fn good_children() -> Vec<u8> {
    [ulst(0, &[]), url(0, [0; 3], b"")].concat()
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

/// A `uinf` holding `children` (whole boxes, or any bytes) is skipped: with
/// it before the codestream and again after it, each next to a well-formed
/// `uinf`, the file decodes and reads exactly as it does without it, and
/// `uuid_info` holds only the two good boxes.
#[track_caller]
fn skipped(children: &[u8]) {
    let bad = bx(b"uinf", children);
    let around = |extra: &[Vec<u8>]| {
        let mut boxes = vec![jp2_ftyp(), good_jp2h(), good_uinf()];
        boxes.extend_from_slice(extra);
        boxes.extend([jp2c(), good_uinf()]);
        boxes.extend_from_slice(extra);
        file(&boxes)
    };
    let without = metadata_of(&around(&[]));
    let with_bad = metadata_of(&around(&[bad]));
    assert_eq!(with_bad, without);
    assert_eq!(with_bad.uuid_info.len(), 2);
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
/// `jp2h` is not top-level and is not read.
#[test]
fn uuid_info_boxes_are_returned_in_file_order() {
    let (a, b, c) = ([0x11; 16], [0x22; 16], core::array::from_fn(|i| i as u8));
    let bytes = file(&[
        jp2_ftyp(),
        uinf(&[ulst(1, &[a]), url(0, [0; 3], b"http://x/a\0")]),
        jp2h(&[good_ihdr(), colr(), good_uinf()]),
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

/// The children may come in either order: Figure I.20 shows `ulst` first,
/// but I.7.3 does not require it.
#[test]
fn a_uinf_with_its_url_before_its_ulst_is_read() {
    let metadata = metadata_of(&with(uinf(&[url(0, [0; 3], b"x\0"), ulst(1, &[[9; 16]])])));
    assert_eq!(metadata.uuid_info.len(), 1);
    assert_eq!(metadata.uuid_info[0].ids, [[9; 16]]);
    assert_eq!(metadata.uuid_info[0].location_str(), Some("x"));
}

/// The 64-bit length form inside a `uinf` is read like any other.
#[test]
fn a_uinf_child_with_a_64_bit_length_is_read() {
    let metadata = metadata_of(&with(uinf(&[
        xl_box(b"ulst", &[0, 0]),
        xl_box(b"url ", b"\0\0\0\0x"),
    ])));
    assert_eq!(metadata.uuid_info[0].location, b"x");
}

#[test]
fn a_uinf_without_a_ulst_is_skipped() {
    skipped(&url(0, [0; 3], b"x\0"));
}

#[test]
fn a_uinf_with_two_ulst_boxes_is_skipped() {
    skipped(&[ulst(0, &[]), ulst(0, &[]), url(0, [0; 3], b"x\0")].concat());
}

#[test]
fn a_uinf_without_a_url_is_skipped() {
    skipped(&ulst(0, &[]));
}

#[test]
fn a_uinf_with_two_url_boxes_is_skipped() {
    skipped(&[url(0, [0; 3], b"x\0"), ulst(0, &[]), url(0, [0; 3], b"y\0")].concat());
}

#[test]
fn an_empty_uinf_is_skipped() {
    skipped(&[]);
}

#[test]
fn a_ulst_too_short_for_its_count_is_skipped() {
    for payload in [&[][..], &[0]] {
        skipped(&[bx(b"ulst", payload), url(0, [0; 3], b"x\0")].concat());
    }
}

/// `NU` UUIDs, no more and no fewer.
#[test]
fn a_ulst_whose_count_does_not_match_its_length_is_skipped() {
    for list in [
        bx(b"ulst", &[&[0, 2][..], &[1; 16]].concat()),
        bx(b"ulst", &[&[0, 1][..], &[1; 17]].concat()),
        bx(b"ulst", &[0, 0, 9]),
        bx(b"ulst", &[0xFF, 0xFF]),
    ] {
        skipped(&[list, url(0, [0; 3], b"x\0")].concat());
    }
}

#[test]
fn a_url_shorter_than_its_version_and_flags_is_skipped() {
    for payload in [&[][..], &[0, 0, 0]] {
        skipped(&[ulst(0, &[]), bx(b"url ", payload)].concat());
    }
}

/// The box-level faults inside a `uinf` skip the whole box too: nothing is
/// reported for the well-formed `ulst` and `url ` read before the fault.
#[test]
fn a_uinf_child_overrunning_the_uinf_is_skipped() {
    skipped(&[good_children(), header_only(100, b"free")].concat());
}

#[test]
fn a_uinf_child_of_length_0_is_skipped() {
    skipped(&[good_children(), open_box(b"free", b"")].concat());
}

#[test]
fn a_uinf_child_shorter_than_its_header_is_skipped() {
    skipped(&[good_children(), header_only(5, b"free")].concat());
}

#[test]
fn a_uinf_child_with_an_xlbox_below_16_is_skipped() {
    skipped(&[good_children(), xl_header_only(15, b"free")].concat());
}

#[test]
fn a_uinf_child_with_the_largest_xlbox_is_skipped() {
    skipped(&[good_children(), xl_header_only(u64::MAX, b"free")].concat());
}

#[test]
fn trailing_bytes_after_the_last_uinf_child_are_skipped() {
    for trailing in 1..8 {
        skipped(&[good_children(), vec![0; trailing]].concat());
    }
}

/// The `uinf` boxes after the codestream are read as those before it are,
/// and skipped the same way.
#[test]
fn a_uinf_after_the_codestream_is_read_or_skipped() {
    let bytes = [minimal(), uinf(&[ulst(1, &[]), url(0, [0; 3], b"")])].concat();
    assert!(metadata_of(&bytes).uuid_info.is_empty());
    let bytes = [minimal(), good_uinf()].concat();
    assert_eq!(metadata_of(&bytes).uuid_info.len(), 1);
}

/// Any bytes as a `uinf`'s contents: the file decodes to the same samples
/// and metadata as without the box, apart from `uuid_info`, which gains at
/// most that one box, and `read_header` agrees. The contents are random
/// bytes and random edits of a well-formed `uinf`, from a fixed seed.
#[test]
fn any_uinf_contents_leave_the_decode_unchanged() {
    const ROUNDS: usize = 800;
    let without = decode(&with(Vec::new())).unwrap();
    let good = [ulst(2, &[[1; 16], [2; 16]]), url(0, [0; 3], b"http://x\0")].concat();
    // xorshift64, so the inputs are the same on every run.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut read = 0;
    for round in 0..ROUNDS {
        let payload: Vec<u8> = if round % 2 == 0 {
            let len = (next() % 80) as usize;
            (0..len).map(|_| next() as u8).collect()
        } else {
            let mut edited = good.clone();
            for _ in 0..1 + next() % 3 {
                if edited.is_empty() {
                    break;
                }
                let at = (next() as usize) % edited.len();
                match next() % 3 {
                    0 => edited[at] = next() as u8,
                    1 => edited.truncate(at),
                    _ => edited.insert(at, next() as u8),
                }
            }
            edited
        };
        let bytes = with(bx(b"uinf", &payload));
        let image = decode(&bytes).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_eq!(image.components, without.components, "round {round}");
        let metadata = image.jp2.unwrap();
        assert_eq!(
            read_header(&bytes).map(|h| h.jp2),
            Ok(Some(metadata.clone())),
            "round {round}"
        );
        assert!(metadata.uuid_info.len() <= 1, "round {round}");
        read += metadata.uuid_info.len();
        let mut rest = metadata;
        rest.uuid_info.clear();
        assert_eq!(Some(rest), without.jp2, "round {round}");
    }
    // Some edits leave the box well formed, so both paths are taken.
    assert!(read > 0 && read < ROUNDS, "{read} of {ROUNDS} read");
}
