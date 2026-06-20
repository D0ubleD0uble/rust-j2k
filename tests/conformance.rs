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
//! corpus stays reproducible. Seed it from the GRIB2 §5.40 fixture already in
//! the fieldglass repo (`jpeg2000_regular_latlon.grib2`) plus HRRR (lossy) and
//! MRMS samples. The `<name>.expected.json` schema lives in [`support`].

mod support;

#[test]
#[ignore = "no fixtures yet — add real codestreams + an OpenJPEG/eccodes oracle"]
fn decodes_corpus_against_oracle() {
    // for each fixture:
    //   let bytes = std::fs::read(path).unwrap();
    //   let img = rust_j2k::decode(&bytes).expect("decode");
    //   let oracle = support::Expected::from_json(&json_text).expect("oracle");
    //   assert_image_matches_oracle(&img, &oracle);
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
