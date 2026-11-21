//! A fault in the header of a tile-part that runs to the end of the input,
//! with `strict` off: a last tile-part whose `Psot` is 0, or one whose
//! `Psot` runs past the end.
//!
//! A fault whose marker segment the input holds in full is the same in any
//! longer input, so it is refused as with `strict` on. An unrecognized, Part
//! 2 or reserved marker is judged by the markers after it, so it is refused
//! once those reach `SOD` (or break). Otherwise the fault may be the input
//! cut short, and the file still decodes, with the loss in the report.
//!
//! Each input is `tiles_multipart_lossless.j2k` with bytes put in front of a
//! tile-part's `SOD`: its last tile-part (tile 8, part 2 of 3, the 27th in
//! the codestream) with `Psot` 0 written into it, or its sixth (tile 1, part
//! 2 of 3) with its `Psot` raised to match, then cut. OpenJPEG 2.5.0
//! (`opj_decompress -allow-partial`) refuses every one of them: "Not sure how
//! that happened." for `FF7F`, "Marker size inconsistent with stream length"
//! for `FF30` and the two over-long lengths, "Inconsistent marker size" for a
//! length of 1, "Error reading COD marker", "Error reading PPT marker" and
//! "Error reading MCC marker", "Marker is not compliant with its position"
//! for `SOP` and `SIZ` (each also cut right after the marker), and "Stream
//! too short" for the cut headers. Reading a cut header as the input cut
//! short, with the loss reported, is how this decoder already treated a
//! tile-part cut inside its header (see `docs/openjpeg-departures.md`).

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
        ("a length below 2", &[0xFF, 0x64, 0, 1], "length below 2"),
    ] {
        assert_refused_as_strict(name, &last_part_with(header).0, message);
    }
}

