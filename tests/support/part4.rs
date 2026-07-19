//! ISO/IEC 15444-4 compliance-class grading: the manifest schema, the PAE/MSE
//! comparator, the panic-guarded decode-and-classify, and the ratchet driver.
//!
//! Part 4 defines decoder conformance not as bit-exact equality — a lossy
//! decoder legitimately differs from the reference — but as staying within a
//! bounded **peak absolute error** (PAE) and a bounded **mean-squared error**
//! (MSE) against the reference decoded image, at a stated compliance class.
//!
//! The machinery here is corpus-agnostic: the Part 4 test binary
//! (`tests/conformance_part4.rs`) supplies the corpus path and its `IN_CLASS`
//! ratchet list, and the JP2 (Phase 3) and HTJ2K (Phase 4) conformance sets
//! will drive the same schema, comparator, and ratchet rather than copying them.

use std::collections::BTreeSet;
use std::fmt;
use std::panic;
use std::path::Path;

use rust_j2k::{Component, Error};
use serde::Deserialize;

use super::pgx::{Pgx, parse_pgx};

// ---------------------------------------------------------------------------
// The corpus manifest. This is the single typed source of the `manifest.json`
// schema: the grading harness reads the subset it needs, and the corpus
// integrity check (`tests/conformance_corpus.rs`) deserializes the same structs
// with `deny_unknown_fields`, so a schema drift fails there instead of being
// silently ignored.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Free-form banner describing the corpus; carried so the schema is exhaustive.
    #[serde(rename = "_comment")]
    pub comment: String,
    /// The compliance class the bounds are quoted at. We grade class 1.
    pub compliance_class: u8,
    /// Corpus-level provenance (sources, licenses). Not graded, so it is kept
    /// as opaque JSON rather than modelled field by field.
    pub provenance: serde_json::Value,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Corpus-relative path to the codestream, e.g. `codestreams/p0_09.j2k`.
    pub codestream: String,
    /// The Part 1 profile the codestream targets (0 or 1).
    pub profile: u32,
    /// The entry's ordinal within its profile (`p0_09` is index 9).
    pub index: u32,
    /// How many leading components carry class-1 references and bounds. May be
    /// fewer than the image's components: `p0_13` has 257 and grades 4.
    pub graded_components: usize,
    /// Whether the decode must equal the reference exactly.
    pub bit_exact: bool,
    /// The resolution reduction the class-1 references were decoded at
    /// (OpenJPEG's `C1P0_ResFactor_list`; only `p0_08` reduces). Grading
    /// decodes at the same reduction, or the geometries cannot meet. Required,
    /// not defaulted: a corpus refresh must record it for every entry.
    pub reduction: u8,
    /// The codestream's structural features. Not consumed by grading, but the
    /// corpus integrity check keys the Phase 2 feature matrix off these fields.
    pub features: Features,
    pub references: References,
    pub bounds_class1: Bounds,
    /// Class-0 bounds for the first component. Parsed for schema completeness;
    /// grading uses the per-component class-1 bounds.
    pub bounds_class0_first_component: Bounds,
    /// Whether the entry is in scope for the Phase 2 milestones.
    pub phase2_in_scope: bool,
}

/// The structural features of a corpus codestream, as recorded in the manifest.
///
/// These drive the corpus integrity check's coverage matrix; grading ignores
/// them. Every field is modelled so `deny_unknown_fields` stays exhaustive.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Features {
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub subsampling: Vec<[u32; 2]>,
    pub precision: Vec<u32>,
    pub signed: Vec<bool>,
    pub tiles: [u32; 2],
    pub progression: String,
    pub layers: u32,
    pub mct: u32,
    pub resolutions: u32,
    pub code_block: [u32; 2],
    pub reversible: bool,
    pub markers_main: Vec<String>,
    pub markers_tile: Vec<String>,
    pub tile_parts: u32,
    pub sop: bool,
    pub eph: bool,
    pub precincts: bool,
    pub cblksty: Cblksty,
}

