//! The HTJ2K conformance sets: the schema of `conformance/ht/manifest.json`
//! (the ISO/IEC 15444-4:2024 HT codestreams and JPH files) and of
//! `openjph/manifest.json` (the OpenJPH test codestreams), and how each entry
//! is graded.
//!
//! Grading reuses [`super::part4`] (the comparator, [`part4::grade_decode`] and
//! the ratchet) and, for the JPH files made from `fileN.jp2`, the JP2 grading
//! in [`super::jp2`].

use std::collections::BTreeMap;
use std::panic;
use std::path::Path;

use rust_j2k::DecodeOptions;
use serde::Deserialize;

use super::jp2::{self, Digest};
use super::part4::{self, Bound, Bounds, Cblksty, Grade};
use super::pgx::parse_pgx;

/// `conformance/ht/manifest.json`. `deny_unknown_fields` throughout.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtManifest {
    #[serde(rename = "_comment")]
    pub comment: String,
    pub compliance_class: u8,
    /// Sources, the bound rule and license; not graded, kept as opaque JSON.
    pub provenance: serde_json::Value,
    pub codestreams: Vec<HtCodestream>,
    pub jph: Vec<JphEntry>,
}

/// One HT codestream: a derived-set transcode of a Part 1 codestream, or
/// `hifi_ht1_02`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtCodestream {
    /// `ds0_ht_06_b11`: derived set, coder (`ht` HTONLY, `hm` MIXED), the Part 1
    /// codestream number and the magnitude bound.
    pub name: String,
    /// Path relative to `ht/`.
    pub codestream: String,
    /// The codestream it was made from: `p0_06`, or `hifi_p1_02`.
    pub source: String,
    pub graded_components: usize,
    /// Every bound is zero. Graded with zero tolerance whatever `bounds` says.
    pub bit_exact: bool,
    /// The resolution reduction the references were decoded at.
    pub reduction: u8,
    /// One `.pgx` per graded component, relative to `ht/`.
    pub references: Vec<String>,
    /// The grading bar: `tolerance.part1` plus the scaled additional tolerance.
    pub bounds: Bounds,
    pub tolerance: Tolerance,
    /// Kakadu's error between the transcode and its Part 1 source
    /// (`stats.txt`); absent for `hifi_ht1_02`.
    pub kakadu_stats: Option<Stats>,
    pub features: HtFeatures,
    pub ht: HtSignaling,
    pub passes: Passes,
    pub file_digest: Digest,
    pub reference_digests: Vec<Digest>,
    pub openjpeg: OpenJpeg,
}

/// Where an HT codestream's bounds come from (T.803 Annex C).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    /// `C.1bis`, `C.4bis` or `C.8`.
    pub table: String,
    /// The Part 1 class-1 tolerance (Tables C.6, C.7), or `hifi_p1_02`'s
    /// (Table C.8).
    pub part1: Bounds,
    /// The additional HT tolerance at `reference_depth`; absent for C.8,
    /// which gives the HT bounds whole.
    pub additional: Option<Bounds>,
    pub reference_depth: Option<u32>,
    /// Each graded component's depth `D` (Tables C.6, C.7, C.8).
    pub depth: Vec<u32>,
}

/// Kakadu's `pnm_psnr -stats` figures, at the depth `kdu_expand` wrote.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stats {
    /// The depth the figures are quoted at, when the file says.
    pub depth: Option<u32>,
    pub pae: Vec<f64>,
    pub mse: Vec<f64>,
}

/// The codestream's structure, read from its main and tile-part headers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtFeatures {
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
    /// Every code-block style byte in a COD or COC: `0x40` is HT, `0xC0` MIXED.
    pub cblksty_bytes: Vec<u8>,
    /// Whether any COD or COC selects the 9/7 transform.
    pub irreversible_anywhere: bool,
    pub markers_main: Vec<String>,
    pub markers_tile: Vec<String>,
    pub tile_parts: u32,
    pub sop: bool,
    pub eph: bool,
    pub precincts: bool,
    pub cblksty: Cblksty,
}

