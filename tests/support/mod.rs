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
//! This module owns the schema *and* the harness that runs it: fixture
//! discovery ([`discover`]), the sample comparator ([`compare`]), and the
//! decode-outcome classifier ([`classify`]) the corpus test drives.
//!
//! It also carries the shared machinery for the ISO/IEC 15444-4 conformance
//! corpus: the PGX reference reader ([`pgx`]) and the manifest schema, PAE/MSE
//! comparator, and ratchet driver ([`part4`]). Each `tests/*.rs` file is its own
//! crate, so any given test binary uses only a slice of this module; the
//! module-wide `#![allow(dead_code)]` keeps the unused-in-this-binary slice from
//! tripping the `dead_code` lint.

#![allow(dead_code)]

pub mod part4;
pub mod pgx;

use std::fmt;
use std::panic;
use std::path::{Path, PathBuf};

use rust_j2k::{Error, Image};
use serde::{Deserialize, Serialize};

/// One fixture's oracle snapshot: the decode a trusted reference produced, plus
/// everything needed to grade our decode against it and to reproduce the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    /// The reference-grid image area the components are registered against.
    pub image: ImageGeometry,
    /// How close our decode must be to count as correct.
    pub tolerance: Tolerance,
    /// One entry per component, in SIZ order.
    pub components: Vec<ComponentSnapshot>,
    /// Where the fixture came from and how to regenerate this snapshot.
    pub provenance: Provenance,
}

/// The reference-grid image area (`Xsiz - XOsiz` by `Ysiz - YOsiz`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageGeometry {
    pub width: u32,
    pub height: u32,
}

/// One component's oracle samples and the geometry they are laid out against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSnapshot {
    pub width: u32,
    pub height: u32,
    /// Bits per sample as declared in SIZ.
    pub bit_depth: u8,
    /// Whether samples are signed (SIZ component sign bit).
    pub signed: bool,
    /// Sub-sampling factors (SIZ `XRsiz`/`YRsiz`); `1` means unit sampling.
    pub x_sampling: u8,
    pub y_sampling: u8,
    /// `width * height` reference samples, row-major. `i32` mirrors
    /// [`rust_j2k::Component::samples`], so the harness compares like-for-like.
    pub samples: Vec<i32>,
}

/// Sample-grid geometry, matching the SIZ-declared component shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    /// Bits per sample as declared in SIZ.
    pub bit_depth: u8,
    /// Whether samples are signed (SIZ component sign bit).
    pub signed: bool,
}

impl ComponentSnapshot {
    /// This component's geometry, for comparing against a decoded component.
    pub fn geometry(&self) -> Geometry {
        Geometry {
            width: self.width,
            height: self.height,
            bit_depth: self.bit_depth,
            signed: self.signed,
        }
    }
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

// ---------------------------------------------------------------------------
// Harness: discovery, comparison, and outcome classification.
// ---------------------------------------------------------------------------

/// One discovered fixture: a `<name>.j2k` codestream and the snapshot loaded
/// from its sibling `<name>.expected.json`.
///
/// `expected` is itself a `Result` so a fixture with a missing or malformed
/// snapshot is surfaced loudly (as a [`Outcome::LoadError`]) rather than
/// silently skipped — a broken corpus is a test failure, not a no-op.
#[derive(Debug)]
pub struct Fixture {
    /// Fixture stem (the file name without the `.j2k` extension).
    pub name: String,
    /// Path to the codestream passed to [`rust_j2k::decode`].
    pub codestream: PathBuf,
    /// The parsed oracle snapshot, or why it could not be loaded.
    pub expected: Result<Expected, String>,
}

/// The result of running one fixture through the decoder and comparator.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Decode produced an image that agrees with the oracle.
    Passed,
    /// Decode ran but disagreed with the oracle, the decoder rejected a
    /// known-good fixture, or the decode panicked (a contract violation:
    /// `decode` promises a typed error for every input). The [`Mismatch`]
    /// carries a readable diff.
    Failed(Mismatch),
    /// The fixture itself is broken: its snapshot is missing, unreadable, or
    /// does not parse. Distinct from `Failed` so a corpus bug is not mistaken
    /// for a decoder bug.
    LoadError(String),
}

