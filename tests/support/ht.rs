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

use serde::Deserialize;

use super::jp2::{self, Digest};
use super::part4::{self, Bound, Bounds, Cblksty, Grade};

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

/// Run `decode` and accept a decoded image or `Error::Unsupported`. Any other
/// error, or a panic, is the reason returned.
pub fn decodes_or_unsupported(
    decode: impl FnOnce() -> Result<rust_j2k::Image, rust_j2k::Error>,
) -> Result<(), String> {
    match panic::catch_unwind(panic::AssertUnwindSafe(decode)) {
        Err(_) => Err("panicked (decode must return a typed error)".into()),
        Ok(Ok(_) | Err(rust_j2k::Error::Unsupported(_))) => Ok(()),
        Ok(Err(e)) => Err(format!("decoder rejected a valid file: {e}")),
    }
}

impl JphEntry {
    /// Decode a file that is not graded: it must decode or be refused as
    /// `Unsupported`, and must not panic. `None` for a graded file.
    pub fn check_ungraded(&self, ht_dir: &Path) -> Option<Result<(), String>> {
        if self.graded_as.is_some() {
            return None;
        }
        let bytes = match std::fs::read(ht_dir.join(&self.file)) {
            Ok(bytes) => bytes,
            Err(e) => return Some(Err(format!("cannot read {}: {e}", self.file))),
        };
        Some(decodes_or_unsupported(|| rust_j2k::decode(&bytes)))
    }

    /// Decode and grade the file. `None` for a file that is not graded.
    ///
    /// `file1`..`file9` are graded as `fileN.jp2` is (`jp2_dir` holds its
    /// manifest and references), with the JPH file in its place; `file10_b12`
    /// bit-exactly against its own references.
    pub fn grade(&self, ht_dir: &Path, jp2_dir: &Path) -> Option<Grade> {
        let graded_as = self.graded_as.as_deref()?;
        if graded_as == "references" {
            let bytes = match read(ht_dir, &self.file) {
                Ok(bytes) => bytes,
                Err(grade) => return Some(grade),
            };
            return Some(part4::grade_decode(
                ht_dir,
                &bytes,
                0,
                &self.references,
                |_| EXACT,
            ));
        }
        let Some(mut entry) = jp2::load_manifest(jp2_dir)
            .entries
            .into_iter()
            .find(|e| e.name() == graded_as)
        else {
            return Some(Grade::Failed(format!("no JP2 entry `{graded_as}`")));
        };
        // The JP2 grading reads `file` relative to `jp2/`; `ht/` is its sibling.
        entry.file = format!("../ht/{}", self.file);
        Some(jp2::grade_entry(jp2_dir, &entry))
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
