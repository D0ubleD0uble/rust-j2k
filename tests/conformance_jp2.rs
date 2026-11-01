//! ISO/IEC 15444-4 Annex G JP2 reader-file grading harness.
//!
//! The set is `file1.jp2` .. `file9.jp2` with OpenJPEG's conformance baselines
//! as references (`tests/fixtures/conformance/jp2/`, described by its
//! `manifest.json`). The references are 8-bit RGB display output, so grading
//! decodes each file, shifts it right as `opj_decompress -p 8S` does, and
//! compares each channel to its `.pgx` plane with the Part 1 comparator
//! ([`support::part4`]) and the same ratchet.
//!
//! [`IN_CLASS`] starts empty: `decode` rejects the JP2 wrapper as
//! `Unsupported`, so every entry reports *not yet decoded*. Any other outcome
//! fails: a decode that errors differently or misses its reference is a
//! failure, and an entry that starts to pass must be added to [`IN_CLASS`].
//! Entries marked `needs_srgb` wait for the sRGB output option; flip
//! [`SRGB_OUTPUT_AVAILABLE`] (and `options_for` in `support/jp2.rs`) when it
//! exists.

mod support;

use std::path::PathBuf;

use support::jp2::{Jp2Manifest, load_manifest, run_ratchet, shifted};
use support::pgx::parse_pgx;

/// The entries that decode within their compliance class today. Grow it as the
/// file-format milestones land; never shrink it without an explanation.
const IN_CLASS: &[&str] = &[];

/// Whether the decoder can produce sRGB output. While `false`, `needs_srgb`
/// entries that decode report *needs sRGB output* and are not graded.
const SRGB_OUTPUT_AVAILABLE: bool = false;

fn jp2_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance/jp2")
}

/// Decode every JP2 file and grade it, enforcing the ratchet.
#[test]
fn grades_the_jp2_reader_files_by_compliance_class() {
    run_ratchet(&jp2_dir(), IN_CLASS, SRGB_OUTPUT_AVAILABLE);
}

/// Every reference plane parses as an 8-bit unsigned PGX of the file's size.
#[test]
fn every_jp2_reference_parses() {
    let dir = jp2_dir();
    let mut parsed = 0;
    for entry in load_manifest(&dir).entries {
        for path in &entry.references {
            let raw = std::fs::read(dir.join(path)).expect("read reference");
            let pgx = parse_pgx(&raw).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!((pgx.bit_depth, pgx.signed), (8, false), "{path}");
            assert_eq!(
                (pgx.width, pgx.height),
                (entry.features.width, entry.features.height),
                "{path}: plane size differs from the codestream's",
            );
            assert_eq!(pgx.samples.len(), (pgx.width * pgx.height) as usize);
            parsed += 1;
        }
    }
    assert_eq!(
        parsed, 21,
        "six colour files x3 planes, three grey files x1"
    );
}

/// The manifest records what the issue and OpenJPEG's conformance run say, so
/// a corpus edit cannot quietly change the bar.
#[test]
fn the_manifest_pins_the_grading_bar() {
    let dir = jp2_dir();
    let manifest = load_manifest(&dir);
    assert_eq!(manifest.entries.len(), 9);

    let indexes: Vec<u32> = manifest.entries.iter().map(|e| e.index).collect();
    assert_eq!(indexes, (1..=9).collect::<Vec<_>>());

    let pick = |f: fn(&support::jp2::Jp2Entry) -> bool| -> Vec<u32> {
        manifest
            .entries
            .iter()
            .filter(|e| f(e))
            .map(|e| e.index)
            .collect()
    };
    assert_eq!(pick(|e| e.bit_exact), [1, 4, 6, 9]);
    assert_eq!(pick(|e| e.needs_srgb), [2, 3, 5, 7, 8]);
    assert_eq!(pick(|e| e.graded_components == 1), [4, 6, 8]);
    assert_eq!(pick(|e| e.shift_right == 4), [6]);
    assert_eq!(pick(|e| e.shift_right == 8), [7]);
    assert_eq!(pick(|e| e.shift_right != 0), [6, 7]);

    for entry in &manifest.entries {
        // Part 4 Table G.1 as OpenJPEG runs it: PAE 4, MSE 1 on every channel.
        assert_eq!(entry.bounds.pae, vec![4.0; entry.graded_components]);
        assert_eq!(entry.bounds.mse, vec![1.0; entry.graded_components]);

        let bytes = std::fs::read(dir.join(&entry.file)).expect("read jp2");
        assert_eq!(
            &bytes[..12],
            b"\x00\x00\x00\x0cjP  \r\n\x87\n",
            "{}: JP2 signature box",
            entry.file
        );
        assert_eq!(entry.boxes.top.first().map(String::as_str), Some("jP  "));
        assert!(entry.boxes.top.iter().any(|b| b == "jp2c"));
        assert!(entry.boxes.jp2h.iter().any(|b| b == "colr"));
    }
}

/// Unknown manifest keys are rejected at the top level, on an entry, and in a
/// nested object, rather than ignored.
#[test]
fn the_manifest_rejects_unknown_keys() {
    let text = std::fs::read_to_string(jp2_dir().join("manifest.json")).expect("read manifest");
    // The committed manifest round-trips.
    serde_json::from_str::<Jp2Manifest>(&text).expect("committed manifest parses");

    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    let reject = |mutate: &dyn Fn(&mut serde_json::Value), what: &str| {
        let mut v = value.clone();
        mutate(&mut v);
        let err = serde_json::from_value::<Jp2Manifest>(v)
            .expect_err(&format!("an unknown key {what} must be rejected"));
        assert!(err.to_string().contains("unknown field"), "{what}: {err}");
    };
    reject(&|v| v["surprise"] = 1.into(), "at the top level");
    reject(&|v| v["entries"][0]["surprise"] = 1.into(), "on an entry");
    reject(
        &|v| v["entries"][0]["colour"]["surprise"] = 1.into(),
        "in colour",
    );
    reject(
        &|v| v["entries"][0]["boxes"]["surprise"] = 1.into(),
        "in boxes",
    );
}

/// The shift is arithmetic and takes the depth down with it.
#[test]
fn shifting_scales_samples_and_depth() {
    let component = rust_j2k::Component::new(3, 1, 12, false, 1, 1, vec![0, 15, 4095]);
    let out = shifted(&component, 4);
    assert_eq!(out.samples, [0, 0, 255]);
    assert_eq!(out.bit_depth, 8);
    assert_eq!(shifted(&component, 0), component);
}