/// Code-block style flags (the SPcod/SPcoc `cblksty` bits).
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Cblksty {
    pub bypass: bool,
    pub reset: bool,
    pub restart: bool,
    pub vert_causal: bool,
    pub pred_term: bool,
    pub segsym: bool,
}

impl Cblksty {
    /// The named flags in bit order, for iterating the code-block style bits.
    pub const NAMES: [&'static str; 6] = [
        "bypass",
        "reset",
        "restart",
        "vert_causal",
        "pred_term",
        "segsym",
    ];

    /// Look up a flag by its manifest name. Panics on an unknown name, so a
    /// typo in a caller's bit list is caught rather than silently read as false.
    pub fn flag(&self, name: &str) -> bool {
        match name {
            "bypass" => self.bypass,
            "reset" => self.reset,
            "restart" => self.restart,
            "vert_causal" => self.vert_causal,
            "pred_term" => self.pred_term,
            "segsym" => self.segsym,
            other => panic!("unknown cblksty flag `{other}`"),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct References {
    /// The class-0 references. Not graded against (class 0 bounds only the
    /// first component), but parsed by [`parse_every_reference`] because they
    /// carry header spellings the class-1 set does not.
    pub class0: Vec<String>,
    /// One `.pgx` per graded component, in component order. The grading bar.
    pub class1: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    /// Peak absolute error bound, per graded component.
    pub pae: Vec<f64>,
    /// Mean-squared error bound, per graded component.
    pub mse: Vec<f64>,
}

impl Entry {
    /// The entry's short name (`p0_09`), used in reports and in the ratchet list.
    pub fn name(&self) -> &str {
        Path::new(&self.codestream)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.codestream)
    }

    /// The PAE/MSE bound for graded component `index`.
    ///
    /// A `bit_exact` entry is pinned to zero on both axes whatever the manifest
    /// says, so exactness is graded off the flag and cannot be loosened by a
    /// corpus edit.
    pub fn bound(&self, index: usize) -> Bound {
        if self.bit_exact {
            return Bound { pae: 0.0, mse: 0.0 };
        }
        Bound {
            pae: self.bounds_class1.pae[index],
            mse: self.bounds_class1.mse[index],
        }
    }
}

/// The class-1 error bounds for one component.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bound {
    pub pae: f64,
    pub mse: f64,
}

// ---------------------------------------------------------------------------
// The comparator: peak absolute error and mean-squared error.
// ---------------------------------------------------------------------------

/// The class-1 error metrics for one component against its reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Peak absolute error: the largest `|actual - reference|` over all samples.
    pub pae: f64,
    /// Mean-squared error: the mean of `(actual - reference)^2`.
    pub mse: f64,
}

impl Metrics {
    /// Whether both axes sit inside `bound` (which is `0/0` for an exact entry).
    pub fn within(self, bound: Bound) -> bool {
        self.pae <= bound.pae && self.mse <= bound.mse
    }

    /// Whether the decode reproduced the reference sample for sample. Both
    /// metrics are non-negative, so this is exactly the zero bound.
    pub fn exact(self) -> bool {
        self.within(Bound { pae: 0.0, mse: 0.0 })
    }
}

impl fmt::Display for Metrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PAE {:.0}, MSE {:.4}", self.pae, self.mse)
    }
}

/// Compute PAE and MSE between equal-length sample vectors.
///
/// Differences are taken in `i64` (the operands are `i32`) and squares summed in
/// `i128`, so neither the subtraction nor the accumulation can overflow at the
/// `i32` extremes over a full-size component. An empty component has no error.
pub fn metrics(actual: &[i32], reference: &[i32]) -> Metrics {
    debug_assert_eq!(actual.len(), reference.len());
    if actual.is_empty() {
        return Metrics { pae: 0.0, mse: 0.0 };
    }

    let mut pae = 0i64;
    let mut squares = 0i128;
    for (&got, &want) in actual.iter().zip(reference) {
        let diff = (got as i64 - want as i64).abs();
        pae = pae.max(diff);
        squares += (diff as i128) * (diff as i128);
    }

    Metrics {
        pae: pae as f64,
        mse: squares as f64 / actual.len() as f64,
    }
}

