//! Conformance harness — the bar that defines "correct".
//!
//! Decode a corpus of real JPEG 2000 codestreams and compare against an oracle
//! produced by a trusted decoder (OpenJPEG, or eccodes' `grid_jpeg` decode for
//! the GRIB2-sourced ones). The agreement standard:
//!
//! - **reversible (5/3, lossless)** fixtures: bit-exact sample equality;
//! - **irreversible (9/7, lossy)** fixtures: within a stated absolute tolerance.
//!
//! Drop fixtures under `tests/fixtures/<name>.j2k` with a sibling
//! `<name>.expected.json` oracle (samples + geometry + tolerance), and record
//! each fixture's provenance (source file, how the oracle was generated) so the
//! corpus stays reproducible. Seed it from the GRIB2 §5.40 fixtures in the
//! fieldglass repo, plus a 9/7 re-encode for the irreversible path (no GRIB2
//! producer ships lossy 9/7). The `<name>.expected.json` schema lives in
//! [`support`].

mod support;

use std::path::PathBuf;

use support::{Outcome, discover, run_fixture};

/// The committed fixture corpus lives next to this test file.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Decode every committed fixture and grade it against its oracle snapshot.
///
/// While `decode` is a skeleton (`todo!()`), each real codestream reports as
/// *not yet decoded* ([`Outcome::Pending`]) rather than panicking the suite —
/// that is the expected steady state for P1.0. The gate fails only on a genuine
/// disagreement ([`Outcome::Failed`]) or a broken fixture ([`Outcome::LoadError`]),
/// so the harness goes green now and stays meaningful as the pipeline lands.
#[test]
fn decodes_corpus_against_oracle() {
    let fixtures = discover(&fixtures_dir());
    if fixtures.is_empty() {
        eprintln!("conformance: no fixtures committed yet (see issue #4); harness is wired.");
        return;
    }

    let mut failures = Vec::new();
    for fixture in &fixtures {
        let outcome = run_fixture(fixture);
        match &outcome {
            Outcome::Passed => eprintln!("conformance: {} … ok", fixture.name),
            Outcome::Pending => {
                eprintln!("conformance: {} … pending (decode is todo!)", fixture.name)
            }
            Outcome::Failed(m) => {
                eprintln!("conformance: {} … FAILED: {m}", fixture.name);
                failures.push(format!("{}: {m}", fixture.name));
            }
            Outcome::LoadError(e) => {
                eprintln!("conformance: {} … LOAD ERROR: {e}", fixture.name);
                failures.push(format!("{}: {e}", fixture.name));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} fixture(s) failed the conformance gate:\n  {}",
        failures.len(),
        failures.join("\n  "),
    );
}

/// Harness behaviour: discovery, the comparator, and outcome classification.
/// These exercise the harness with hand-built inputs so it is proven before
/// the real corpus (issue #4) and a working `decode` exist.
mod harness {
    use super::support::{
        Expected, Fixture, Geometry, Mismatch, Outcome, Provenance, Tolerance, classify, compare,
        discover, run_fixture,
    };
    use rust_j2k::{Error, Image};
    use std::path::{Path, PathBuf};

    fn geometry(width: u32, height: u32) -> Geometry {
        Geometry {
            width,
            height,
            bit_depth: 16,
            signed: false,
        }
    }

    fn image(width: u32, height: u32, samples: Vec<i32>) -> Image {
        Image {
            width,
            height,
            bit_depth: 16,
            signed: false,
            samples,
        }
    }

    fn snapshot(geometry: Geometry, tolerance: Tolerance, samples: Vec<i32>) -> Expected {
        Expected {
            geometry,
            tolerance,
            samples,
            provenance: Provenance {
                source: "test".into(),
                oracle_command: "n/a".into(),
                notes: None,
            },
        }
    }

    // --- comparator ---------------------------------------------------------

    #[test]
    fn exact_match_passes() {
        let want = snapshot(geometry(2, 2), Tolerance::Exact, vec![0, 1, 2, 3]);
        let got = image(2, 2, vec![0, 1, 2, 3]);
        assert_eq!(compare(&got, &want), Ok(()));
    }

    #[test]
    fn exact_reports_first_divergent_sample() {
        // Second row, second column (index 3) is the first divergence.
        let want = snapshot(geometry(2, 2), Tolerance::Exact, vec![0, 1, 2, 3]);
        let got = image(2, 2, vec![0, 1, 2, 9]);
        let err = compare(&got, &want).unwrap_err();
        assert_eq!(
            err,
            Mismatch::Sample {
                index: 3,
                x: 1,
                y: 1,
                expected: 3,
                actual: 9,
                bound: 0.0,
            }
        );
    }

    #[test]
    fn absolute_within_bound_passes() {
        let want = snapshot(
            geometry(2, 1),
            Tolerance::Absolute { max_abs_error: 1.5 },
            vec![10, 20],
        );
        let got = image(2, 1, vec![11, 19]); // |diff| = 1 ≤ 1.5
        assert_eq!(compare(&got, &want), Ok(()));
    }

    #[test]
    fn absolute_outside_bound_fails() {
        let want = snapshot(
            geometry(2, 1),
            Tolerance::Absolute { max_abs_error: 1.5 },
            vec![10, 20],
        );
        let got = image(2, 1, vec![10, 23]); // |diff| = 3 > 1.5 at index 1
        let err = compare(&got, &want).unwrap_err();
        assert_eq!(
            err,
            Mismatch::Sample {
                index: 1,
                x: 1,
                y: 0,
                expected: 20,
                actual: 23,
                bound: 1.5,
            }
        );
    }

    #[test]
    fn geometry_mismatch_fails_before_samples() {
        let want = snapshot(geometry(2, 2), Tolerance::Exact, vec![0, 0, 0, 0]);
        let got = image(4, 1, vec![0, 0, 0, 0]); // same count, wrong shape
        let err = compare(&got, &want).unwrap_err();
        assert!(matches!(err, Mismatch::Geometry { .. }));
    }

    // --- classification -----------------------------------------------------

    #[test]
    fn panic_classifies_as_pending() {
        let want = snapshot(geometry(1, 1), Tolerance::Exact, vec![0]);
        let decoded =
            std::panic::catch_unwind(|| -> Result<Image, Error> { todo!("stubbed decode") });
        assert_eq!(classify(decoded, &want), Outcome::Pending);
    }

    #[test]
    fn ok_matching_classifies_as_passed() {
        let want = snapshot(geometry(1, 1), Tolerance::Exact, vec![42]);
        let decoded = Ok(Ok(image(1, 1, vec![42])));
        assert_eq!(classify(decoded, &want), Outcome::Passed);
    }

    #[test]
    fn ok_mismatch_classifies_as_failed() {
        let want = snapshot(geometry(1, 1), Tolerance::Exact, vec![42]);
        let decoded = Ok(Ok(image(1, 1, vec![7])));
        assert!(matches!(classify(decoded, &want), Outcome::Failed(_)));
    }

    #[test]
    fn decode_error_classifies_as_failed() {
        let want = snapshot(geometry(1, 1), Tolerance::Exact, vec![42]);
        let decoded = Ok(Err(Error::Unsupported("nope".into())));
        assert!(matches!(
            classify(decoded, &want),
            Outcome::Failed(Mismatch::DecodeError(_))
        ));
    }

    // --- discovery ----------------------------------------------------------

    /// A unique scratch directory under the test binary's temp dir, so discovery
    /// can be exercised against synthetic fixtures without a committed corpus.
    fn scratch(tag: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("discover-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    const VALID_SNAPSHOT: &str = r#"{
  "geometry": { "width": 1, "height": 1, "bit_depth": 8, "signed": false },
  "tolerance": { "mode": "exact" },
  "samples": [0],
  "provenance": { "source": "s", "oracle_command": "c" }
}"#;

    #[test]
    fn discovers_paired_fixture() {
        let dir = scratch("paired");
        // Not a real codestream, so end to end it classifies as Failed
        // (the decoder rightly rejects non-codestream bytes). What is under
        // test here is discovery: the `.j2k` is paired with its snapshot and
        // run end to end. The Pending path is covered by `panic_classifies_as_pending`.
        write(&dir, "alpha.j2k", "\x00not-a-codestream");
        write(&dir, "alpha.expected.json", VALID_SNAPSHOT);

        let found = discover(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "alpha");
        assert!(found[0].expected.is_ok());
        assert!(matches!(
            run_fixture(&found[0]),
            Outcome::Failed(Mismatch::DecodeError(_))
        ));
    }

    #[test]
    fn missing_snapshot_is_a_load_error() {
        let dir = scratch("orphan-codestream");
        write(&dir, "lonely.j2k", "bytes");
        // no sibling .expected.json

        let found = discover(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].expected.is_err());
        assert!(matches!(run_fixture(&found[0]), Outcome::LoadError(_)));
    }

    #[test]
    fn malformed_snapshot_is_a_load_error() {
        let dir = scratch("malformed-snapshot");
        write(&dir, "broken.j2k", "bytes");
        write(&dir, "broken.expected.json", "{ not valid json");

        let found = discover(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].expected.is_err());
        assert!(matches!(run_fixture(&found[0]), Outcome::LoadError(_)));
    }

    #[test]
    fn snapshot_with_wrong_sample_count_is_a_load_error() {
        let dir = scratch("wrong-count");
        write(&dir, "bad.j2k", "bytes");
        // geometry declares 2x2 = 4 samples, but only 3 are listed.
        write(
            &dir,
            "bad.expected.json",
            r#"{
  "geometry": { "width": 2, "height": 2, "bit_depth": 8, "signed": false },
  "tolerance": { "mode": "exact" },
  "samples": [0, 1, 2],
  "provenance": { "source": "s", "oracle_command": "c" }
}"#,
        );

        let found = discover(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].expected.is_err());
        assert!(matches!(run_fixture(&found[0]), Outcome::LoadError(_)));
    }

    #[test]
    fn discovery_is_sorted_and_ignores_other_files() {
        let dir = scratch("sorted");
        write(&dir, "beta.j2k", "bytes");
        write(&dir, "beta.expected.json", VALID_SNAPSHOT);
        write(&dir, "alpha.j2k", "bytes");
        write(&dir, "alpha.expected.json", VALID_SNAPSHOT);
        write(&dir, "notes.txt", "ignore me");

        let names: Vec<_> = discover(&dir).into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn missing_directory_yields_empty_corpus() {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("does-not-exist-xyz");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(discover(&dir).is_empty());
    }

    // Touch `Fixture` directly so the type is exercised by name.
    #[test]
    fn fixture_load_error_short_circuits_run() {
        let fixture = Fixture {
            name: "x".into(),
            codestream: PathBuf::from("/nonexistent/x.j2k"),
            expected: Err("synthetic load error".into()),
        };
        assert!(matches!(run_fixture(&fixture), Outcome::LoadError(_)));
    }
}

