//! Synthetic JP2 fixtures (`tests/fixtures/jp2_*.jp2`): lossless codestreams
//! wrapped in hand-built boxes by `scripts/gen-jp2-fixtures.py`, each with an
//! `<name>.expected.json` oracle from `opj_decompress` (provenance in the
//! fixtures README and in each snapshot). Every fixture must match its oracle
//! bit-exactly.

mod support;

use std::path::{Path, PathBuf};

use rust_j2k::{DecodeOptions, decode, decode_with};
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
            "jp2_palette_12bit",
            "jp2_palette_8bit",
            "jp2_palette_clamp",
            "jp2_palette_corrected",
            "jp2_palette_mixed",
            "jp2_palette_permuted",
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
        assert!(jp2.palette_applied, "{name}");
        assert_eq!(jp2.channels.len(), image.components.len(), "{name}");
    }
}

/// `raw_channels` gives exactly the decode of the bare codestream.
#[test]
fn raw_channels_returns_the_codestream_as_coded() {
    let mut options = DecodeOptions::default();
    options.raw_channels = true;
    for path in fixtures() {
        let name = path.display();
        let bytes = std::fs::read(&path).unwrap();
        let image = decode_with(&bytes, options).unwrap_or_else(|e| panic!("{name}: {e}"));
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
