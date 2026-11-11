//! The `ftyp`, `ihdr`, `colr` and `rreq` fields on `Jp2Metadata`, through
//! `read_header` and `decode`.
//!
//! The file is the Part 4 `file5.jp2` (a `jpx `-branded file with `jp2 ` in
//! its compatibility list, an `rreq` box and two `colr` boxes), with its
//! `ftyp` minor version and `ihdr` flags patched here so they differ from the
//! defaults. The expected values are read off a box dump of the committed
//! file. None of these fields changes the samples.
//!
//! The box-layer edge cases no committed file has (a vendor feature in
//! `rreq`, a misplaced top-level `colr`, a second `jp2h`, a malformed first
//! `rreq`) are file5 patched by the edits in
//! `conformance/jp2/edge_cases.json`. The box walker
//! (`scripts/gen-jp2-box-snapshots.py`) writes both the edits and the fields
//! it reads from the patched bytes, so the expected values do not come from
//! this decoder.

mod support;

use std::path::PathBuf;

use rust_j2k::{ColourSpace, Jp2Metadata, decode, read_header};

/// `file5.jp2` with `ftyp`'s `MinV` set to 0x0102_0304 and `ihdr`'s `UnkC`
/// and `IPR` set to these values.
fn file5_with(unknown_colour_space: u8, ipr: u8) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance/jp2/files/file5.jp2");
    let mut bytes = std::fs::read(path).unwrap();
    // `ftyp` follows the 12-byte signature: its type at 16, `BR` at 20, then
    // `MinV` at 24.
    assert_eq!(&bytes[16..28], b"ftypjpx \0\0\0\0");
    bytes[24..28].copy_from_slice(&0x0102_0304u32.to_be_bytes());
    // jP (12), ftyp (32), rreq (24), then jp2h's header and ihdr's: the
    // `ihdr` payload starts at 84, and `BPC`, `C`, `UnkC`, `IPR` end it.
    assert_eq!(&bytes[80..84], b"ihdr");
    assert_eq!(&bytes[94..98], &[7, 7, 0, 0]);
    bytes[96] = unknown_colour_space;
    bytes[97] = ipr;
    bytes
}

/// Check every field the box dump gives, and the `ihdr` flags set by
/// [`file5_with`].
#[track_caller]
fn check(jp2: &Jp2Metadata, unknown_colour_space: u8, ipr: u8) {
    assert_eq!(jp2.file_type.brand, *b"jpx ");
    assert_eq!(jp2.file_type.minor_version, 0x0102_0304);
    // The encoder wrote `00 00 00 03` after `MinV`; by I.5.2's layout it is
    // the first compatibility code, and it is reported as stored.
    assert_eq!(
        jp2.file_type.compatibility,
        [[0, 0, 0, 3], *b"jp2 ", *b"jpx ", *b"jpxb"]
    );

    let header = jp2.image_header;
    assert_eq!(
        (header.compression, header.unknown_colour_space, header.ipr),
        (7, unknown_colour_space, ipr)
    );

    // `rreq`: ML 1, FUAM A0, DCM C0, standard features 5, 61 and 43 with
    // masks 80, 60 and 40, no vendor features.
    let rreq = jp2.reader_requirements.as_ref().expect("an rreq");
    assert_eq!(rreq.mask_length, 1);
    assert_eq!(rreq.fully_understand, [0xA0]);
    assert_eq!(rreq.decode_completely, [0xC0]);
    let features: Vec<_> = rreq
        .standard_features
        .iter()
        .map(|f| (f.feature, f.mask.clone()))
        .collect();
    assert_eq!(
        features,
        [(5, vec![0x80]), (61, vec![0x60]), (43, vec![0x40])]
    );
    assert!(rreq.vendor_features.is_empty());

    // The first `colr` is a 546-byte restricted ICC profile with `APPROX` 1,
    // and it is the one that sets the colour space. The second, enumerated
    // ROMM-RGB (21) with `PREC` 1 and `APPROX` 1, is reported and ignored.
    let [first, second] = &jp2.colour_specifications[..] else {
        panic!("{} colr boxes", jp2.colour_specifications.len());
    };
    assert_eq!(
        (first.method, first.precedence, first.approximation),
        (2, 0, 1)
    );
    assert_eq!(first.data.len(), 546);
    assert_eq!(jp2.colour, ColourSpace::RestrictedIcc(first.data.clone()));
    assert_eq!(
        (second.method, second.precedence, second.approximation),
        (1, 1, 1)
    );
    assert_eq!(second.data, [0, 0, 0, 21]);
}

#[test]
fn read_header_reports_the_ftyp_ihdr_colr_and_rreq_fields() {
    for (unknown_colour_space, ipr) in [(1, 0), (0, 1)] {
        let bytes = file5_with(unknown_colour_space, ipr);
        let jp2 = read_header(&bytes).unwrap().jp2.unwrap();
        check(&jp2, unknown_colour_space, ipr);
    }
}

#[test]
fn decode_reports_the_ftyp_ihdr_colr_and_rreq_fields() {
    for (unknown_colour_space, ipr) in [(1, 0), (0, 1)] {
        let bytes = file5_with(unknown_colour_space, ipr);
        let jp2 = decode(&bytes).unwrap().jp2.unwrap();
        check(&jp2, unknown_colour_space, ipr);
    }
}

