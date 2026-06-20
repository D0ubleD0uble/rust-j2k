//! Shared test support: the `<name>.expected.json` oracle-snapshot schema.
//!
//! Each fixture under `tests/fixtures/` ships a sibling `<name>.expected.json`
//! holding what a trusted decoder (OpenJPEG, or eccodes for GRIB2-sourced
//! files) produced for it: the decoded samples, their geometry, the agreement
//! tolerance, and the provenance that lets the snapshot be regenerated. The
//! conformance harness deserializes one of these and compares it against our
//! own `decode` output. See `tests/fixtures/README.md` for the documented
//! schema and `docs/correctness.md` for how the corpus is graded.
//!
//! This module owns the schema only — loading fixtures and comparing samples is
//! the harness's job (a later issue).

use serde::{Deserialize, Serialize};

/// One fixture's oracle snapshot: the decode a trusted reference produced, plus
/// everything needed to grade our decode against it and to reproduce the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    /// Image geometry the samples are laid out against.
    pub geometry: Geometry,
    /// How close our decode must be to count as correct.
    pub tolerance: Tolerance,
    /// `width * height` reference samples, row-major. `i32` mirrors
    /// [`rust_j2k::Image::samples`], so the harness compares like-for-like.
    pub samples: Vec<i32>,
    /// Where the fixture came from and how to regenerate this snapshot.
    pub provenance: Provenance,
}

/// Sample-grid geometry, matching the SIZ-declared component shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    /// Bits per sample as declared in SIZ (1..=32 for the GRIB2 subset).
    pub bit_depth: u8,
    /// Whether samples are signed (SIZ component sign bit).
    pub signed: bool,
}

/// How exactly a decode must match the oracle.
///
/// Serializes as a tagged object so both arms read the same in JSON:
///
/// ```json
/// "tolerance": { "mode": "exact" }
/// "tolerance": { "mode": "absolute", "max_abs_error": 1.0 }
/// ```
///
/// Reversible (5/3, lossless) fixtures use [`Tolerance::Exact`]; irreversible
/// (9/7, lossy) fixtures use [`Tolerance::Absolute`] with a per-sample bound.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Tolerance {
    /// Every sample must equal the oracle exactly (bit-exact lossless).
    Exact,
    /// Each sample may differ from the oracle by at most `max_abs_error` in
    /// absolute value.
    Absolute { max_abs_error: f64 },
}

/// Where a fixture came from and the exact command that regenerates its oracle,
/// so the corpus stays reproducible without the reference decoder installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// The source the codestream was taken from, e.g.
    /// `"fieldglass/jpeg2000_regular_latlon.grib2"`.
    pub source: String,
    /// The exact command that regenerates this snapshot, e.g. an
    /// `opj_decompress …` or eccodes invocation.
    pub oracle_command: String,
    /// Optional free-form note (how the source was obtained, caveats).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Expected {
    /// Parse an `<name>.expected.json` snapshot from its JSON text.
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text)
    }
}

impl Geometry {
    /// Number of samples a correct snapshot must carry (`width * height`).
    pub fn sample_count(self) -> usize {
        self.width as usize * self.height as usize
    }
}
