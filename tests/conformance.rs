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
//! MRMS samples.

#![allow(unused)]

#[test]
#[ignore = "no fixtures yet — add real codestreams + an OpenJPEG/eccodes oracle"]
fn decodes_corpus_against_oracle() {
    // for each fixture:
    //   let bytes = std::fs::read(path).unwrap();
    //   let img = rust_j2k::decode(&bytes).expect("decode");
    //   assert_image_matches_oracle(&img, &oracle, tolerance);
}