/// The Part 15 signaling: `CAP` (`Pcap`, `Ccap15` and its fields) and `CPF`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtSignaling {
    pub pcap: u32,
    pub ccap15: u16,
    /// `HTONLY`, `HTDECLARED` or `MIXED` (`Ccap15` bits 14-15).
    pub coder_set: String,
    pub multiht: bool,
    pub rgn: bool,
    pub heterogeneous: bool,
    pub irreversible: bool,
    /// The magnitude bound `B` from `Ccap15` bits 0-4 (T.814 A.3.7).
    pub magb_bound: u32,
    /// `CPFnum`, when a `CPF` marker segment is present.
    pub cpf: Option<i64>,
}

/// What is known about the coding passes of the HT code-blocks.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passes {
    /// T.803 Table C.2bis or C.5bis describes the BSET as having multiple
    /// layers with placeholder passes.
    pub t803_placeholder_bset: bool,
    /// Whether any block has placeholder passes; unknown unless measured.
    pub placeholder: Option<bool>,
    /// Whether any block has a SigProp or MagRef pass; unknown unless measured.
    pub refinement: Option<bool>,
    pub measured: Option<Measured>,
    /// Pass counts from the crate's own Tier-2 walk; absent where the main
    /// header is refused before it (MIXED).
    pub walked: Option<Walked>,
}

/// Pass counts from the crate's Tier-2 walk of every packet header
/// (`HT_WALKED` in `regenerate.py`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Walked {
    /// Blocks with any pass.
    pub blocks: u64,
    /// Blocks with placeholder passes.
    pub placeholder: u64,
    /// Passes past the placeholders -> number of blocks.
    pub histogram: BTreeMap<String, u64>,
    pub zero_length_refinement: u64,
}

impl Walked {
    /// Whether every block holds at most one HT set: a cleanup pass and up
    /// to two refinement passes.
    pub fn one_ht_set(&self) -> bool {
        self.histogram
            .keys()
            .all(|k| k.parse::<u32>().is_ok_and(|passes| passes <= 3))
    }
}

/// Pass counts measured with OpenJPEG on a file it decodes in full.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measured {
    pub blocks: u64,
    /// Passes per block -> number of blocks.
    pub histogram: BTreeMap<String, u64>,
    pub zero_length_refinement: u64,
}

/// What `opj_decompress` 2.5.0 does with the codestream.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenJpeg {
    pub decoded: bool,
    /// Its first error, when it refuses.
    pub message: Option<String>,
    pub warning: Option<String>,
    /// Its error against `references`, when it decodes.
    pub metrics: Option<Bounds>,
    pub matches_references: Option<bool>,
    /// How far a decode may drift from OpenJPEG's, when it decodes.
    pub agreement_bound: Option<ScalarBound>,
    /// OpenJPEG's output, when it decodes and differs from `references`.
    pub snapshot: Option<Snapshot>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScalarBound {
    pub pae: f64,
    pub mse: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub references: Vec<String>,
    pub reference_digests: Vec<Digest>,
}

/// One JPH file (T.803 G.5).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JphEntry {
    pub name: String,
    /// Path relative to `ht/`.
    pub file: String,
    /// `N` of `fileN_bB.jph`.
    pub index: u32,
    pub magb_bound: u32,
    /// The highest bound of its set, and so lossless against its source.
    pub highest_bound: bool,
    /// The byte range of the `jp2c` payload.
    pub codestream_range: [usize; 2],
    /// Top-level boxes, in file order.
    pub boxes: Vec<String>,
    pub features: HtFeatures,
    pub ht: HtSignaling,
    pub passes: Passes,
    pub kakadu_stats: Stats,
    pub file_digest: Digest,
    /// `fileN` when graded as `fileN.jp2` is, `references` when graded against
    /// `references`, absent when not graded.
    pub graded_as: Option<String>,
    /// Why the file is not graded.
    pub not_graded: Option<String>,
    pub references: Vec<String>,
    pub reference_digests: Vec<Digest>,
    pub openjpeg: JphOpenJpeg,
}

/// Whether `opj_decompress` 2.5.0 decodes the whole file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JphOpenJpeg {
    pub decoded: bool,
    pub message: Option<String>,
    /// The SHA-256 of each component OpenJPEG decodes from the bare
    /// codestream, its samples as little-endian `i32`.
    pub codestream_sha256: Vec<String>,
}

