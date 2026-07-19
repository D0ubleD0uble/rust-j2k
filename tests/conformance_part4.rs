//! ISO/IEC 15444-4 compliance-class grading harness.
//!
//! This is the authoritative bar for "is the decoder correct" (see
//! `docs/correctness.md` §The hierarchy of authority). Part 4 defines decoder
//! conformance not as bit-exact equality — a lossy decoder legitimately differs
//! from the reference — but as staying within a bounded **peak absolute error**
//! (PAE) and a bounded **mean-squared error** (MSE) against the reference
//! decoded image, at a stated compliance class.
//!
//! The corpus lives in `tests/fixtures/conformance/` and is described by its
//! `manifest.json`: per entry, the codestream, the class-1 reference images
//! (one `.pgx` per graded component), the per-component PAE/MSE bounds, and a
//! `bit_exact` flag. We grade at **class 1**, which bounds every graded
//! component independently, so the bounds arrays are consumed as-is.
//!
//! Exactness is graded off each entry's `bit_exact` flag, never off its
//! wavelet: `p0_09` is 9/7 yet reference-exact, and `p0_04` is 5/3-free lossy.
//! When the flag is set the bounds are forced to zero regardless of what the
//! manifest records, so a corpus edit cannot quietly relax an exact entry.
//!
//! An entry whose features the decoder does not implement comes back as
//! `Error::Unsupported` and reports as *not yet decoded*. Every current entry
//! decodes — [`IN_CLASS`] holds all 23 — and that list is the ratchet: a
//! regression that turns an entry red, or a corpus refresh that adds one,
//! fails this test until the list is updated.
//!
//! The reusable machinery — the PGX reader, the PAE/MSE comparator, the
//! panic-guarded decode-and-classify, and the ratchet driver — lives in
//! [`support::part4`] and [`support::pgx`], shared with the corpus integrity
//! check and the future JP2 (Phase 3) and HTJ2K (Phase 4) conformance sets.

mod support;

use std::path::PathBuf;

use support::part4::{parse_every_reference, run_ratchet};

/// The conformance entries that decode within their compliance class today.
///
/// Every other entry must report as *not yet decoded*. Grow this list as the
/// Phase 2 milestones land; never shrink it without an explanation, because a
/// shrink is a regression.
const IN_CLASS: &[&str] = &[
    "p0_01", "p0_02", "p0_05", "p0_06", "p0_08", "p0_09", "p0_10", "p0_11", "p0_12", "p0_14",
    "p0_03", "p0_04", "p0_07", "p0_13", "p0_15", "p0_16", "p1_01", "p1_02", "p1_04", "p1_07",
    "p1_06", "p1_03", "p1_05",
];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

/// Decode every Part 4 entry and grade it against its compliance class.
///
/// Fails on any entry that faults, and on any drift between the entries that
/// actually decode in class and [`IN_CLASS`]. Entries the decoder does not
/// implement yet report as *not yet decoded* and are not failures.
#[test]
fn grades_the_conformance_corpus_by_compliance_class() {
    run_ratchet(&corpus_dir(), IN_CLASS);
}

/// Every reference image in the corpus parses, so the PGX reader is exercised
/// over all the header spellings the corpus carries — not just the ones on the
/// handful of entries that decode today. The class-0 references are included
/// because they cover spellings the class-1 set does not, notably the three
/// files written with CRLF line endings.
#[test]
fn every_reference_image_parses() {
    assert_eq!(
        parse_every_reference(&corpus_dir()),
        78,
        "the corpus carries 78 reference images"
    );
}

/// The default options are full resolution: `decode` and `decode_with` at
/// reduction 0 are the same decode.
#[test]
fn decode_with_no_reduction_equals_decode() {
    let bytes = std::fs::read(corpus_dir().join("codestreams/p0_01.j2k")).expect("read p0_01");
    let full = rust_j2k::decode(&bytes).expect("p0_01 decodes");
    let with = rust_j2k::decode_with(&bytes, rust_j2k::DecodeOptions::default())
        .expect("p0_01 decodes with default options");
    assert_eq!(full, with);
}

/// A reduction must leave every component at least its coarsest resolution;
/// one that consumes a whole pyramid is rejected, as OpenJPEG rejects it, and
/// one under the limit halves each axis per level, rounding up.
#[test]
fn reduction_is_bounded_by_the_resolution_count() {
    let bytes = std::fs::read(corpus_dir().join("codestreams/p0_01.j2k")).expect("read p0_01");
    let full = rust_j2k::decode(&bytes).expect("p0_01 decodes");

    let reduced = rust_j2k::decode_with(
        &bytes,
        rust_j2k::DecodeOptions::default().with_resolution_reduction(1),
    )
    .expect("one level fewer still decodes");
    assert_eq!(reduced.width, full.width.div_ceil(2));
    assert_eq!(reduced.height, full.height.div_ceil(2));
    assert_eq!(
        reduced.components[0].width,
        full.components[0].width.div_ceil(2)
    );
    assert_eq!(
        reduced.components[0].height,
        full.components[0].height.div_ceil(2)
    );

    // p0_01 has 3 decomposition levels: 4 resolutions, so 4 is one too many.
    let err = rust_j2k::decode_with(
        &bytes,
        rust_j2k::DecodeOptions::default().with_resolution_reduction(4),
    )
    .expect_err("a reduction past the coarsest resolution is rejected");
    assert!(matches!(err, rust_j2k::Error::InvalidOptions(_)), "{err:?}");
}
