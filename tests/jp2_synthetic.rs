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
            "jp2_palette_12bit",
            "jp2_palette_8bit",
            "jp2_palette_clamp",
            "jp2_palette_corrected",
            "jp2_palette_mixed",
            "jp2_palette_subsampled_big_first",
            "jp2_palette_subsampled_small_first",
            "jp2_sycc_420_odd",
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
        let image = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Err(mismatch) = compare(&image, &want) {
            panic!("{name}: {mismatch}");
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

/// `raw_channels` gives exactly the decode of the bare codestream.
#[test]
fn raw_channels_returns_the_codestream_as_coded() {
    let options = DecodeOptions::default().with_raw_channels(true);
    for path in fixtures() {
        let name = path.display();
        let bytes = std::fs::read(&path).unwrap();
        let image = decode_with(&bytes, options.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
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