/// `openjph/manifest.json`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenJphManifest {
    #[serde(rename = "_comment")]
    pub comment: String,
    pub provenance: serde_json::Value,
    pub entries: Vec<OpenJphEntry>,
}

/// One OpenJPH test codestream, graded against OpenJPEG's decode of it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenJphEntry {
    pub name: String,
    /// Path relative to `openjph/`.
    pub file: String,
    /// The codestream's byte range: the `jp2c` payload of a `.jph`, the whole
    /// of a `.j2c`.
    pub codestream_range: [usize; 2],
    pub graded_components: usize,
    pub bit_exact: bool,
    pub bounds: Bounds,
    pub references: Vec<String>,
    pub features: HtFeatures,
    pub ht: HtSignaling,
    pub passes: Passes,
    pub file_digest: Digest,
    pub reference_digests: Vec<Digest>,
}

const EXACT: Bound = Bound { pae: 0.0, mse: 0.0 };

/// The JPH file components where Kakadu's quoted MSE differs from this
/// decoder's by more than the `1e-6` relative rule allows: components 0 and
/// 1 of `file7_b15` (16-bit, MSE about 32, 2.3e-6 and 4.5e-6 apart, the PAE
/// equal on every component). No whole sum of squared differences gives
/// Kakadu's figure when divided by the sample count and rounded to its six
/// decimals: on component 0, with 307200 samples, 31.932053 needs a sum in
/// [9809526.53, 9809526.84), which holds no integer. So Kakadu's figure is
/// not an exact MSE, and these components are held to `1e-5`
/// ([`kakadu_mse_slack`]). `jph_files_match_kakadu_stats` asserts the list
/// is exactly the components that need it.
pub const KAKADU_MSE_INEXACT: &[(&str, usize)] = &[("file7_b15", 0), ("file7_b15", 1)];

/// The relative slack on a comparison with Kakadu's MSE for `component` of
/// the JPH file `name`: `1e-6`, as Kakadu quotes six or seven significant
/// digits, or `1e-5` on the [`KAKADU_MSE_INEXACT`] components.
pub fn kakadu_mse_slack(name: &str, component: usize) -> f64 {
    if KAKADU_MSE_INEXACT.contains(&(name, component)) {
        1e-5
    } else {
        1e-6
    }
}

/// JPH files whose codestream holds `fileN.jp2`'s components in another
/// order: codestream component `k` is as-coded component `order[k]`.
/// `file2_b12` stores them reversed, although its `cdef` maps channels to
/// colors as `file2.jp2`'s does; `opj_decompress` 2.5.0 decodes its bare
/// codestream to the same reversed order.
const CODESTREAM_ORDER: &[(&str, &[usize])] = &[("file2_b12", &[2, 1, 0])];

/// The bound for component `index` of `bounds`, or zero when `bit_exact`, so
/// a corpus edit cannot loosen an exact entry.
fn bound_of(bounds: &Bounds, bit_exact: bool, index: usize) -> Bound {
    if bit_exact {
        return EXACT;
    }
    Bound {
        pae: bounds.pae[index],
        mse: bounds.mse[index],
    }
}

/// The counts that must agree before an entry is graded, so a short list
/// cannot silently grade fewer components.
fn check_counts(graded: usize, references: usize, bounds: &Bounds) -> Result<(), Grade> {
    if graded == 0
        || references != graded
        || bounds.pae.len() != graded
        || bounds.mse.len() != graded
    {
        return Err(Grade::Failed(format!(
            "manifest: {graded} graded components but {references} references, {} PAE, {} MSE bounds",
            bounds.pae.len(),
            bounds.mse.len(),
        )));
    }
    Ok(())
}

fn read(dir: &Path, path: &str) -> Result<Vec<u8>, Grade> {
    std::fs::read(dir.join(path)).map_err(|e| Grade::Failed(format!("cannot read {path}: {e}")))
}