/// Grade one decoded component against its class-1 reference.
///
/// The reference fixes the component's geometry, depth, and sign, so a
/// disagreement there is a decode fault reported before any sample is compared:
/// PAE/MSE over mismatched grids would be meaningless.
pub fn grade_component(
    actual: &Component,
    reference: &Pgx,
    bound: Bound,
) -> Result<Metrics, String> {
    if (actual.width, actual.height) != (reference.width, reference.height) {
        return Err(format!(
            "geometry: reference is {}x{}, decoded {}x{}",
            reference.width, reference.height, actual.width, actual.height
        ));
    }
    if (actual.bit_depth, actual.signed) != (reference.bit_depth, reference.signed) {
        return Err(format!(
            "format: reference is depth {} signed {}, decoded depth {} signed {}",
            reference.bit_depth, reference.signed, actual.bit_depth, actual.signed
        ));
    }
    // Matching geometry already implies matching sample counts, but `metrics`
    // zips the two vectors: a short one would silently grade only its prefix.
    if actual.samples.len() != reference.samples.len() {
        return Err(format!(
            "sample count: reference has {}, decoded {}",
            reference.samples.len(),
            actual.samples.len()
        ));
    }

    let measured = metrics(&actual.samples, &reference.samples);
    if !measured.within(bound) {
        return Err(format!(
            "out of class: {measured} exceeds bound PAE {:.0}, MSE {:.4}",
            bound.pae, bound.mse
        ));
    }
    Ok(measured)
}

// ---------------------------------------------------------------------------
// Per-entry grading.
// ---------------------------------------------------------------------------

/// How one corpus entry graded.
#[derive(Debug, Clone, PartialEq)]
pub enum Grade {
    /// Decoded and reproduced every graded component exactly.
    Exact,
    /// Decoded within the entry's compliance-class PAE/MSE bounds.
    WithinClass,
    /// The decoder rejected a feature it does not implement yet
    /// (`Error::Unsupported`). Expected while a phase is still in flight.
    NotYetDecoded(String),
    /// A genuine fault: a decode error other than `Unsupported`, a panic, a
    /// geometry disagreement, or samples outside the compliance class.
    Failed(String),
}

impl Grade {
    /// Whether the entry decoded and graded inside its class.
    pub fn is_in_class(&self) -> bool {
        matches!(self, Grade::Exact | Grade::WithinClass)
    }

    /// The status word reported for this entry.
    pub fn label(&self) -> &'static str {
        match self {
            Grade::Exact => "pass (bit-exact)",
            Grade::WithinClass => "pass (within class)",
            Grade::NotYetDecoded(_) => "not yet decoded",
            Grade::Failed(_) => "FAIL",
        }
    }
}