/// A readable account of why a decode disagreed with its oracle: either the
/// geometry differs, or the first sample that falls outside tolerance.
#[derive(Debug, Clone, PartialEq)]
pub enum Mismatch {
    /// A component's decoded geometry differs from the oracle's.
    Geometry {
        component: usize,
        expected: Geometry,
        actual: Geometry,
    },
    /// A sample differs by more than the allowed bound. Reports the *first*
    /// such sample in row-major order so a failure points at one coordinate.
    Sample {
        component: usize,
        index: usize,
        x: u32,
        y: u32,
        expected: i32,
        actual: i32,
        /// Allowed absolute error (`0.0` for an exact/reversible fixture).
        bound: f64,
    },
    /// The decode produced an error instead of an image, or disagreed with the
    /// oracle about the image's shape rather than its samples.
    DecodeError(String),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mismatch::Geometry {
                component,
                expected,
                actual,
            } => write!(
                f,
                "component {component} geometry mismatch: expected {}x{} (depth {}, signed {}), \
                 got {}x{} (depth {}, signed {})",
                expected.width,
                expected.height,
                expected.bit_depth,
                expected.signed,
                actual.width,
                actual.height,
                actual.bit_depth,
                actual.signed,
            ),
            Mismatch::Sample {
                component,
                index,
                x,
                y,
                expected,
                actual,
                bound,
            } => write!(
                f,
                "component {component} sample #{index} at ({x},{y}): expected {expected}, \
                 got {actual} (|diff| = {} exceeds bound {bound})",
                (*actual as i64 - *expected as i64).abs(),
            ),
            Mismatch::DecodeError(msg) => {
                write!(f, "decoder rejected a known-good fixture: {msg}")
            }
        }
    }
}

