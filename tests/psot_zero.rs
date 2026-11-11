//! A `Psot = 0` last tile-part, with and without the closing EOC.
//!
//! OpenJPEG 2.5.0 gives a `Psot = 0` tile-part everything up to the last two
//! bytes of the codestream. It does not check that those bytes are FF D9 (it
//! warns). A codestream whose EOC is missing therefore loses two data bytes
//! and fails in tile decoding. Every case here was run through
//! `opj_decompress` 2.5.0; see `tests/fixtures/psot0/README.md`.
//!
//! The `jp2_*.jp2` files wrap these codestreams in JP2 boxes, with boxes after
//! the codestream. When the `jp2c` box's length is right, the codestream ends
//! at the end of the box.

use std::path::PathBuf;

use rust_j2k::{Error, decode, read_header};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/psot0")
}

/// The samples of an 8-bit PGM (`P5`) from `opj_decompress`: the last
/// `width * height` bytes.
fn pgm_samples(name: &str) -> Vec<i32> {
    let bytes = std::fs::read(dir().join(name)).unwrap();
    bytes[bytes.len() - 48 * 40..]
        .iter()
        .map(|&b| i32::from(b))
        .collect()
}

fn decode_file(name: &str) -> rust_j2k::Result<rust_j2k::Image> {
    decode(&std::fs::read(dir().join(name)).unwrap())
}

/// Decodes, equal to the OpenJPEG output in `expected`.
fn assert_decodes(name: &str, expected: &str) {
    let image = decode_file(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.components.len(), 1, "{name}");
    assert_eq!(image.components[0].samples, pgm_samples(expected), "{name}");
}

#[test]
fn psot_zero_with_eoc_decodes() {
    assert_decodes("single_psot0_eoc.j2k", "single.expected.pgm");
    assert_decodes("tiled_psot0_eoc.j2k", "tiled.expected.pgm");
    assert_decodes("tileparts_psot0_eoc.j2k", "tileparts.expected.pgm");
}

/// Two junk bytes where the EOC should be: OpenJPEG warns and decodes.
#[test]
fn psot_zero_with_junk_in_place_of_eoc_decodes() {
    assert_decodes("single_psot0_junk_tail.j2k", "single.expected.pgm");
    assert_decodes("tiled_psot0_junk_tail.j2k", "tiled.expected.pgm");
}

/// No EOC: the last two data bytes are taken for it. OpenJPEG fails with
/// "segment too long" while decoding the tile; here the packet walk runs out
/// of data, a typed codestream error.
#[test]
fn psot_zero_without_eoc_fails_in_the_tile() {
    for name in [
        "single_psot0_no_eoc.j2k",
        "single_psot0_no_eoc_trunc100.j2k",
        "tiled_psot0_no_eoc.j2k",
        "tileparts_psot0_no_eoc.j2k",
    ] {
        match decode_file(name) {
            Err(Error::Codestream(m)) => {
                assert!(m.contains("overruns the tile-part"), "{name}: {m}");
            }
            other => panic!("{name}: expected a codestream error, got {other:?}"),
        }
    }
}

/// Fewer than two bytes after the tile-part header: OpenJPEG fails with
/// "Tile part length size inconsistent with stream length".
#[test]
fn psot_zero_with_no_room_for_the_tail_is_codestream() {
    assert!(matches!(
        decode_file("single_psot0_one_byte_after_sod.j2k"),
        Err(Error::Codestream(m)) if m.contains("fewer than two bytes")
    ));
}

/// Decodes like the OpenJPEG output in `expected`, and `read_header` reports
/// the same metadata as `decode`. Returns the metadata.
fn assert_jp2_decodes(name: &str, expected: &str) -> rust_j2k::Jp2Metadata {
    assert_decodes(name, expected);
    let bytes = std::fs::read(dir().join(name)).unwrap();
    let decoded = decode(&bytes).unwrap().jp2.expect("a JP2 file");
    let header = read_header(&bytes)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .jp2
        .expect("a JP2 file");
    assert_eq!(header, decoded, "{name}");
    decoded
}

/// Refused as `Codestream`, for the bytes past the last packet.
fn assert_leftover(name: &str) {
    match decode_file(name) {
        Err(Error::Codestream(m)) => {
            assert!(m.contains("left after the last packet"), "{name}: {m}");
        }
        other => panic!("{name}: expected a codestream error, got {other:?}"),
    }
}