/// Decode one entry and grade every component the corpus grades.
///
/// A decode that returns `Error::Unsupported` reports as [`Grade::NotYetDecoded`]
/// — a feature the decoder has not implemented yet. Every other error variant,
/// including the `Error::Limit`/`Error::InvalidOptions` decode guards, is a
/// [`Grade::Failed`]: a guard tripping on a conformance entry is a fault, not a
/// not-yet-decoded feature.
pub fn grade_entry(dir: &Path, entry: &Entry) -> Grade {
    let graded = entry.graded_components;
    // An entry that grades nothing would compare nothing and report `Exact`.
    // Reject it, so a corpus edit cannot turn an entry into a vacuous pass.
    if graded == 0 {
        return Grade::Failed("manifest: entry grades no components".into());
    }
    if entry.references.class1.len() != graded
        || entry.bounds_class1.pae.len() != graded
        || entry.bounds_class1.mse.len() != graded
    {
        return Grade::Failed(format!(
            "manifest: {graded} graded components but {} references, {} PAE, {} MSE bounds",
            entry.references.class1.len(),
            entry.bounds_class1.pae.len(),
            entry.bounds_class1.mse.len(),
        ));
    }

    let bytes = match std::fs::read(dir.join(&entry.codestream)) {
        Ok(bytes) => bytes,
        Err(e) => return Grade::Failed(format!("cannot read {}: {e}", entry.codestream)),
    };

    // `decode_with` promises a typed error for every input, so a panic here is
    // a contract violation, not a not-yet-implemented feature. Catch it so one
    // bad entry reports as a failure instead of aborting the whole harness.
    // The entry's recorded reduction is the one its references were decoded
    // at, so grading decodes there too (`-r` in OpenJPEG's conformance runs).
    let options = rust_j2k::DecodeOptions::default().with_resolution_reduction(entry.reduction);
    let decoded = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        rust_j2k::decode_with(&bytes, options)
    }));
    let image = match decoded {
        Err(_) => return Grade::Failed("panicked (decode must return a typed error)".into()),
        Ok(Err(Error::Unsupported(what))) => return Grade::NotYetDecoded(what),
        Ok(Err(e)) => return Grade::Failed(format!("decoder rejected a valid codestream: {e}")),
        Ok(Ok(image)) => image,
    };

    if image.components.len() < graded {
        return Grade::Failed(format!(
            "decoded {} components but the corpus grades {graded}",
            image.components.len()
        ));
    }

    let mut all_exact = true;
    for (index, reference_path) in entry.references.class1.iter().enumerate() {
        let raw = match std::fs::read(dir.join(reference_path)) {
            Ok(raw) => raw,
            Err(e) => return Grade::Failed(format!("cannot read {reference_path}: {e}")),
        };
        let reference = match parse_pgx(&raw) {
            Ok(reference) => reference,
            Err(e) => return Grade::Failed(format!("cannot parse {reference_path}: {e}")),
        };

        let component = &image.components[index];
        match grade_component(component, &reference, entry.bound(index)) {
            Ok(measured) => all_exact &= measured.exact(),
            Err(why) => return Grade::Failed(format!("component {index}: {why}")),
        }
    }

    if all_exact {
        Grade::Exact
    } else {
        Grade::WithinClass
    }
}