impl HtCodestream {
    /// Decode and grade against the ISO references and bounds.
    pub fn grade(&self, ht_dir: &Path) -> Grade {
        if let Err(grade) =
            check_counts(self.graded_components, self.references.len(), &self.bounds)
        {
            return grade;
        }
        let bytes = match read(ht_dir, &self.codestream) {
            Ok(bytes) => bytes,
            Err(grade) => return grade,
        };
        part4::grade_decode(ht_dir, &bytes, self.reduction, &self.references, |i| {
            bound_of(&self.bounds, self.bit_exact, i)
        })
    }

    /// Decode and grade against OpenJPEG's output, within the agreement
    /// bound. `None` when OpenJPEG refuses the codestream.
    pub fn grade_agreement(&self, ht_dir: &Path) -> Option<Grade> {
        let bound = self.openjpeg.agreement_bound.as_ref()?;
        let references = match &self.openjpeg.snapshot {
            Some(snapshot) => &snapshot.references,
            None => &self.references,
        };
        let bytes = match read(ht_dir, &self.codestream) {
            Ok(bytes) => bytes,
            Err(grade) => return Some(grade),
        };
        let bound = Bound {
            pae: bound.pae,
            mse: bound.mse,
        };
        Some(part4::grade_decode(
            ht_dir,
            &bytes,
            self.reduction,
            references,
            |_| bound,
        ))
    }
}

/// Decode `bytes` with `options`: a panic or an error other than
/// `Unsupported` fails the grade, and `Unsupported` is *not yet decoded*.
fn decode_graded(bytes: &[u8], options: &DecodeOptions) -> Result<rust_j2k::Image, Grade> {
    match panic::catch_unwind(panic::AssertUnwindSafe(|| {
        rust_j2k::decode_with(bytes, options)
    })) {
        Err(_) => Err(Grade::Failed(
            "panicked (decode must return a typed error)".into(),
        )),
        Ok(Err(rust_j2k::Error::Unsupported(what))) => Err(Grade::NotYetDecoded(what)),
        Ok(Err(e)) => Err(Grade::Failed(format!("decoder rejected a valid file: {e}"))),
        Ok(Ok(image)) => Ok(image),
    }
}

/// What a JPH file below the highest bound of its set is graded against: the
/// as-coded image of the set's source, at the reference's depth.
struct AsCodedSource<'a> {
    dir: &'a Path,
    references: Vec<String>,
    /// The right shift that brings a decoded component to the reference's
    /// depth (`file6`'s references are 8-bit, its samples 12-bit).
    shift: u8,
    /// The source's recorded class bound; `None` where the references are
    /// matched exactly: `fileN.jp2`'s as-coded references (OpenJPEG's
    /// snapshot, which `fileN.jp2` matches bit for bit) and `file10`'s.
    bounds: Option<Bounds>,
    /// The options the source is graded with, so the display path is decoded
    /// too.
    display: DecodeOptions,
}

impl JphEntry {
    /// Decode and grade the file.
    ///
    /// The highest-bound file of a set is graded as `fileN.jp2` is (`jp2_dir`
    /// holds its manifest and references), with the JPH file in its place;
    /// `file10_b12` bit-exactly against its own references. A file below the
    /// highest bound is lossy against its source, by the amount Kakadu's
    /// `stats.txt` records, so it is graded as coded against the source's
    /// as-coded image, within the source's bound plus Kakadu's error
    /// ([`Self::grade_lower_bound`]). `set` is every JPH entry of the
    /// manifest.
    pub fn grade(&self, ht_dir: &Path, jp2_dir: &Path, set: &[JphEntry]) -> Grade {
        let Some(graded_as) = self.graded_as.as_deref() else {
            return self.grade_lower_bound(ht_dir, jp2_dir, set);
        };
        if graded_as == "references" {
            let bytes = match read(ht_dir, &self.file) {
                Ok(bytes) => bytes,
                Err(grade) => return grade,
            };
            return part4::grade_decode(ht_dir, &bytes, 0, &self.references, |_| EXACT);
        }
        let Some(mut entry) = jp2::load_manifest(jp2_dir)
            .entries
            .into_iter()
            .find(|e| e.name() == graded_as)
        else {
            return Grade::Failed(format!("no JP2 entry `{graded_as}`"));
        };
        // The JP2 grading reads `file` relative to `jp2/`; `ht/` is its sibling.
        entry.file = format!("../ht/{}", self.file);
        jp2::grade_entry(jp2_dir, &entry)
    }

