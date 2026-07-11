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
//! Entries whose features the decoder does not implement yet come back as
//! `Error::Unsupported` and report as *not yet decoded* — expected while the
//! Phase 2 milestones land, and the reason this harness is useful from day one.
//! [`IN_CLASS`] is the ratchet: it names exactly the entries that decode today,
//! so a milestone that turns one green, or a regression that turns one red,
//! fails this test until the list is updated.

use std::collections::BTreeSet;
use std::fmt;
use std::panic;
use std::path::{Path, PathBuf};

use rust_j2k::{Component, Error};
use serde::Deserialize;

/// The conformance entries that decode within their compliance class today.
///
/// Every other entry must report as *not yet decoded*. Grow this list as the
/// Phase 2 milestones land; never shrink it without an explanation, because a
/// shrink is a regression.
const IN_CLASS: &[&str] = &[
    "p0_01", "p0_02", "p0_05", "p0_06", "p0_08", "p0_09", "p0_10", "p0_11", "p0_12", "p0_14",
    "p0_16", "p1_04",
];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance")
}

// ---------------------------------------------------------------------------
// The corpus manifest. Only the fields grading needs; `serde` ignores the rest
// (provenance, feature tags, class-0 bounds), so this is a view of the schema
// committed in `manifest.json`, not a parallel copy of it.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Manifest {
    /// The compliance class the bounds are quoted at. We grade class 1.
    compliance_class: u8,
    entries: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    /// Corpus-relative path to the codestream, e.g. `codestreams/p0_09.j2k`.
    codestream: String,
    /// How many leading components carry class-1 references and bounds. May be
    /// fewer than the image's components: `p0_13` has 257 and grades 4.
    graded_components: usize,
    /// Whether the decode must equal the reference exactly.
    bit_exact: bool,
    /// The resolution reduction the class-1 references were decoded at
    /// (OpenJPEG's `C1P0_ResFactor_list`; only `p0_08` reduces). Grading
    /// decodes at the same reduction, or the geometries cannot meet. Required,
    /// not defaulted: a corpus refresh must record it for every entry.
    reduction: u8,
    references: References,
    bounds_class1: Bounds,
}