/// Read and parse the corpus manifest, panicking with the offending path on any
/// I/O or schema error.
pub fn load_manifest(dir: &Path) -> Manifest {
    let path = dir.join("manifest.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// The ratchet driver.
// ---------------------------------------------------------------------------

/// Decode every corpus entry, grade it against its compliance class, and enforce
/// the ratchet against `in_class`.
///
/// Fails on any entry that faults, and on any drift between the entries that
/// actually decode in class and `in_class` — a regression that turns an entry
/// red, or a corpus refresh that adds one, fails until the list is updated.
/// Entries the decoder does not implement yet report as *not yet decoded* and
/// are not failures.
pub fn run_ratchet(dir: &Path, in_class: &[&str]) {
    let manifest = load_manifest(dir);
    assert_eq!(
        manifest.compliance_class, 1,
        "the harness grades the per-component class-1 bounds"
    );

    let graded: Vec<(&str, Grade)> = manifest
        .entries
        .iter()
        .map(|entry| (entry.name(), grade_entry(dir, entry)))
        .collect();

    eprintln!(
        "\nISO/IEC 15444-4 class-{} grading:",
        manifest.compliance_class
    );
    for (name, grade) in &graded {
        match grade {
            Grade::NotYetDecoded(what) => eprintln!("  {name:<8} {:<20} {what}", grade.label()),
            Grade::Failed(why) => eprintln!("  {name:<8} {:<20} {why}", grade.label()),
            _ => eprintln!("  {name:<8} {}", grade.label()),
        }
    }
    let in_class_count = graded.iter().filter(|(_, g)| g.is_in_class()).count();
    eprintln!(
        "  ── {in_class_count}/{} in class, {} not yet decoded\n",
        graded.len(),
        graded.len() - in_class_count,
    );

    let failures: Vec<String> = graded
        .iter()
        .filter_map(|(name, grade)| match grade {
            Grade::Failed(why) => Some(format!("  {name}: {why}")),
            _ => None,
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} conformance entries failed:\n{}",
        failures.len(),
        failures.join("\n"),
    );

    let actual: BTreeSet<&str> = graded
        .iter()
        .filter(|(_, g)| g.is_in_class())
        .map(|(name, _)| *name)
        .collect();
    let expected: BTreeSet<&str> = in_class.iter().copied().collect();
    assert_eq!(
        actual,
        expected,
        "the set of entries decoding in class changed.\n  \
         newly in class (add to IN_CLASS): {:?}\n  \
         no longer in class (a regression): {:?}",
        actual.difference(&expected).collect::<Vec<_>>(),
        expected.difference(&actual).collect::<Vec<_>>(),
    );
}

/// Parse every reference image the corpus names (class-1 and class-0), asserting
/// each `.pgx` parses and its sample count matches its header. Returns the number
/// of references parsed so the caller can pin the corpus size.
pub fn parse_every_reference(dir: &Path) -> usize {
    let mut parsed = 0;
    for entry in load_manifest(dir).entries {
        let references = entry.references;
        for reference in references.class1.iter().chain(&references.class0) {
            let raw = std::fs::read(dir.join(reference)).expect("read reference");
            let pgx = parse_pgx(&raw).unwrap_or_else(|e| panic!("{reference}: {e}"));
            assert_eq!(
                pgx.samples.len(),
                pgx.width as usize * pgx.height as usize,
                "{reference}: sample count disagrees with its header",
            );
            parsed += 1;
        }
    }
    parsed
}

// ---------------------------------------------------------------------------
// Unit tests: the PAE/MSE comparator and per-entry grading.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_on_a_known_image_pair() {
        // Differences 0, 1, 2, 3: PAE = 3, MSE = (0 + 1 + 4 + 9) / 4 = 3.5.
        let measured = metrics(&[10, 11, 12, 13], &[10, 10, 10, 10]);
        assert_eq!(measured.pae, 3.0);
        assert_eq!(measured.mse, 3.5);
        assert!(!measured.exact());

        // Sign of the difference does not matter; the errors are symmetric.
        assert_eq!(metrics(&[7, 13], &[10, 10]), metrics(&[13, 7], &[10, 10]));
    }

    #[test]
    fn metrics_of_an_identical_pair_are_zero() {
        let measured = metrics(&[-5, 0, 5], &[-5, 0, 5]);
        assert_eq!((measured.pae, measured.mse), (0.0, 0.0));
        assert!(measured.exact());
        assert!(measured.within(Bound { pae: 0.0, mse: 0.0 }));
    }

    /// The `i32` extremes must not overflow the difference or the accumulator.
    #[test]
    fn metrics_survive_the_i32_extremes() {
        let measured = metrics(&[i32::MIN, i32::MAX], &[i32::MAX, i32::MIN]);
        let span = (i32::MAX as i64 - i32::MIN as i64) as f64;
        assert_eq!(measured.pae, span);
        assert_eq!(measured.mse, span * span);
    }

    #[test]
    fn metrics_of_an_empty_component_are_zero() {
        assert!(metrics(&[], &[]).exact());
    }

    fn component(samples: Vec<i32>) -> Component {
        Component::new(samples.len() as u32, 1, 8, false, 1, 1, samples)
    }

    fn reference(samples: Vec<i32>) -> Pgx {
        Pgx {
            width: samples.len() as u32,
            height: 1,
            bit_depth: 8,
            signed: false,
            samples,
        }
    }

    #[test]
    fn grading_accepts_a_component_inside_its_bounds() {
        let measured = grade_component(
            &component(vec![10, 12]),
            &reference(vec![10, 10]),
            Bound { pae: 2.0, mse: 2.0 },
        )
        .unwrap();
        assert_eq!(measured.pae, 2.0);
        assert_eq!(measured.mse, 2.0); // (0 + 4) / 2
    }

    /// Both axes bind independently: a component can sit inside the MSE bound
    /// (few large errors averaged away) yet blow the PAE bound, and vice versa.
    #[test]
    fn grading_rejects_a_breach_of_either_bound() {
        // PAE 4 breaches a PAE bound of 3, though MSE 8 is inside its bound.
        let err = grade_component(
            &component(vec![14, 10]),
            &reference(vec![10, 10]),
            Bound { pae: 3.0, mse: 9.0 },
        )
        .unwrap_err();
        assert!(err.contains("out of class"), "got {err}");

        // MSE 4 breaches an MSE bound of 3, though PAE 2 is inside its bound.
        let err = grade_component(
            &component(vec![12, 12]),
            &reference(vec![10, 10]),
            Bound { pae: 2.0, mse: 3.0 },
        )
        .unwrap_err();
        assert!(err.contains("out of class"), "got {err}");
    }

    #[test]
    fn grading_rejects_a_geometry_or_format_disagreement() {
        let err = grade_component(
            &component(vec![10, 10, 10]),
            &reference(vec![10, 10]),
            Bound { pae: 0.0, mse: 0.0 },
        )
        .unwrap_err();
        assert!(err.contains("geometry"), "got {err}");

        let mut wrong_depth = reference(vec![10, 10]);
        wrong_depth.bit_depth = 12;
        let err = grade_component(
            &component(vec![10, 10]),
            &wrong_depth,
            Bound { pae: 0.0, mse: 0.0 },
        )
        .unwrap_err();
        assert!(err.contains("format"), "got {err}");
    }

    /// Geometry can agree while the sample vectors do not; grading the common
    /// prefix would pass a truncated decode.
    #[test]
    fn grading_rejects_a_short_sample_vector() {
        let mut truncated = component(vec![10, 10]);
        truncated.samples.pop();
        let err = grade_component(
            &truncated,
            &reference(vec![10, 10]),
            Bound { pae: 0.0, mse: 0.0 },
        )
        .unwrap_err();
        assert!(err.contains("sample count"), "got {err}");
    }

    /// An entry that grades no components would compare nothing and fall
    /// through to `Exact`. It must fail instead, before any file is read.
    #[test]
    fn an_entry_grading_no_components_fails() {
        let entry = Entry {
            codestream: "codestreams/nonexistent.j2k".into(),
            graded_components: 0,
            bit_exact: true,
            ..Default::default()
        };
        let grade = grade_entry(Path::new("/nonexistent"), &entry);
        assert!(
            matches!(&grade, Grade::Failed(why) if why.contains("grades no components")),
            "got {grade:?}",
        );
    }

    /// A `bit_exact` entry is pinned to zero bounds even if the manifest were
    /// edited to quote a loose one, so exactness is graded off the flag.
    #[test]
    fn bit_exact_entries_ignore_their_recorded_bounds() {
        let entry = Entry {
            codestream: "codestreams/p0_09.j2k".into(),
            graded_components: 1,
            bit_exact: true,
            references: References {
                class1: vec!["references/c1p0_09_0.pgx".into()],
                class0: vec!["references/c0p0_09.pgx".into()],
            },
            bounds_class1: Bounds {
                pae: vec![5.0],
                mse: vec![5.0],
            },
            ..Default::default()
        };
        assert_eq!(entry.name(), "p0_09");
        assert_eq!(entry.bound(0), Bound { pae: 0.0, mse: 0.0 });

        // A lossy entry keeps the bounds the manifest records.
        let lossy = Entry {
            bit_exact: false,
            ..entry
        };
        assert_eq!(lossy.bound(0), Bound { pae: 5.0, mse: 5.0 });
    }
}
