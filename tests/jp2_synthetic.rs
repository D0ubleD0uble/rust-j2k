//! Synthetic JP2 fixtures (`tests/fixtures/jp2_*.jp2`): lossless codestreams
//! wrapped in hand-built boxes by `scripts/gen-jp2-fixtures.py`, each with an
//! `<name>.expected.json` oracle from `opj_decompress` (provenance in the
//! fixtures README and in each snapshot). Every fixture must match its oracle
//! bit-exactly.

mod support;

use std::path::{Path, PathBuf};

use rust_j2k::{Association, ChannelDefinition, ChannelType, DecodeOptions, decode, decode_with};
use support::{Expected, compare};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every `jp2_*.jp2` in the fixtures directory, sorted.
fn fixtures() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir())
        .expect("read tests/fixtures")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "jp2")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("jp2_"))
        })
        .collect();
    found.sort();
    found
}

fn expected(path: &Path) -> Expected {
    let text = std::fs::read_to_string(path.with_extension("expected.json"))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Expected::from_json(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The contents of the first top-level `jp2c` box. These files are built with
/// 32-bit box lengths only.
fn first_jp2c(bytes: &[u8]) -> &[u8] {
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if &bytes[at + 4..at + 8] == b"jp2c" {
            return &bytes[at + 8..at + length];
        }
        at += length;
    }
    panic!("no jp2c box");
}

/// The set is what the generator writes, so a missing file cannot pass by
/// being skipped.
#[test]
fn the_fixture_set_is_complete() {
    let names: Vec<String> = fixtures()
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "jp2_cdef_alpha_first",
            "jp2_cdef_palette",
            "jp2_cdef_reversed",
            "jp2_cdef_rgba",
            "jp2_cdef_unspecified",
            "jp2_metadata_boxes",
            "jp2_palette_12bit",
            "jp2_palette_8bit",
            "jp2_palette_clamp",
            "jp2_palette_corrected",
            "jp2_palette_mixed",
            "jp2_palette_subsampled_big_first",
            "jp2_palette_subsampled_small_first",
            "jp2_signed16",
            "jp2_sycc_420_even",
            "jp2_sycc_420_odd",
            "jp2_sycc_422_even",
            "jp2_sycc_422_odd",
        ]
    );
}

