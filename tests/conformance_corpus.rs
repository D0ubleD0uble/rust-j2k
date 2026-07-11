//! Integrity check for the vendored ISO/IEC 15444-4 conformance corpus.
//!
//! This is *not* the grading harness — decoding these codestreams and comparing
//! against the PAE/MSE bounds is `tests/conformance_part4.rs`. Here we only
//! verify the corpus is internally consistent: every file `manifest.json` names exists
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

        // Feature-to-fixture fields: the Phase 2 issues key their acceptance
        // criteria on "the matching Part 4 codestream", and these fields are
        // what makes the match identifiable from committed data.
        for key in ["markers_main", "markers_tile"] {
            entry["features"][key]
                .as_array()
                .unwrap_or_else(|| panic!("{cs}: `features.{key}` is an array"));
        }
        for key in ["sop", "eph", "precincts"] {
            entry["features"][key]
                .as_bool()
                .unwrap_or_else(|| panic!("{cs}: `features.{key}` is a bool"));
        }
        let cblksty = entry["features"]["cblksty"]
            .as_object()
            .expect("entry has `features.cblksty`");
        for bit in CBLKSTY_BITS {
            cblksty[bit]
                .as_bool()
                .unwrap_or_else(|| panic!("{cs}: `features.cblksty.{bit}` is a bool"));
        }
        // Every tile has at least one tile-part.
        let tiles = entry["features"]["tiles"]
            .as_array()
            .expect("entry has `features.tiles`");
        let tile_count: u64 = tiles.iter().map(|t| t.as_u64().unwrap()).product();
        let tile_parts = entry["features"]["tile_parts"]
            .as_u64()
            .expect("entry has `features.tile_parts`");
        assert!(
            tile_parts >= tile_count,
            "{cs}: {tile_parts} tile-parts for {tile_count} tiles"
        );
    }
}

const CBLKSTY_BITS: [&str; 6] = [
    "bypass",
    "reset",
    "restart",
    "vert_causal",
    "pred_term",
    "segsym",
];

/// The corpus covers every Phase 2 feature the issues gate on — except PLM,
/// whose absence is asserted here so the gap stays visible: issue #72 grades
/// PLM against a synthetic fixture instead.
#[test]
fn corpus_covers_the_phase2_feature_matrix() {
    let dir = corpus_dir();
    let text = std::fs::read_to_string(dir.join("manifest.json")).expect("read manifest.json");
    let manifest: Value = serde_json::from_str(&text).expect("manifest.json parses as JSON");
    let entries = manifest["entries"].as_array().expect("`entries` array");

    let has = |pred: &dyn Fn(&Value) -> bool| entries.iter().any(|e| pred(&e["features"]));
    let marker = |f: &Value, m: &str| {
        ["markers_main", "markers_tile"].iter().any(|k| {
            f[k].as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(m)))
        })
    };

    for bit in CBLKSTY_BITS {
        assert!(
            has(&|f| f["cblksty"][bit].as_bool() == Some(true)),
            "no corpus entry exercises code-block style `{bit}`"
        );
    }
    for flag in ["sop", "eph", "precincts"] {
        assert!(
            has(&|f| f[flag].as_bool() == Some(true)),
            "no corpus entry exercises `{flag}`"
        );
    }
    for m in [
        "COC", "QCC", "RGN", "POC", "PPM", "PPT", "PLT", "TLM", "CRG",
    ] {
        assert!(
            has(&|f| marker(f, m)),
            "no corpus entry carries the {m} marker"
        );
    }
    for prog in ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"] {
        assert!(
            has(&|f| f["progression"].as_str() == Some(prog)),
            "no corpus entry uses {prog} progression"
        );
    }
    assert!(
        has(&|f| f["components"].as_u64() > Some(1)),
        "no multi-component corpus entry"
    );
    assert!(
        has(&|f| {
            let tiles = f["tiles"][0].as_u64().unwrap() * f["tiles"][1].as_u64().unwrap();
            f["tile_parts"].as_u64() > Some(tiles)
        }),
        "no corpus entry with more tile-parts than tiles"
    );
    assert!(
        has(&|f| f["mct"].as_u64() == Some(1) && f["reversible"] == Value::Bool(true)),
        "no RCT corpus entry"
    );
    assert!(
        has(&|f| f["mct"].as_u64() == Some(1) && f["reversible"] == Value::Bool(false)),
        "no ICT corpus entry"
    );

    // Known coverage gaps, asserted so a corpus refresh that closes (or
    // widens) them fails loudly and the affected issues get updated:
    // - PLM appears in no entry (issue #72 uses a synthetic fixture);
    // - no tile-part header carries a COD/COC/QCC override (issue #59 covers
    //   per-tile COD/QCD resolution with a synthetic fixture; per-tile
    //   quantization is exercised only via p1_04's tile-part QCDs).
    assert!(!has(&|f| marker(f, "PLM")), "corpus now covers PLM");
    for m in ["COD", "COC", "QCC"] {
        assert!(
            !has(&|f| f["markers_tile"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(m)))),
            "corpus now covers tile-part {m}"
        );
    }
}

/// Length markers are informational: the packets and tile-parts they point at
/// are read from the codestream itself, so decoding must not depend on the
/// hint. Excise `p0_05`'s TLM segment and the decode must be identical —
/// which is issue #72's "same result with the hints used and ignored", pinned
/// against the one conformance entry that both carries a length marker and
/// decodes today.
#[test]
fn decoding_p0_05_ignores_its_tlm_hint() {
    let bytes = std::fs::read(corpus_dir().join("codestreams/p0_05.j2k")).expect("read p0_05");

    // TLM = 0xFF55; its segment length immediately follows the marker.
    let tlm = bytes
        .windows(2)
        .position(|w| w == [0xFF, 0x55])
        .expect("p0_05 carries a TLM marker");
    let seg_len = u16::from_be_bytes([bytes[tlm + 2], bytes[tlm + 3]]) as usize;
    let mut without = bytes.clone();
    without.drain(tlm..tlm + 2 + seg_len);

    let with_hint = rust_j2k::decode(&bytes).expect("p0_05 decodes");
    let without_hint = rust_j2k::decode(&without).expect("p0_05 decodes without its TLM");
    assert_eq!(with_hint, without_hint);
}