#[derive(Debug, Deserialize)]
struct References {
    /// One `.pgx` per graded component, in component order. The grading bar.
    class1: Vec<String>,
    /// The class-0 references. Not graded against (class 0 bounds only the
    /// first component), but parsed by [`every_reference_image_parses`] because
    /// they carry header spellings the class-1 set does not.
    class0: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Bounds {
    /// Peak absolute error bound, per graded component.
    pae: Vec<f64>,
    /// Mean-squared error bound, per graded component.
    mse: Vec<f64>,
}

impl Entry {
    /// The entry's short name (`p0_09`), used in reports and in [`IN_CLASS`].
    fn name(&self) -> &str {
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
    fn bound(&self, index: usize) -> Bound {
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
struct Bound {
    pae: f64,
    mse: f64,
}

// ---------------------------------------------------------------------------
// PGX reader — the reference image format of the Part 4 corpus.
// ---------------------------------------------------------------------------

/// A decoded `.pgx` reference image: one component on its own sample grid.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pgx {
    width: u32,
    height: u32,
    bit_depth: u8,
    signed: bool,
    samples: Vec<i32>,
}

/// Parse a `.pgx` reference image.
///
/// The header is one ASCII line — `PG <endian> <depth> <width> <height>` — then
/// the samples, packed at the natural byte width for the depth (1 byte up to 8
/// bits, 2 up to 16, 4 beyond) in the declared byte order.
///
/// Four details the corpus actually exercises and a naive reader gets wrong:
///
/// - the sign is part of the depth token, and may be glued (`-4`, `+8`) or
///   separated (`+ 8`); it may also be absent (`8`), meaning unsigned;
/// - fields are separated by *runs* of whitespace (`PG ML  8 17 37`), so the
///   header must be tokenized, not split on single spaces;
/// - three references end their header with CRLF, so the header must be
///   whitespace-trimmed or the `\r` contaminates the height token;
/// - signed samples are sign-extended from the **storage byte width**, not from
///   the declared depth: `p0_03` is 4-bit signed stored one sample per `i8`.
///
/// The byte width follows OpenJPEG's writer: 1 byte to 8 bits, 2 to 16, 4
/// beyond. (This is `ceil(depth / 8)` only up to 16 bits; 17..=24 uses 4, not 3.)
///
/// `endian` is `ML` (big) or `LM` (little). Every reference in the corpus is
/// `ML`; `LM` is accepted because the format allows it.
fn parse_pgx(bytes: &[u8]) -> Result<Pgx, String> {
    let newline = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("no header line")?;
    let header = std::str::from_utf8(&bytes[..newline]).map_err(|_| "header is not UTF-8")?;

    let mut tokens = header.split_ascii_whitespace();
    match tokens.next() {
        Some("PG") => {}
        other => return Err(format!("bad magic: expected `PG`, got {other:?}")),
    }
    let big_endian = match tokens.next() {
        Some("ML") => true,
        Some("LM") => false,
        other => return Err(format!("bad byte order: expected `ML`/`LM`, got {other:?}")),
    };

    // The depth token, re-joined if the sign was written as its own token.
    let mut depth_token = tokens.next().ok_or("missing depth")?.to_string();
    if depth_token == "+" || depth_token == "-" {
        depth_token.push_str(tokens.next().ok_or("missing depth after sign")?);
    }
    let depth: i32 = depth_token
        .strip_prefix('+')
        .unwrap_or(&depth_token)
        .parse()
        .map_err(|_| format!("bad depth `{depth_token}`"))?;
    let signed = depth < 0;
    let bit_depth = depth.unsigned_abs();
    if !(1..=32).contains(&bit_depth) {
        return Err(format!("depth {bit_depth} outside 1..=32"));
    }

    let mut dimension = |what: &str| -> Result<u32, String> {
        tokens
            .next()
            .ok_or_else(|| format!("missing {what}"))?
            .parse()
            .map_err(|_| format!("bad {what}"))
    };
    let width = dimension("width")?;
    let height = dimension("height")?;

    let stride = match bit_depth {
        1..=8 => 1usize,
        9..=16 => 2,
        _ => 4,
    };
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("width * height overflows")?;
    let needed = count.checked_mul(stride).ok_or("body size overflows")?;

    // Exact, not `<`: every reference in the corpus is exactly its body, so a
    // short file is truncation and a long one is a header that disagrees with
    // its payload. Both should fail loudly rather than grade a prefix.
    let body = &bytes[newline + 1..];
    if body.len() != needed {
        return Err(format!(
            "body is {} bytes but {width}x{height} at {stride} byte(s)/sample needs {needed}",
            body.len()
        ));
    }

    let samples = body
        .chunks_exact(stride)
        .map(|chunk| {
            // Widen to `u32` most-significant byte first, then reinterpret at the
            // storage width so a signed sample sign-extends correctly.
            let fold = |acc: u32, &b: &u8| (acc << 8) | b as u32;
            let raw = if big_endian {
                chunk.iter().fold(0u32, fold)
            } else {
                chunk.iter().rev().fold(0u32, fold)
            };
            match (signed, stride) {
                // Samples land in `i32`, matching `rust_j2k::Component`. An
                // unsigned value past `i32::MAX` has no representation there, so
                // reject it rather than wrap it to a negative reference sample.
                (false, _) if raw > i32::MAX as u32 => {
                    Err(format!("unsigned sample {raw} exceeds the i32 container"))
                }
                (false, _) => Ok(raw as i32),
                (true, 1) => Ok(raw as u8 as i8 as i32),
                (true, 2) => Ok(raw as u16 as i16 as i32),
                (true, _) => Ok(raw as i32),
            }
        })
        .collect::<Result<Vec<i32>, String>>()?;

    Ok(Pgx {
        width,
        height,
        bit_depth: bit_depth as u8,
        signed,
        samples,
    })
}

// ---------------------------------------------------------------------------
// The comparator: peak absolute error and mean-squared error.
// ---------------------------------------------------------------------------

/// The class-1 error metrics for one component against its reference.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Metrics {
    /// Peak absolute error: the largest `|actual - reference|` over all samples.
    pae: f64,
    /// Mean-squared error: the mean of `(actual - reference)^2`.
    mse: f64,
}

impl Metrics {
    /// Whether both axes sit inside `bound` (which is `0/0` for an exact entry).
    fn within(self, bound: Bound) -> bool {
        self.pae <= bound.pae && self.mse <= bound.mse
    }