mod expected_schema {
    use super::support::{Expected, Geometry, Provenance, Tolerance};

    /// A reversible (lossless) snapshot: 2x2 grid, exact agreement required.
    /// Kept in sync with the documented example in `tests/fixtures/README.md`.
    const EXACT_JSON: &str = r#"{
  "geometry": { "width": 2, "height": 2, "bit_depth": 16, "signed": false },
  "tolerance": { "mode": "exact" },
  "samples": [0, 1, 2, 3],
  "provenance": {
    "source": "fieldglass/jpeg2000_regular_latlon.grib2",
    "oracle_command": "opj_decompress -i sample.j2k -o sample.pgx"
  }
}"#;

    /// An irreversible (lossy) snapshot: same grid, bounded absolute error, and
    /// an optional provenance note.
    const ABSOLUTE_JSON: &str = r#"{
  "geometry": { "width": 2, "height": 1, "bit_depth": 12, "signed": true },
  "tolerance": { "mode": "absolute", "max_abs_error": 1.5 },
  "samples": [-2, 7],
  "provenance": {
    "source": "hrrr/sample.grib2",
    "oracle_command": "grib_to_jpeg ...",
    "notes": "9/7 lossy path"
  }
}"#;

    #[test]
    fn parses_exact_reversible_snapshot() {
        let expected = Expected::from_json(EXACT_JSON).expect("parse exact snapshot");

        assert_eq!(
            expected.geometry,
            Geometry {
                width: 2,
                height: 2,
                bit_depth: 16,
                signed: false,
            }
        );
        assert_eq!(expected.tolerance, Tolerance::Exact);
        assert_eq!(expected.samples, vec![0, 1, 2, 3]);
        assert_eq!(expected.samples.len(), expected.geometry.sample_count());
        assert_eq!(expected.provenance.notes, None);
    }

    #[test]
    fn parses_absolute_irreversible_snapshot() {
        let expected = Expected::from_json(ABSOLUTE_JSON).expect("parse absolute snapshot");

        assert_eq!(
            expected.tolerance,
            Tolerance::Absolute { max_abs_error: 1.5 }
        );
        assert_eq!(expected.samples, vec![-2, 7]);
        assert_eq!(expected.samples.len(), expected.geometry.sample_count());
        assert_eq!(expected.provenance.source, "hrrr/sample.grib2");
        assert_eq!(expected.provenance.notes.as_deref(), Some("9/7 lossy path"));
    }

    #[test]
    fn round_trips_through_serde() {
        // Deserialize -> serialize -> deserialize must be a fixed point, so the
        // committed JSON and the schema can never silently drift apart.
        let parsed = Expected::from_json(EXACT_JSON).expect("parse");
        let reserialized = serde_json::to_string(&parsed).expect("serialize");
        let reparsed = Expected::from_json(&reserialized).expect("reparse");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn omitted_optional_note_defaults_to_none() {
        let json = r#"{
  "geometry": { "width": 1, "height": 1, "bit_depth": 8, "signed": false },
  "tolerance": { "mode": "exact" },
  "samples": [42],
  "provenance": { "source": "s", "oracle_command": "c" }
}"#;
        let expected = Expected::from_json(json).expect("parse without notes");
        assert_eq!(
            expected.provenance,
            Provenance {
                source: "s".to_string(),
                oracle_command: "c".to_string(),
                notes: None,
            }
        );
    }

    #[test]
    fn rejects_unknown_field() {
        // A typo'd key (here `sample` instead of `samples`) must fail loudly
        // rather than silently deserialize to an empty grid.
        let json = r#"{
  "geometry": { "width": 1, "height": 1, "bit_depth": 8, "signed": false },
  "tolerance": { "mode": "exact" },
  "sample": [0],
  "provenance": { "source": "s", "oracle_command": "c" }
}"#;
        assert!(Expected::from_json(json).is_err());
    }

    #[test]
    fn rejects_unknown_tolerance_mode() {
        let json = r#"{
  "geometry": { "width": 1, "height": 1, "bit_depth": 8, "signed": false },
  "tolerance": { "mode": "relative", "max_abs_error": 1.0 },
  "samples": [0],
  "provenance": { "source": "s", "oracle_command": "c" }
}"#;
        assert!(Expected::from_json(json).is_err());
    }
}