/// A `jp2c` box of the right length, holding a codestream with no EOC, then
/// an `xml ` box. OpenJPEG 2.5.0 reads the tile-part on to the end of the file
/// and decodes; here the codestream ends at the end of the box, which gives
/// the same image. The `xml ` box after it is reported by both calls.
#[test]
fn a_psot_zero_codestream_ends_at_the_end_of_its_box() {
    let metadata = assert_jp2_decodes("jp2_p0noeoc_xml.jp2", "single.expected.pgm");
    assert_eq!(metadata.xml, vec![b"<a>hello</a>".to_vec()]);
}

/// The same with many tile-parts (the PR #182 shape), then an `xml ` and a
/// `uuid` box.
#[test]
fn a_psot_zero_tile_part_codestream_ends_at_the_end_of_its_box() {
    let metadata = assert_jp2_decodes(
        "jp2_p0noeoc_tileparts_xml_uuid.jp2",
        "tileparts.expected.pgm",
    );
    assert_eq!(metadata.xml, vec![b"<a>hello</a>".to_vec()]);
    assert_eq!(metadata.uuid.len(), 1);
    assert_eq!(metadata.uuid[0].id, std::array::from_fn(|i| i as u8));
    assert_eq!(metadata.uuid[0].payload, b"payload");
}

/// A `colr` box of two bytes after the box. OpenJPEG never reads it: its walk
/// after the decode starts at the end of the file, where the `Psot = 0`
/// tile-part left its stream. The file decodes.
#[test]
fn a_bad_box_after_a_psot_zero_codestream_box_is_not_read() {
    assert_jp2_decodes("jp2_p0noeoc_badcolr.jp2", "single.expected.pgm");
}

/// A `jp2c` length that ends inside the codestream (`LBox` 108 or 200), or
/// `LBox` 0. The box end is not where the codestream ends, so the tile-part
/// runs to the end of the file and the boxes after it are tile data past the
/// last packet, refused as for a raw codestream. OpenJPEG 2.5.0 decodes each
/// of these, ignoring those bytes (a known difference, `docs/correctness.md`).
#[test]
fn a_psot_zero_codestream_box_of_the_wrong_length_is_refused() {
    for name in [
        "jp2_short108_p0eoc_xml.jp2",
        "jp2_p0eoc_short_badcolr.jp2",
        "jp2_t2_last0_short.jp2",
        "jp2_lbox0_p0eoc_xml.jp2",
    ] {
        assert_leftover(name);
    }
}

/// A `jp2c` box that ends inside the main header, at a COM segment whose text
/// reads as a box header for the rest of the codestream. The bytes after the
/// box are a whole box, but the box does not hold the last tile-part's header,
/// so its end is not trusted: the codestream runs to the end of the file and
/// decodes, as in OpenJPEG 2.5.0.
#[test]
fn a_codestream_box_that_ends_before_the_last_tile_part_is_not_trusted() {
    assert_decodes("jp2_p0eoc_cut_in_com.jp2", "single.expected.pgm");
}

/// A right-sized `jp2c`, but the boxes after it do not reach the end of the
/// file (three bytes follow the `xml ` box): the box end is not trusted, and
/// the file is refused as before. OpenJPEG 2.5.0 decodes it.
#[test]
fn a_psot_zero_codestream_box_followed_by_broken_boxes_is_refused() {
    assert_leftover("jp2_p0noeoc_xml_junk.jp2");
}

/// A raw codestream has no box to end at: an `xml ` box appended to a
/// `Psot = 0` codestream with no EOC is tile data past the last packet.
/// OpenJPEG 2.5.0 warns ("Stream does not end with EOC") and decodes it.
#[test]
fn a_raw_psot_zero_codestream_with_bytes_after_its_packets_is_refused() {
    let mut bytes = std::fs::read(dir().join("single_psot0_no_eoc.j2k")).unwrap();
    bytes.extend_from_slice(b"\x00\x00\x00\x14xml <a>hello</a>");
    match decode(&bytes) {
        Err(Error::Codestream(m)) => assert!(m.contains("left after the last packet"), "{m}"),
        other => panic!("expected a codestream error, got {other:?}"),
    }
}