    /// The as-coded image of this file's set: `fileN.jp2`'s as-coded
    /// references, or its references where its boxes change nothing a
    /// default decode keeps (no color conversion), or `file10_b12`'s
    /// references.
    fn as_coded_source<'a>(
        &self,
        ht_dir: &'a Path,
        jp2_dir: &'a Path,
        set: &[JphEntry],
    ) -> Result<AsCodedSource<'a>, Grade> {
        let highest = set
            .iter()
            .find(|j| j.index == self.index && j.highest_bound)
            .ok_or_else(|| {
                Grade::Failed(format!("no highest-bound file for set {}", self.index))
            })?;
        match highest.graded_as.as_deref() {
            Some("references") => Ok(AsCodedSource {
                dir: ht_dir,
                references: highest.references.clone(),
                shift: 0,
                bounds: None,
                display: DecodeOptions::default(),
            }),
            Some(name) => {
                let entry = jp2::load_manifest(jp2_dir)
                    .entries
                    .into_iter()
                    .find(|e| e.name() == name)
                    .ok_or_else(|| Grade::Failed(format!("no JP2 entry `{name}`")))?;
                let display = jp2::options_for(&entry);
                let (references, shift, bounds) = match entry.as_coded {
                    Some(as_coded) => (as_coded.references, 0, None),
                    None if !entry.needs_srgb => {
                        (entry.references, entry.shift_right, Some(entry.bounds))
                    }
                    None => {
                        return Err(Grade::Failed(format!(
                            "`{name}` has no as-coded references"
                        )));
                    }
                };
                Ok(AsCodedSource {
                    dir: jp2_dir,
                    references,
                    shift,
                    bounds,
                    display,
                })
            }
            None => Err(Grade::Failed(format!(
                "the highest-bound file of set {} is not graded",
                self.index
            ))),
        }
    }

    /// Grade a file below the highest bound of its set. It is decoded as
    /// coded (with its palette and channel definitions applied, no color
    /// conversion), which is the image Kakadu's figures describe, and each
    /// component is graded against the set's as-coded image within the
    /// source's class bound plus Kakadu's PAE and MSE for this file, scaled
    /// to the reference's depth. It is also decoded as its source is graded
    /// (sRGB output where the source's references need it), which must
    /// succeed.
    fn grade_lower_bound(&self, ht_dir: &Path, jp2_dir: &Path, set: &[JphEntry]) -> Grade {
        let source = match self.as_coded_source(ht_dir, jp2_dir, set) {
            Ok(source) => source,
            Err(grade) => return grade,
        };
        let graded = source.references.len();
        let stats = &self.kakadu_stats;
        if graded == 0 || stats.pae.len() != graded || stats.mse.len() != graded {
            return Grade::Failed(format!(
                "manifest: {graded} references but {} Kakadu PAE and {} MSE figures",
                stats.pae.len(),
                stats.mse.len()
            ));
        }
        let bytes = match read(ht_dir, &self.file) {
            Ok(bytes) => bytes,
            Err(grade) => return grade,
        };
        let image = match decode_graded(&bytes, &DecodeOptions::default()) {
            Ok(image) => image,
            Err(grade) => return grade,
        };
        if let Err(grade) = decode_graded(&bytes, &source.display) {
            return grade;
        }
        if image.components.len() != graded {
            return Grade::Failed(format!(
                "decoded {} components but the set grades {graded}",
                image.components.len()
            ));
        }
        let scale = f64::from(1u32 << source.shift);
        let mut all_exact = true;
        for (index, path) in source.references.iter().enumerate() {
            let decoded = &image.components[index];
            if stats.depth != Some(u32::from(decoded.bit_depth)) {
                return Grade::Failed(format!(
                    "component {index}: Kakadu's figures are at depth {:?}, the decode at {}",
                    stats.depth, decoded.bit_depth
                ));
            }
            let raw = match std::fs::read(source.dir.join(path)) {
                Ok(raw) => raw,
                Err(e) => return Grade::Failed(format!("cannot read {path}: {e}")),
            };
            let reference = match parse_pgx(&raw) {
                Ok(reference) => reference,
                Err(e) => return Grade::Failed(format!("cannot parse {path}: {e}")),
            };
            let base = source
                .bounds
                .as_ref()
                .map_or(EXACT, |b| bound_of(b, false, index));
            // Kakadu quotes its MSE to six or seven significant digits, so
            // the bound carries that slack, or more on the components where
            // its figure is not an exact MSE ([`kakadu_mse_slack`]). With an
            // exact base (as-coded references) nothing else absorbs the
            // rounding.
            //
            // This is a heuristic, not a proven bound. Adding the MSEs
            // assumes the two errors add in energy; the true bound on their
            // sum is `(sqrt(a) + sqrt(b))^2`. And for `file6` (shift 4),
            // Kakadu's figures are at 12 bits and the references at 8:
            // dividing by `scale^2` does not bound the error after the
            // shift's rounding, which passes only through the class's +1.
            // What pins these files is `jph_files_match_kakadu_stats`, which
            // matches Kakadu's figures at the native depth.
            let mse = stats.mse[index] / (scale * scale);
            let bound = Bound {
                pae: base.pae + (stats.pae[index] / scale).ceil(),
                mse: base.mse + mse + kakadu_mse_slack(&self.name, index) * mse.max(1.0),
            };
            let component = jp2::shifted(decoded, source.shift);
            match part4::grade_component(&component, &reference, bound) {
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

    /// Decode the bare `jp2c` codestream of a graded file and grade its
    /// components bit-exactly, before any box is applied: against
    /// `fileN.jp2`'s as-coded references, or its references where nothing
    /// in its boxes changes the components (no palette, no shift, no color
    /// conversion), or `file10_b12`'s own. `None` for a file not graded, or
    /// whose components no reference holds (`file9`'s palette indices).
    pub fn grade_codestream(&self, ht_dir: &Path, jp2_dir: &Path) -> Option<Grade> {
        let graded_as = self.graded_as.as_deref()?;
        let (dir, references) = if graded_as == "references" {
            (ht_dir, self.references.clone())
        } else {
            let entry = jp2::load_manifest(jp2_dir)
                .entries
                .into_iter()
                .find(|e| e.name() == graded_as)?;
            let unchanged = entry.bit_exact
                && entry.shift_right == 0
                && !entry.needs_srgb
                && !entry.boxes.jp2h.iter().any(|b| b == "pclr");
            let mut references = match entry.as_coded {
                Some(as_coded) => as_coded.references,
                None if unchanged => entry.references,
                None => return None,
            };
            if let Some((_, order)) = CODESTREAM_ORDER.iter().find(|(n, _)| *n == self.name) {
                references = order.iter().map(|&i| references[i].clone()).collect();
            }
            (jp2_dir, references)
        };
        let bytes = match read(ht_dir, &self.file) {
            Ok(bytes) => bytes,
            Err(grade) => return Some(grade),
        };
        let [start, end] = self.codestream_range;
        let Some(codestream) = bytes.get(start..end) else {
            return Some(Grade::Failed(format!(
                "codestream range {start}..{end} past the file"
            )));
        };
        Some(part4::grade_decode(dir, codestream, 0, &references, |_| {
            EXACT
        }))
    }
}

impl OpenJphEntry {
    /// Decode the codestream and grade it against OpenJPEG's decode.
    pub fn grade(&self, dir: &Path) -> Grade {
        if let Err(grade) =
            check_counts(self.graded_components, self.references.len(), &self.bounds)
        {
            return grade;
        }
        let bytes = match read(dir, &self.file) {
            Ok(bytes) => bytes,
            Err(grade) => return grade,
        };
        let [start, end] = self.codestream_range;
        let Some(codestream) = bytes.get(start..end) else {
            return Grade::Failed(format!("codestream range {start}..{end} past the file"));
        };
        part4::grade_decode(dir, codestream, 0, &self.references, |i| {
            bound_of(&self.bounds, self.bit_exact, i)
        })
    }
}

fn load<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

pub fn load_ht_manifest(ht_dir: &Path) -> HtManifest {
    load(&ht_dir.join("manifest.json"))
}

pub fn load_openjph_manifest(dir: &Path) -> OpenJphManifest {
    load(&dir.join("manifest.json"))
}
