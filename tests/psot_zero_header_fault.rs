//! A fault in the header of a last tile-part whose `Psot` is 0, with
//! `strict` off.
//!
//! `Psot` 0 means the tile-part runs to the end of the codestream, so a
//! header the input holds in full is not cut short: a fault in it is refused
//! as with `strict` on. Only a header whose marker segments run to the end of
//! the input before they reach `SOD` can be the input cut short, and that
//! still decodes, with the loss in the report.
//!
//! Each input is `tiles_multipart_lossless.j2k` with `Psot` 0 written into
//! its last tile-part (tile 8, part 2 of 3, the 27th in the codestream) and
//! bytes put in front of that tile-part's `SOD`. OpenJPEG 2.5.0
//! (`opj_decompress -allow-partial`) refuses every one of them: "Not sure how
//! that happened." for `FF7F`, "Marker size inconsistent with stream length"
//! for `FF30` and the two over-long lengths, and "Stream too short" for the
//! cut headers.

use rust_j2k::{DecodeOptions, IncrementalDecoder, Refined, ReportEntry, decode_with};

/// The `SOT` offsets of a codestream, by the `Psot` chain.
fn sots(bytes: &[u8]) -> Vec<usize> {
    let mut at = bytes.windows(2).position(|w| w == [0xFF, 0x90]).unwrap();
    let mut found = Vec::new();
    while at + 12 <= bytes.len() && bytes[at..at + 2] == [0xFF, 0x90] {
        found.push(at);
        let psot = u32::from_be_bytes(bytes[at + 6..at + 10].try_into().unwrap()) as usize;
        if psot == 0 {
            break;
        }
        at += psot;
    }
    found
}

/// The fixture with `Psot` 0 in its last tile-part and `header` in front of
/// that tile-part's `SOD`, and the offset of that tile-part's `SOT`.
fn last_part_with(header: &[u8]) -> (Vec<u8>, usize) {
    let base = std::fs::read("tests/fixtures/tiles_multipart_lossless.j2k").unwrap();
    let parts = sots(&base);
    assert_eq!(parts.len(), 27);
    let at = parts[26];
    // Tile 8, part 2 of 3, its header `SOD` alone.
    assert_eq!(base[at + 4..at + 6], [0, 8]);
    assert_eq!(base[at + 10..at + 12], [2, 3]);
    assert_eq!(base[at + 12..at + 14], [0xFF, 0x93]);
    let mut bytes = base[..at + 12].to_vec();
    bytes[at + 6..at + 10].fill(0);
    bytes.extend_from_slice(header);
    bytes.extend_from_slice(&base[at + 12..]);
    (bytes, at)
}

fn options(strict: bool) -> DecodeOptions {
    DecodeOptions::default().with_strict(strict)
}

/// The decode with `strict` off gives the error the decode with `strict`
/// on does, and the two tile-part walks agree.
fn assert_refused_as_strict(name: &str, bytes: &[u8], message: &str) {
    rust_j2k::fuzz::tile_parts_walks_agree(bytes);
    let strict = decode_with(bytes, &options(true)).expect_err(name);
    assert!(strict.to_string().contains(message), "{name}: {strict}");
    let lenient = decode_with(bytes, &options(false)).expect_err(name);
    assert_eq!(format!("{lenient:?}"), format!("{strict:?}"), "{name}");
}

/// The repros of the issue whose header the input holds in full: an
/// unrecognized marker, a reserved one, and a segment whose length runs
/// past `SOD` into the packet data.
#[test]
fn a_fault_in_a_whole_psot_zero_header_is_refused() {
    for (name, header, message) in [
        (
            "FF7F",
            &[0xFF, 0x7F, 0, 2][..],
            "unrecognized marker 0xFF7F in a tile-part header",
        ),
        (
            "FF30",
            &[0xFF, 0x30],
            "reserved marker 0xFF30 in a tile-part header",
        ),
        (
            "a COM over SOD",
            &[0xFF, 0x64, 0, 4],
            "expected a marker in a tile-part header",
        ),
        ("an empty COD", &[0xFF, 0x52, 0, 2], "truncated"),
    ] {
        assert_refused_as_strict(name, &last_part_with(header).0, message);
    }
}

/// The decode with `strict` off is `Ok`, and its report says the codestream
/// ended early and tile 8 has two of its three tile-parts: the one lost is
/// part 2, the last.
fn assert_read_as_cut(name: &str, bytes: &[u8]) {
    rust_j2k::fuzz::tile_parts_walks_agree(bytes);
    assert!(decode_with(bytes, &options(true)).is_err(), "{name}");
    let image = decode_with(bytes, &options(false)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let entries = &image.report.entries;
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, ReportEntry::MissingEoc { .. })),
        "{name}: {entries:?}"
    );
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            ReportEntry::TilePartCountMismatch {
                tile: 8,
                declared: 3,
                found: 2,
                ..
            }
        )),
        "{name}: {entries:?}"
    );
}

/// A header whose markers run to the end of the input before `SOD` may be
/// the input cut short, so it decodes as one: a length that runs past the
/// end of the input (the issue's third repro, `FF7F 7FF0`) reads as the
/// input cut inside that segment, as the real cuts here do.
#[test]
fn a_psot_zero_header_the_input_ends_inside_is_a_cut() {
    assert_read_as_cut("FF7F 7FF0", &last_part_with(&[0xFF, 0x7F, 0x7F, 0xF0]).0);
    let com: Vec<u8> = [&[0xFF, 0x64, 0, 16][..], &[0; 14]].concat();
    let (bytes, at) = last_part_with(&com);
    for keep in [0, 1, 2, 3, 6, com.len(), com.len() + 1] {
        assert_read_as_cut(&format!("cut {keep}"), &bytes[..at + 12 + keep]);
    }
}

/// Every prefix of the `FF7F` file decodes with `strict` off until it holds
/// the whole header, through `SOD`, and gives the strict error from then
/// on, so an `IncrementalDecoder` waits through the header and refuses only
/// once the bytes show the fault.
#[test]
fn a_psot_zero_header_fault_is_refused_only_once_its_header_is_in() {
    let (bytes, at) = last_part_with(&[0xFF, 0x7F, 0, 2]);
    let whole = at + 12 + 6;
    let strict = format!("{:?}", decode_with(&bytes, &options(true)).unwrap_err());
    for length in at..=bytes.len() {
        let decoded = decode_with(&bytes[..length], &options(false));
        if length < whole {
            assert!(decoded.is_ok(), "{length}: {decoded:?}");
        } else {
            assert_eq!(format!("{:?}", decoded.unwrap_err()), strict, "{length}");
        }
    }
    let mut decoder = IncrementalDecoder::new(options(false)).unwrap();
    for byte in &bytes {
        decoder.feed(std::slice::from_ref(byte));
        match decoder.refine() {
            Ok(Refined::Image(_) | Refined::Waiting(_)) => {}
            Ok(other) => panic!("{other:?}"),
            Err(error) => {
                assert!(decoder.bytes_fed() >= whole, "{}", decoder.bytes_fed());
                assert_eq!(format!("{error:?}"), strict);
            }
        }
    }
    assert_eq!(format!("{:?}", decoder.finish().unwrap_err()), strict);
}