    /// Whether the decode reproduced the reference sample for sample. Both
    /// metrics are non-negative, so this is exactly the zero bound.
    fn exact(self) -> bool {
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
fn metrics(actual: &[i32], reference: &[i32]) -> Metrics {
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
fn grade_component(actual: &Component, reference: &Pgx, bound: Bound) -> Result<Metrics, String> {
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
enum Grade {
    /// Decoded and reproduced every graded component exactly.
    Exact,
    /// Decoded within the entry's compliance-class PAE/MSE bounds.
    WithinClass,
    /// The decoder rejected a feature it does not implement yet
    /// (`Error::Unsupported`). Expected while Phase 2 is in flight.
    NotYetDecoded(String),
    /// A genuine fault: a decode error other than `Unsupported`, a panic, a
    /// geometry disagreement, or samples outside the compliance class.
    Failed(String),
}

impl Grade {
    /// Whether the entry decoded and graded inside its class.
    fn is_in_class(&self) -> bool {
        matches!(self, Grade::Exact | Grade::WithinClass)
    }

    /// The status word reported for this entry.
    fn label(&self) -> &'static str {
        match self {
            Grade::Exact => "pass (bit-exact)",
            Grade::WithinClass => "pass (within class)",
            Grade::NotYetDecoded(_) => "not yet decoded",
            Grade::Failed(_) => "FAIL",
        }
    }
}

/// Decode one entry and grade every component the corpus grades.
fn grade_entry(dir: &Path, entry: &Entry) -> Grade {
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
    let options = rust_j2k::DecodeOptions {
        resolution_reduction: entry.reduction,
    };
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

fn load_manifest(dir: &Path) -> Manifest {
    let path = dir.join("manifest.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// The gate.
// ---------------------------------------------------------------------------

/// Decode every Part 4 entry and grade it against its compliance class.
///
/// Fails on any entry that faults, and on any drift between the entries that
/// actually decode in class and [`IN_CLASS`]. Entries the decoder does not
/// implement yet report as *not yet decoded* and are not failures.
#[test]
fn grades_the_conformance_corpus_by_compliance_class() {
    let dir = corpus_dir();
    let manifest = load_manifest(&dir);
    assert_eq!(
        manifest.compliance_class, 1,
        "the harness grades the per-component class-1 bounds"
    );

    let graded: Vec<(&str, Grade)> = manifest
        .entries
        .iter()
        .map(|entry| (entry.name(), grade_entry(&dir, entry)))
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
    let in_class = graded.iter().filter(|(_, g)| g.is_in_class()).count();
    eprintln!(
        "  ── {in_class}/{} in class, {} not yet decoded\n",
        graded.len(),
        graded.len() - in_class,
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
    let expected: BTreeSet<&str> = IN_CLASS.iter().copied().collect();
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

/// Every reference image in the corpus parses, so the PGX reader is exercised
/// over all the header spellings the corpus carries — not just the ones on the
/// handful of entries that decode today. The class-0 references are included
/// because they cover spellings the class-1 set does not, notably the three
/// files written with CRLF line endings.
#[test]
fn every_reference_image_parses() {
    let dir = corpus_dir();
    let mut parsed = 0;
    for entry in load_manifest(&dir).entries {
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
    assert_eq!(parsed, 78, "the corpus carries 78 reference images");
}

// ---------------------------------------------------------------------------
// Unit tests: the PGX reader and the PAE/MSE comparator.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pgx(header: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = header.as_bytes().to_vec();
        bytes.push(b'\n');
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn pgx_reads_unsigned_bytes() {
        let image = parse_pgx(&pgx("PG ML 8 2 2", &[0, 127, 128, 255])).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!((image.bit_depth, image.signed), (8, false));
        assert_eq!(image.samples, vec![0, 127, 128, 255]);
    }

    /// The sign may be glued to the depth (`+8`), stand alone (`+ 8`), or be
    /// absent — and fields may be separated by runs of whitespace. All four
    /// spellings appear in the corpus.
    #[test]
    fn pgx_accepts_every_depth_spelling() {
        let body = [1u8, 2];
        for header in [
            "PG ML +8 2 1",
            "PG ML + 8 2 1",
            "PG ML  8 2 1",
            "PG ML 8 2 1",
        ] {
            let image = parse_pgx(&pgx(header, &body)).unwrap_or_else(|e| panic!("{header}: {e}"));
            assert!(!image.signed, "{header}: should be unsigned");
            assert_eq!(image.bit_depth, 8, "{header}");
            assert_eq!(image.samples, vec![1, 2], "{header}");
        }
    }

    /// A negative depth means signed, and samples sign-extend from the storage
    /// byte width — not from the declared depth. `p0_03` is 4-bit signed stored
    /// one sample per byte, with values down to -8 (`0xF8`).
    #[test]
    fn pgx_sign_extends_from_the_storage_width() {
        let image = parse_pgx(&pgx("PG ML -4 3 1", &[0xF8, 0x00, 0x05])).unwrap();
        assert!(image.signed);
        assert_eq!(image.bit_depth, 4);
        assert_eq!(image.samples, vec![-8, 0, 5]);

        // 12-bit signed occupies two bytes; -1 is 0xFFFF, not 0x0FFF.
        let image = parse_pgx(&pgx("PG ML -12 2 1", &[0xFF, 0xFF, 0x07, 0xFF])).unwrap();
        assert_eq!(image.samples, vec![-1, 2047]);
    }

    /// Samples land in `i32`. An unsigned 32-bit sample past `i32::MAX` has no
    /// representation there and must be rejected, not wrapped to a negative
    /// reference sample that would then be graded against.
    #[test]
    fn pgx_rejects_an_unsigned_sample_past_the_i32_container() {
        let err = parse_pgx(&pgx("PG ML 32 1 1", &[0xFF, 0xFF, 0xFF, 0xFF])).unwrap_err();
        assert!(err.contains("exceeds the i32 container"), "got {err}");

        // The largest representable unsigned sample still reads back exactly.
        let image = parse_pgx(&pgx("PG ML 32 1 1", &[0x7F, 0xFF, 0xFF, 0xFF])).unwrap();
        assert_eq!(image.samples, vec![i32::MAX]);

        // Signed 32-bit spans the whole container, so nothing is rejected.
        let image = parse_pgx(&pgx("PG ML -32 1 1", &[0xFF, 0xFF, 0xFF, 0xFF])).unwrap();
        assert_eq!(image.samples, vec![-1]);
    }

    /// Three corpus references (`c0p0_03r0`, `c0p0_03r1`, `c0p0_14`) terminate
    /// their header with CRLF. The `\r` must not leak into the height token, and
    /// the body must still start after the `\n`.
    #[test]
    fn pgx_accepts_a_crlf_header() {
        let mut bytes = b"PG ML -4 2 1\r\n".to_vec();
        bytes.extend_from_slice(&[0xF8, 0x05]);
        let image = parse_pgx(&bytes).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.samples, vec![-8, 5]);
    }

    /// Depths above 8 bits pack big-endian by default (`ML`); `LM` is little-endian.
    #[test]
    fn pgx_honours_byte_order() {
        let big = parse_pgx(&pgx("PG ML 12 2 1", &[0x0F, 0xFF, 0x00, 0x01])).unwrap();
        assert_eq!(big.samples, vec![4095, 1]);

        let little = parse_pgx(&pgx("PG LM 12 2 1", &[0xFF, 0x0F, 0x01, 0x00])).unwrap();
        assert_eq!(little.samples, vec![4095, 1]);
    }

    #[test]
    fn pgx_rejects_malformed_input() {
        // A truncated body is the failure the corpus most needs caught.
        let err = parse_pgx(&pgx("PG ML 8 4 4", &[0; 15])).unwrap_err();
        assert!(err.contains("needs 16"), "got {err}");

        for (bytes, what) in [
            (pgx("XX ML 8 1 1", &[0]), "magic"),
            (pgx("PG XX 8 1 1", &[0]), "byte order"),
            (pgx("PG ML 8 1", &[0]), "missing height"),
            (pgx("PG ML 0 1 1", &[0]), "depth 0"),
            (pgx("PG ML 33 1 1", &[0]), "depth 33"),
            (b"PG ML 8 1 1".to_vec(), "no newline"),
        ] {
            assert!(parse_pgx(&bytes).is_err(), "{what} should be rejected");
        }
    }

    // --- comparator ---------------------------------------------------------

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
        Component {
            width: samples.len() as u32,
            height: 1,
            bit_depth: 8,
            signed: false,
            x_sampling: 1,
            y_sampling: 1,
            samples,
        }
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
            reduction: 0,
            references: References {
                class1: vec![],
                class0: vec![],
            },
            bounds_class1: Bounds {
                pae: vec![],
                mse: vec![],
            },
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
            reduction: 0,
            references: References {
                class1: vec!["references/c1p0_09_0.pgx".into()],
                class0: vec!["references/c0p0_09.pgx".into()],
            },
            bounds_class1: Bounds {
                pae: vec![5.0],
                mse: vec![5.0],
            },
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
        rust_j2k::DecodeOptions {
            resolution_reduction: 1,
        },
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
        rust_j2k::DecodeOptions {
            resolution_reduction: 4,
        },
    )
    .expect_err("a reduction past the coarsest resolution is rejected");
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
}
