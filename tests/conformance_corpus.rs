//! Integrity check for the vendored ISO/IEC 15444-4 conformance corpus.
//!
//! This is *not* the grading harness — decoding these codestreams and comparing
//! against the PAE/MSE bounds is separate work (issue #55). Here we only verify
//! the corpus is internally consistent: every file `manifest.json` names exists
//! and carries the right magic (codestreams the JPEG 2000 SOC marker, references
//! the PGX magic, so a truncated or LFS-pointer-only commit fails loudly), and
//! the per-component bound arrays line up with the graded-component count and the
//! class-1 references.
//!
//! See `tests/fixtures/conformance/README.md` for the corpus and its license.

use std::path::PathBuf;

use serde_json::Value;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

#[test]
fn manifest_and_corpus_are_consistent() {
    let dir = corpus_dir();
    let manifest_path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let manifest: Value = serde_json::from_str(&text).expect("manifest.json parses as JSON");

    let entries = manifest["entries"]
        .as_array()
        .expect("manifest has an `entries` array");
    assert_eq!(entries.len(), 23, "expected 23 conformance entries");

    for entry in entries {
        let cs = entry["codestream"]
            .as_str()
            .expect("entry has `codestream`");
        let cs_path = dir.join(cs);
        let bytes = std::fs::read(&cs_path)
            .unwrap_or_else(|e| panic!("read codestream {}: {e}", cs_path.display()));
        assert!(
            bytes.starts_with(&[0xFF, 0x4F]),
            "{cs}: missing SOC marker (0xFF4F) — truncated or not a raw codestream?"
        );

        // Components graded at class 1 — may be fewer than the image's total
        // (p0_13 is a 257-component image of which 4 are graded).
        let graded = entry["graded_components"]
            .as_u64()
            .expect("entry has `graded_components`") as usize;
        assert!(graded >= 1, "{cs}: graded_components must be >= 1");
        let image_components = entry["features"]["components"]
            .as_u64()
            .expect("entry has `features.components`") as usize;
        assert!(
            graded <= image_components,
            "{cs}: grades {graded} but image has {image_components} components"
        );

        let class1 = entry["references"]["class1"]
            .as_array()
            .expect("entry has `references.class1`");
        assert_eq!(
            class1.len(),
            graded,
            "{cs}: {} class-1 references for {graded} graded components",
            class1.len(),
        );

        let class0 = entry["references"]["class0"]
            .as_array()
            .expect("entry has `references.class0`");
        assert!(!class0.is_empty(), "{cs}: no class-0 reference");
        // Validate every reference is real content, not a zero-byte stub or an
        // LFS pointer: each .pgx must be non-empty and carry the PGX magic. This
        // keeps the docstring's "truncated commit fails loudly" promise honest
        // for references, the way the SOC check does for codestreams.
        for r in class1.iter().chain(class0.iter()) {
            let rp = dir.join(r.as_str().expect("reference path is a string"));
            let rb = std::fs::read(&rp)
                .unwrap_or_else(|e| panic!("read reference {}: {e}", rp.display()));
            assert!(
                rb.starts_with(b"PG"),
                "{}: missing PGX magic — truncated or not a .pgx?",
                rp.display()
            );
        }

        // Class-1 bounds are the grading bar: one PAE and one MSE per component.
        let pae = entry["bounds_class1"]["pae"]
            .as_array()
            .expect("entry has `bounds_class1.pae`");
        let mse = entry["bounds_class1"]["mse"]
            .as_array()
            .expect("entry has `bounds_class1.mse`");
        assert_eq!(
            pae.len(),
            graded,
            "{cs}: one PAE bound per graded component"
        );
        assert_eq!(
            mse.len(),
            graded,
            "{cs}: one MSE bound per graded component"
        );

        // `bit_exact` must agree with all-zero bounds (graded for an exact
        // match). This tracks the bounds, not the wavelet: p0_09 is 9/7 yet
        // bit_exact, so it is intentionally decoupled from `features.reversible`.
        let bit_exact = entry["bit_exact"].as_bool().expect("entry has `bit_exact`");
        let all_zero = pae
            .iter()
            .chain(mse.iter())
            .all(|v| v.as_f64() == Some(0.0));
        assert_eq!(
            bit_exact, all_zero,
            "{cs}: `bit_exact` disagrees with its bounds"
        );
    }
}