#[test]
fn every_fixture_matches_its_oracle_bit_exactly() {
    for path in fixtures() {
        let name = path.display();
        let bytes = std::fs::read(&path).unwrap();
        let want = expected(&path);
        assert_eq!(want.tolerance, support::Tolerance::Exact, "{name}");
        assert!(
            want.jp2.is_some() || want.jp2_boxes.is_some(),
            "{name}: no box snapshot"
        );
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Err(mismatch) = compare(&image, &want) {
            panic!("{name}: {mismatch}");
        }
        // `read_header` reports the same metadata, checked the same way.
        let header = rust_j2k::read_header(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(header.jp2, image.jp2, "{name}: read_header");
        if let Some(want) = &want.jp2 {
            let got = header.jp2.as_ref().map(support::Jp2Snapshot::of);
            assert_eq!(got.as_ref(), Some(want), "{name}: read_header snapshot");
        }
        if let Some(want) = &want.jp2_boxes {
            let got = header.jp2.as_ref().map(support::Jp2Boxes::of);
            assert_eq!(got.as_ref(), Some(want), "{name}: read_header boxes");
        }
        let jp2 = image.jp2.as_ref().expect("JP2 metadata");
        let stem = path.file_stem().unwrap().to_string_lossy();
        let palette = stem.starts_with("jp2_palette") || stem == "jp2_cdef_palette";
        assert_eq!(jp2.palette_applied, palette, "{name}");
        assert_eq!(jp2.channels.len(), image.components.len(), "{name}");
    }
}

/// The `cdef` box of a file, as `(Cn, Typ, Asoc)` triples: a test-side read
/// of the first one inside `jp2h`.
fn cdef_entries(bytes: &[u8]) -> Vec<(u16, u16, u16)> {
    let boxes = |data: &[u8]| {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= data.len() {
            let length = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            out.push((
                data[at + 4..at + 8].to_vec(),
                data[at + 8..at + length].to_vec(),
            ));
            at += length;
        }
        out
    };
    let top = boxes(&bytes[12..]);
    let (_, jp2h) = top.iter().find(|(kind, _)| kind == b"jp2h").unwrap();
    let (_, cdef) = boxes(jp2h)
        .into_iter()
        .find(|(kind, _)| kind == b"cdef")
        .unwrap();
    let be = |at: usize| u16::from_be_bytes([cdef[at], cdef[at + 1]]);
    (0..usize::from(be(0)))
        .map(|i| (be(2 + 6 * i), be(4 + 6 * i), be(6 + 6 * i)))
        .collect()
}

/// Each `cdef` fixture's box, and the label `decode` must give each output
/// channel: the entry of the channel that ends up there. The samples' order is
/// checked against the oracle above.
#[test]
fn every_cdef_fixture_labels_its_channels_from_the_box() {
    use Association::{Colour, None as Unassociated, WholeImage};
    use ChannelType::{Colour as C, Opacity, Unspecified};
    let def = |kind, association| ChannelDefinition { kind, association };
    let colours = |n: u16| (1..=n).map(|c| def(C, Colour(c))).collect::<Vec<_>>();
    let rgba = [colours(3), vec![def(Opacity, WholeImage)]].concat();
    type Case = (
        &'static str,
        &'static [(u16, u16, u16)],
        Vec<ChannelDefinition>,
    );
    let cases: &[Case] = &[
        (
            "jp2_cdef_rgba",
            &[(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 1, 0)],
            rgba.clone(),
        ),
        (
            "jp2_cdef_alpha_first",
            &[(0, 1, 0), (1, 0, 1), (2, 0, 2), (3, 0, 3)],
            rgba,
        ),
        (
            "jp2_cdef_reversed",
            &[(0, 0, 3), (1, 0, 2), (2, 0, 1)],
            colours(3),
        ),
        (
            "jp2_cdef_palette",
            &[(0, 0, 3), (1, 0, 2), (2, 0, 1)],
            colours(3),
        ),
        (
            "jp2_cdef_unspecified",
            &[(0, 65535, 65535), (1, 65535, 65535), (2, 65535, 65535)],
            vec![def(Unspecified, Unassociated); 3],
        ),
    ];
    for (name, entries, want) in cases {
        let bytes = std::fs::read(dir().join(format!("{name}.jp2"))).unwrap();
        assert_eq!(cdef_entries(&bytes), *entries, "{name}: the box");
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(image.jp2.unwrap().channels, *want, "{name}");
    }
}

/// `jp2_metadata_boxes`'s snapshot, written whole by the box walker, holds
/// the boxes the fixture is for, so a regenerated snapshot cannot lose them
/// and still pass: negative `res ` exponents, three counted `colr` boxes,
/// a `cdef` channel that is not a colour, and `jp2i` and `uinf` boxes. The
/// decoder's match with it is checked above.
#[test]
fn the_metadata_fixture_carries_its_boxes() {
    let want = expected(&dir().join("jp2_metadata_boxes.jp2"));
    let jp2 = want.jp2.expect("a jp2 snapshot");
    let exponents = |res: Option<[i32; 6]>| res.map(|r| (r[4], r[5]));
    assert_eq!(exponents(jp2.resolution.capture), Some((-1, 3)));
    assert_eq!(exponents(jp2.resolution.display), Some((2, -128)));
    let methods: Vec<u8> = jp2.colour_specifications.iter().map(|c| c.method).collect();
    assert_eq!(methods, [1, 3, 1]);
    assert_eq!(
        jp2.channels.last().map(String::as_str),
        Some("premultiplied/2")
    );
    assert_eq!(jp2.ipr, [13, 7]);
    assert_eq!(jp2.uuid_info.len(), 1);
}

/// `jp2_signed16`'s oracle has negative 16-bit samples, which the fixture is
/// for; the decoder's match with it is checked above.
#[test]
fn the_signed_16_bit_oracle_has_negative_samples() {
    let want = expected(&dir().join("jp2_signed16.jp2"));
    let [plane] = &want.components[..] else {
        panic!("{} planes", want.components.len());
    };
    assert_eq!((plane.bit_depth, plane.signed), (16, true));
    assert!(plane.samples.iter().any(|&v| v < -(1 << 14)));
}

/// `raw_channels` gives exactly the decode of the bare codestream.
#[test]
fn raw_channels_returns_the_codestream_as_coded() {
    let options = DecodeOptions::default().with_raw_channels(true);
    for path in fixtures() {
        let name = path.display();
        let bytes = std::fs::read(&path).unwrap();
        let image = decode_with(&bytes, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
        let raw = decode(first_jp2c(&bytes)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            (image.width, image.height),
            (raw.width, raw.height),
            "{name}"
        );
        assert_eq!(image.components, raw.components, "{name}");
        assert!(!image.jp2.unwrap().palette_applied, "{name}");
    }
}