/// `conformance/jp2/edge_cases.json`: each case's edits to file5 and the
/// fields the walker read from the patched bytes.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeCases {
    #[serde(rename = "_comment")]
    _comment: String,
    cases: std::collections::BTreeMap<String, EdgeCase>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeCase {
    #[serde(rename = "note")]
    _note: String,
    edits: Vec<Edit>,
    boxes: support::Jp2Boxes,
}

/// Remove `remove` bytes at offset `at`, then insert `insert` (hex) there.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    at: usize,
    remove: usize,
    insert: String,
}

fn edge_cases() -> EdgeCases {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance/jp2/edge_cases.json");
    let text = std::fs::read_to_string(path).expect("read edge_cases.json");
    serde_json::from_str(&text).expect("parse edge_cases.json")
}

/// Patch file5 with one case's edits, check that `read_header` and `decode`
/// both report the walker's fields, and return them.
fn edge_case(name: &str) -> support::Jp2Boxes {
    let case = edge_cases()
        .cases
        .remove(name)
        .unwrap_or_else(|| panic!("{name}: no case in edge_cases.json"));
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/conformance/jp2/files/file5.jp2");
    let mut bytes = std::fs::read(path).unwrap();
    for edit in &case.edits {
        let insert: Vec<u8> = (0..edit.insert.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&edit.insert[i..i + 2], 16).unwrap())
            .collect();
        bytes.splice(edit.at..edit.at + edit.remove, insert);
    }
    let header = read_header(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let from_header = support::Jp2Boxes::of(header.jp2.as_ref().expect("JP2 metadata"));
    assert_eq!(from_header, case.boxes, "{name}: read_header");
    let image = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let from_decode = support::Jp2Boxes::of(image.jp2.as_ref().expect("JP2 metadata"));
    assert_eq!(from_decode, case.boxes, "{name}: decode");
    case.boxes
}

/// The case list is what the walker writes, so a case cannot pass by being
/// left out.
#[test]
fn every_edge_case_has_a_test() {
    let names: Vec<String> = edge_cases().cases.into_keys().collect();
    assert_eq!(
        names,
        [
            "colr_after_jp2c",
            "colr_before_jp2h",
            "colr_between_jp2h_and_jp2c",
            "malformed_rreq_first",
            "rreq_vendor_features",
            "second_jp2h",
        ]
    );
}

/// The `METH` and `PREC` of each counted `colr`, in order.
fn colours(boxes: &support::Jp2Boxes) -> Vec<(u8, u8)> {
    boxes
        .colour_specifications
        .iter()
        .map(|c| (c.method, c.precedence))
        .collect()
}

/// An `rreq` with two vendor features after its standard one (and `ML` 2,
/// so every mask is two bytes) reports both, in box order.
#[test]
fn an_rreq_vendor_feature_is_reported() {
    let boxes = edge_case("rreq_vendor_features");
    let rreq = boxes.reader_requirements.expect("an rreq");
    assert_eq!(rreq.mask_length, 2);
    assert_eq!(rreq.standard_features, [(5, "8000".to_string())]);
    let vendor = [
        ("000102030405060708090a0b0c0d0e0f", "2000"),
        ("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff", "1000"),
    ]
    .map(|(id, mask)| (id.to_string(), mask.to_string()));
    assert_eq!(rreq.vendor_features, vendor);
}

/// A top-level `colr` before the first `jp2h` is not counted: file5's two
/// remain, the ICC profile first.
#[test]
fn a_colr_before_the_first_jp2h_is_not_counted() {
    let boxes = edge_case("colr_before_jp2h");
    assert_eq!(colours(&boxes), [(2, 0), (1, 1)]);
}

/// A top-level `colr` after the `jp2c` is not counted.
#[test]
fn a_colr_after_the_jp2c_is_not_counted() {
    let boxes = edge_case("colr_after_jp2c");
    assert_eq!(colours(&boxes), [(2, 0), (1, 1)]);
}

/// A top-level `colr` between the `jp2h` and the `jp2c` is read as if it were
/// inside the `jp2h`, so it is counted, after the two there.
#[test]
fn a_colr_between_the_jp2h_and_the_jp2c_is_counted() {
    let boxes = edge_case("colr_between_jp2h_and_jp2c");
    assert_eq!(colours(&boxes), [(2, 0), (1, 1), (1, 0)]);
}

/// A second `jp2h` is merged into the first: its `colr` is counted after the
/// first's two, and its `ihdr` (with `UnkC` and `IPR` set) is ignored.
#[test]
fn a_second_jp2h_adds_its_colr_and_not_its_ihdr() {
    let boxes = edge_case("second_jp2h");
    assert_eq!(colours(&boxes), [(2, 0), (1, 1), (1, 2)]);
    let header = &boxes.image_header;
    assert_eq!((header.unknown_colour_space, header.ipr), (0, 0));
}

/// The first top-level `rreq` is the one that counts, even when it is cut
/// short and a well-formed one follows: there is none.
#[test]
fn a_malformed_first_rreq_hides_a_later_one() {
    let boxes = edge_case("malformed_rreq_first");
    assert_eq!(boxes.reader_requirements, None);
}