/// The decode with `strict` off is `Ok`, and its report names the lost
/// tile-part, tile 8 part 2, as `TruncatedTilePart` with `declared` 0 (its
/// `Psot`), says tile 8 has two of its three tile-parts, and gives
/// `MissingEoc` only when the input does not end in `FF D9`.
fn assert_read_as_cut(name: &str, bytes: &[u8]) {
    rust_j2k::fuzz::tile_parts_walks_agree(bytes);
    assert!(decode_with(bytes, &options(true)).is_err(), "{name}");
    let image = decode_with(bytes, &options(false)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let entries = &image.report.entries;
    let at = sots(bytes)[26];
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            ReportEntry::TruncatedTilePart { tile: 8, tile_part: 2, declared: 0, present, .. }
                if *present == (bytes.len() - at) as u64
        )),
        "{name}: {entries:?}"
    );
    let named = entries
        .iter()
        .find(|entry| matches!(entry, ReportEntry::TruncatedTilePart { .. }))
        .unwrap();
    assert!(
        named.to_string().contains("is cut short in its header"),
        "{name}: {named}"
    );
    assert_eq!(
        entries
            .iter()
            .any(|entry| matches!(entry, ReportEntry::MissingEoc { .. })),
        !bytes.ends_with(&[0xFF, 0xD9]),
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
/// input cut inside that segment, as the real cuts here do. That file ends
/// in its `EOC`, so the report has no `MissingEoc`.
#[test]
fn a_psot_zero_header_the_input_ends_inside_is_a_cut() {
    let (long, _) = last_part_with(&[0xFF, 0x7F, 0x7F, 0xF0]);
    assert!(long.ends_with(&[0xFF, 0xD9]));
    assert_read_as_cut("FF7F 7FF0", &long);
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

/// The fixture with `header` in front of the `SOD` of its sixth tile-part
/// (tile 1, part 2 of 3, its `SOT` at 1277), its `Psot` raised to match,
/// and the offset of that tile-part's `SOT` and its end.
fn sixth_part_with(header: &[u8]) -> (Vec<u8>, usize, usize) {
    let base = std::fs::read("tests/fixtures/tiles_multipart_lossless.j2k").unwrap();
    let at = sots(&base)[5];
    assert_eq!(at, 1277);
    assert_eq!(base[at + 4..at + 6], [0, 1]);
    assert_eq!(base[at + 10..at + 12], [2, 3]);
    assert_eq!(base[at + 12..at + 14], [0xFF, 0x93]);
    let psot = u32::from_be_bytes(base[at + 6..at + 10].try_into().unwrap());
    let psot = psot + u32::try_from(header.len()).unwrap();
    let mut bytes = base[..at + 12].to_vec();
    bytes[at + 6..at + 10].copy_from_slice(&psot.to_be_bytes());
    bytes.extend_from_slice(header);
    bytes.extend_from_slice(&base[at + 12..]);
    (bytes, at, at + psot as usize)
}

/// The same holds for a tile-part whose `Psot` runs past the end of the
/// input: an `MCC` in a header the input holds in full is refused, cut one
/// byte short of the tile-part's end (1914 bytes) as at its end (1915).
/// OpenJPEG 2.5.0 refuses both with "Error reading MCC marker".
#[test]
fn a_fault_in_a_whole_header_of_a_cut_tile_part_is_refused() {
    let (bytes, _, end) = sixth_part_with(&[0xFF, 0x75, 0, 4, 0, 0]);
    assert_eq!(end, 1915);
    for length in [end - 1, end] {
        assert_refused_as_strict(
            &format!("MCC, {length} bytes"),
            &bytes[..length],
            "`MCC` (0xFF75) is a JPEG 2000 Part 2 marker",
        );
    }
}

/// A cut tile-part whose header the input ends inside still decodes, and
/// the report names it as `TruncatedTilePart`.
#[test]
fn a_cut_tile_part_whose_header_the_input_ends_inside_is_a_cut() {
    let com: Vec<u8> = [&[0xFF, 0x64, 0, 16][..], &[0; 14]].concat();
    let (bytes, at, _) = sixth_part_with(&com);
    for keep in [0, 1, 2, 3, 6, com.len(), com.len() + 1] {
        let cut = &bytes[..at + 12 + keep];
        rust_j2k::fuzz::tile_parts_walks_agree(cut);
        let image = decode_with(cut, &options(false)).unwrap_or_else(|e| panic!("{keep}: {e}"));
        assert!(
            image.report.entries.iter().any(|entry| matches!(
                entry,
                ReportEntry::TruncatedTilePart {
                    tile: 1,
                    tile_part: 2,
                    ..
                }
            )),
            "{keep}: {:?}",
            image.report.entries
        );
    }
}

/// Every prefix of the `MCC` file, from that tile-part's `SOT`, decodes with
/// `strict` off until it holds the whole header and gives the strict error
/// from then on.
#[test]
fn a_cut_tile_part_header_fault_is_refused_only_once_its_header_is_in() {
    let (bytes, at, _) = sixth_part_with(&[0xFF, 0x75, 0, 4, 0, 0]);
    let whole = at + 12 + 8;
    let strict = format!("{:?}", decode_with(&bytes, &options(true)).unwrap_err());
    for length in at..=bytes.len() {
        let decoded = decode_with(&bytes[..length], &options(false));
        if length < whole {
            assert!(decoded.is_ok(), "{length}: {decoded:?}");
        } else {
            assert_eq!(format!("{:?}", decoded.unwrap_err()), strict, "{length}");
        }
    }
}

/// Faults whose marker segment is whole, each put in front of `SOD`: an
/// empty `COD`, a `COD` of length 3 then a whole `COM`, an empty `PPT`, a
/// `SOP`, a `SIZ`, and the `MCC` of the repro above. OpenJPEG 2.5.0
/// (`-allow-partial`) refuses each with the input cut right after it.
const WHOLE_SEGMENTS: [(&str, &[u8]); 6] = [
    ("an empty COD", &[0xFF, 0x52, 0, 2]),
    (
        "a short COD, then a COM",
        &[0xFF, 0x52, 0, 3, 0, 0xFF, 0x64, 0, 4, 0, 0],
    ),
    ("an empty PPT", &[0xFF, 0x61, 0, 2]),
    ("SOP", &[0xFF, 0x91, 0, 4, 0, 0]),
    ("SIZ", &[0xFF, 0x51, 0, 4, 0, 0]),
    ("MCC", &[0xFF, 0x75, 0, 4, 0, 0]),
];

/// The two tile-parts the faults go in: the last, with `Psot` 0, and the
/// sixth, with its `Psot` raised. Each is the file and its `SOT` offset.
fn both_parts(header: &[u8]) -> [(Vec<u8>, usize); 2] {
    let (sixth, at, _) = sixth_part_with(header);
    [last_part_with(header), (sixth, at)]
}

/// Cut right after such a segment, the input holds all the fault depends
/// on, so the decode with `strict` off gives the strict error of the whole
/// file. `MCC` is the exception: whether it is refused as a Part 2 marker
/// or as lost sync depends on the markers after it, so cut there it reads
/// as a cut.
#[test]
fn a_whole_faulty_segment_before_the_cut_is_refused() {
    for (name, header) in WHOLE_SEGMENTS {
        for (bytes, at) in both_parts(header) {
            let cut = &bytes[..at + 12 + header.len()];
            rust_j2k::fuzz::tile_parts_walks_agree(cut);
            let whole = decode_with(&bytes, &options(true)).expect_err(name);
            let found = decode_with(cut, &options(false));
            if name == "MCC" {
                assert!(found.is_ok(), "{name} at {at}: {found:?}");
            } else {
                assert_eq!(
                    format!("{:?}", found.expect_err(name)),
                    format!("{whole:?}"),
                    "{name} at {at}"
                );
            }
        }
    }
}

/// Every prefix from the tile-part's `SOT` on, with `strict` off, is `Ok`
/// until the first one that fails, and every longer one fails with the
/// same error, the whole file's. The walks agree on each. With `strict`
/// off the whole file gives the strict error, or decodes for `FF7F 7FF0`.
#[test]
fn a_prefix_moves_from_ok_to_one_final_error() {
    let mut cases: Vec<(&str, &[u8])> = WHOLE_SEGMENTS.to_vec();
    cases.extend([
        ("FF7F", &[0xFF, 0x7F, 0, 2][..]),
        ("FF30", &[0xFF, 0x30]),
        ("a COM over SOD", &[0xFF, 0x64, 0, 4]),
        ("a length below 2", &[0xFF, 0x64, 0, 1]),
        ("FF7F 7FF0", &[0xFF, 0x7F, 0x7F, 0xF0]),
    ]);
    for (name, header) in cases {
        for (bytes, at) in both_parts(header) {
            let last = decode_with(&bytes, &options(false))
                .err()
                .map(|e| format!("{e:?}"));
            let mut failed: Option<String> = None;
            for length in at..=bytes.len() {
                let prefix = &bytes[..length];
                if length < at + 40 {
                    rust_j2k::fuzz::tile_parts_walks_agree(prefix);
                }
                match (decode_with(prefix, &options(false)), &failed) {
                    (Ok(_), None) => {}
                    (Ok(_), Some(error)) => panic!("{name} at {at}, {length}: Ok after {error}"),
                    (Err(error), None) => failed = Some(format!("{error:?}")),
                    (Err(error), Some(first)) => {
                        assert_eq!(&format!("{error:?}"), first, "{name} at {at}, {length}")
                    }
                }
            }
            assert_eq!(failed, last, "{name} at {at}");
        }
    }
}
