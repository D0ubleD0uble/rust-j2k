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
//! The manifest is deserialized through the single typed schema in
//! [`support::part4`], whose `#[serde(deny_unknown_fields)]` is the
//! exhaustive-fields check: a manifest key with no matching struct field (a
//! schema drift) fails [`load_manifest`] here, and every modelled field is
//! required, so a missing or mistyped field fails too — replacing the parallel
//! untyped field-path assertions this test used to carry.
//!
//! See `tests/fixtures/conformance/README.md` for the corpus and its license.

mod support;

use std::path::PathBuf;

use support::part4::{Cblksty, Features, load_manifest};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

#[test]
fn manifest_and_corpus_are_consistent() {
    let dir = corpus_dir();
    // Deserializing through the typed schema is itself the field check: an
    // unknown key, or a missing/mistyped modelled field, panics here.
    let manifest = load_manifest(&dir);
    assert_eq!(
        manifest.entries.len(),
        23,
        "expected 23 conformance entries"
    );

    for entry in &manifest.entries {
        let cs = &entry.codestream;
        let cs_path = dir.join(cs);
        let bytes = std::fs::read(&cs_path)
            .unwrap_or_else(|e| panic!("read codestream {}: {e}", cs_path.display()));
        assert!(
            bytes.starts_with(&[0xFF, 0x4F]),
            "{cs}: missing SOC marker (0xFF4F) — truncated or not a raw codestream?"
        );

        // Components graded at class 1 — may be fewer than the image's total
        // (p0_13 is a 257-component image of which 4 are graded).
        let graded = entry.graded_components;
        assert!(graded >= 1, "{cs}: graded_components must be >= 1");
        let image_components = entry.features.components as usize;
        assert!(
            graded <= image_components,
            "{cs}: grades {graded} but image has {image_components} components"
        );

        assert_eq!(
            entry.references.class1.len(),
            graded,
            "{cs}: {} class-1 references for {graded} graded components",
            entry.references.class1.len(),
        );

        assert!(
            !entry.references.class0.is_empty(),
            "{cs}: no class-0 reference"
        );
        // Validate every reference is real content, not a zero-byte stub or an
        // LFS pointer: each .pgx must be non-empty and carry the PGX magic. This
        // keeps the docstring's "truncated commit fails loudly" promise honest
        // for references, the way the SOC check does for codestreams.
        for r in entry
            .references
            .class1
            .iter()
            .chain(&entry.references.class0)
        {
            let rp = dir.join(r);
            let rb = std::fs::read(&rp)
                .unwrap_or_else(|e| panic!("read reference {}: {e}", rp.display()));
            assert!(
                rb.starts_with(b"PG"),
                "{}: missing PGX magic — truncated or not a .pgx?",
                rp.display()
            );
        }

        // Class-1 bounds are the grading bar: one PAE and one MSE per component.
        assert_eq!(
            entry.bounds_class1.pae.len(),
            graded,
            "{cs}: one PAE bound per graded component"
        );
        assert_eq!(
            entry.bounds_class1.mse.len(),
            graded,
            "{cs}: one MSE bound per graded component"
        );

        // `bit_exact` must agree with all-zero bounds (graded for an exact
        // match). This tracks the bounds, not the wavelet: p0_09 is 9/7 yet
        // bit_exact, so it is intentionally decoupled from `features.reversible`.
        let all_zero = entry
            .bounds_class1
            .pae
            .iter()
            .chain(&entry.bounds_class1.mse)
            .all(|&v| v == 0.0);
        assert_eq!(
            entry.bit_exact, all_zero,
            "{cs}: `bit_exact` disagrees with its bounds"
        );

        // Every tile has at least one tile-part.
        let tile_count = entry.features.tiles[0] as u64 * entry.features.tiles[1] as u64;
        let tile_parts = entry.features.tile_parts as u64;
        assert!(
            tile_parts >= tile_count,
            "{cs}: {tile_parts} tile-parts for {tile_count} tiles"
        );
    }
}

/// The corpus covers every Phase 2 feature the issues gate on — except PLM,
/// whose absence is asserted here so the gap stays visible: issue #72 grades
/// PLM against a synthetic fixture instead.
#[test]
fn corpus_covers_the_phase2_feature_matrix() {
    let manifest = load_manifest(&corpus_dir());
    let entries = &manifest.entries;

    let has = |pred: &dyn Fn(&Features) -> bool| entries.iter().any(|e| pred(&e.features));
    let marker = |f: &Features, m: &str| {
        f.markers_main.iter().any(|x| x == m) || f.markers_tile.iter().any(|x| x == m)
    };

    for bit in Cblksty::NAMES {
        assert!(
            has(&|f| f.cblksty.flag(bit)),
            "no corpus entry exercises code-block style `{bit}`"
        );
    }
    assert!(has(&|f| f.sop), "no corpus entry exercises `sop`");
    assert!(has(&|f| f.eph), "no corpus entry exercises `eph`");
    assert!(
        has(&|f| f.precincts),
        "no corpus entry exercises `precincts`"
    );
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
            has(&|f| f.progression == prog),
            "no corpus entry uses {prog} progression"
        );
    }
    assert!(
        has(&|f| f.components > 1),
        "no multi-component corpus entry"
    );
    assert!(
        has(&|f| {
            let tiles = f.tiles[0] as u64 * f.tiles[1] as u64;
            f.tile_parts as u64 > tiles
        }),
        "no corpus entry with more tile-parts than tiles"
    );
    assert!(has(&|f| f.mct == 1 && f.reversible), "no RCT corpus entry");
    assert!(has(&|f| f.mct == 1 && !f.reversible), "no ICT corpus entry");

    // Known coverage gaps, asserted so a corpus refresh that closes (or
    // widens) them fails loudly and the affected issues get updated:
    // - PLM appears in no entry (issue #72 uses a synthetic fixture);
    // - no tile-part header carries a COD/COC/QCC override (issue #59 covers
    //   per-tile COD/QCD resolution with a synthetic fixture; per-tile
    //   quantization is exercised only via p1_04's tile-part QCDs).
    assert!(!has(&|f| marker(f, "PLM")), "corpus now covers PLM");
    for m in ["COD", "COC", "QCC"] {
        assert!(
            !has(&|f| f.markers_tile.iter().any(|x| x == m)),
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