/// Discover every `<name>.j2k` fixture in `dir`, pairing each with the snapshot
/// from its sibling `<name>.expected.json`.
///
/// Fixtures are returned sorted by name for deterministic test output. A
/// missing `dir` yields an empty list (the corpus simply has nothing yet); a
/// codestream whose snapshot is missing or unparseable is still returned, with
/// the reason captured in [`Fixture::expected`].
pub fn discover(dir: &Path) -> Vec<Fixture> {
    let mut fixtures: Vec<Fixture> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "j2k"))
            .map(|codestream| {
                let name = codestream
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("<non-utf8>")
                    .to_string();
                let snapshot = codestream.with_extension("expected.json");
                let expected = load_snapshot(&snapshot);
                Fixture {
                    name,
                    codestream,
                    expected,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    fixtures.sort_by(|a, b| a.name.cmp(&b.name));
    fixtures
}

/// Read and parse one `<name>.expected.json`, mapping every failure to a
/// human-readable reason string.
///
/// Also checks the snapshot is internally consistent — `samples.len()` must
/// equal the declared `width * height` — so a malformed oracle is caught here
/// (as a [`Outcome::LoadError`]) rather than silently under-comparing later.
fn load_snapshot(path: &Path) -> Result<Expected, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let expected =
        Expected::from_json(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    if expected.components.is_empty() {
        return Err(format!("malformed {}: no components", path.display()));
    }
    for (index, component) in expected.components.iter().enumerate() {
        let want = component.geometry().sample_count();
        if component.samples.len() != want {
            return Err(format!(
                "malformed {}: component {index} is {}x{} = {want} samples but the array has {}",
                path.display(),
                component.width,
                component.height,
                component.samples.len(),
            ));
        }
    }
    Ok(expected)
}

/// Compare a decoded [`Image`] against an oracle snapshot.
///
/// The image area, the component count, and each component's geometry must match
/// exactly; then each sample must agree within the snapshot's tolerance —
/// bit-exact for [`Tolerance::Exact`], within an absolute bound for
/// [`Tolerance::Absolute`]. Returns the first divergence so a failure localises
/// to a single cause.
pub fn compare(actual: &Image, expected: &Expected) -> Result<(), Mismatch> {
    let actual_image = ImageGeometry {
        width: actual.width,
        height: actual.height,
    };
    if actual_image != expected.image {
        return Err(Mismatch::DecodeError(format!(
            "image area is {}x{} but the snapshot says {}x{}",
            actual.width, actual.height, expected.image.width, expected.image.height,
        )));
    }
    if actual.components.len() != expected.components.len() {
        return Err(Mismatch::DecodeError(format!(
            "decoded {} components but the snapshot has {}",
            actual.components.len(),
            expected.components.len(),
        )));
    }

    for (component_index, (component, want)) in actual
        .components
        .iter()
        .zip(&expected.components)
        .enumerate()
    {
        compare_component(component_index, component, want, expected.tolerance)?;
    }
    Ok(())
}

/// Compare one decoded component against its snapshot.
fn compare_component(
    component: usize,
    actual: &rust_j2k::Component,
    expected: &ComponentSnapshot,
    tolerance: Tolerance,
) -> Result<(), Mismatch> {
    let actual_geometry = Geometry {
        width: actual.width,
        height: actual.height,
        bit_depth: actual.bit_depth,
        signed: actual.signed,
    };
    if actual_geometry != expected.geometry() {
        return Err(Mismatch::Geometry {
            component,
            expected: expected.geometry(),
            actual: actual_geometry,
        });
    }
    if (actual.x_sampling, actual.y_sampling) != (expected.x_sampling, expected.y_sampling) {
        return Err(Mismatch::DecodeError(format!(
            "component {component} sub-samples ({}, {}) but the snapshot says ({}, {})",
            actual.x_sampling, actual.y_sampling, expected.x_sampling, expected.y_sampling,
        )));
    }
    // Matching geometry already implies matching sample counts, but the loop
    // below zips the two vectors: a short one would compare only its prefix.
    if actual.samples.len() != expected.samples.len() {
        return Err(Mismatch::DecodeError(format!(
            "component {component} decoded {} samples but the snapshot has {}",
            actual.samples.len(),
            expected.samples.len(),
        )));
    }

    let bound = match tolerance {
        Tolerance::Exact => 0.0,
        Tolerance::Absolute { max_abs_error } => max_abs_error,
    };
    let width = expected.width;
    for (index, (&got, &want)) in actual.samples.iter().zip(&expected.samples).enumerate() {
        // i64 so the subtraction cannot overflow at the i32 extremes.
        let diff = (got as i64 - want as i64).abs() as f64;
        if diff > bound {
            return Err(Mismatch::Sample {
                component,
                index,
                x: (index as u32) % width,
                y: (index as u32) / width,
                expected: want,
                actual: got,
                bound,
            });
        }
    }
    Ok(())
}

/// Map the result of a (panic-guarded) decode to an [`Outcome`].
///
/// - a panic ⇒ `Failed`: `decode` promises a typed error for every input, so
///   a panic is a contract violation, not a not-yet-implemented feature;
/// - `Ok(image)` ⇒ compared against the oracle ⇒ `Passed` or `Failed`;
/// - `Err(e)` ⇒ `Failed`, since the decoder rejected a fixture the oracle decoded.
pub fn classify(
    decoded: std::thread::Result<Result<Image, Error>>,
    expected: &Expected,
) -> Outcome {
    match decoded {
        Err(_) => Outcome::Failed(Mismatch::DecodeError(
            "panicked (decode must return a typed error for every input)".into(),
        )),
        Ok(Ok(image)) => match compare(&image, expected) {
            Ok(()) => Outcome::Passed,
            Err(mismatch) => Outcome::Failed(mismatch),
        },
        Ok(Err(e)) => Outcome::Failed(Mismatch::DecodeError(e.to_string())),
    }
}

/// Run one fixture end to end: load its bytes, decode under a panic guard, and
/// classify the result against its snapshot.
pub fn run_fixture(fixture: &Fixture) -> Outcome {
    let expected = match &fixture.expected {
        Ok(expected) => expected,
        Err(reason) => return Outcome::LoadError(reason.clone()),
    };
    let bytes = match std::fs::read(&fixture.codestream) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Outcome::LoadError(format!(
                "cannot read {}: {e}",
                fixture.codestream.display()
            ));
        }
    };

    // Catch a panic so one bad fixture reports as Failed instead of aborting
    // the whole suite. We deliberately leave the default panic hook in place —
    // the panic still prints one informative line to stderr — rather than
    // swapping the process-global hook, which races with other tests running
    // in parallel and could swallow a genuine panic's message.
    let decoded = panic::catch_unwind(panic::AssertUnwindSafe(|| rust_j2k::decode(&bytes)));

    classify(decoded, expected)
}
