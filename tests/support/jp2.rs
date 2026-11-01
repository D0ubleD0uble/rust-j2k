//! The JP2 reader-file conformance set (ISO/IEC 15444-4 Annex G,
//! `file1.jp2` .. `file9.jp2`): the manifest schema and per-entry grading.
//!
//! The references are OpenJPEG's conformance baselines: 8-bit RGB *display
//! output* (palette, `cdef`, sYCC -> RGB and ICC -> sRGB applied, then
//! `opj_decompress -p 8S`, a plain right shift to 8 bits). Each channel is a
//! `.pgx` plane read by [`super::pgx`]. Grading reuses the Part 1 machinery in
//! [`super::part4`]: the same bounds type, PAE/MSE comparator, component
//! grader, [`Grade`], and ratchet.

use std::panic;
use std::path::Path;

use rust_j2k::{Component, DecodeOptions, Error};
use serde::Deserialize;

use super::part4::{Bound, Bounds, Features, Grade, grade_component, ratchet};
use super::pgx::parse_pgx;

/// The `jp2/manifest.json` schema. `deny_unknown_fields` throughout, so a key
/// with no field here fails to parse instead of being ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jp2Manifest {
    #[serde(rename = "_comment")]
    pub comment: String,
    pub compliance_class: u8,
    /// Sources, bounds origin, and license; not graded, kept as opaque JSON.
    pub provenance: serde_json::Value,
    pub entries: Vec<Jp2Entry>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Jp2Entry {
    /// The `.jp2` path, relative to the `jp2/` directory.
    pub file: String,
    /// `N` of `fileN.jp2`.
    pub index: u32,
    /// Channels graded: three for colour files, one for the greyscale files.
    pub graded_components: usize,
    /// Graded with zero tolerance; the recorded bounds are ignored.
    pub bit_exact: bool,
    /// Right shift that takes the decoded samples to the reference's 8 bits
    /// (`opj_decompress -p 8S`).
    pub shift_right: u8,
    /// The reference is sRGB display output, so the entry cannot be graded
    /// until the decoder offers an sRGB option.
    pub needs_srgb: bool,
    pub boxes: Boxes,
    pub colour: Colour,
    /// The codestream's parameters, in the Part 1 manifest's shape.
    pub features: Features,
    /// One `.pgx` per graded channel, in channel order.
    pub references: Vec<String>,
    pub bounds: Bounds,
}

/// The box types present, in file order.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Boxes {
    /// Top-level boxes (`"jP  "`, `"ftyp"`, ...).
    pub top: Vec<String>,
    /// Children of `jp2h` (`"ihdr"`, `"colr"`, `"pclr"`, ...).
    pub jp2h: Vec<String>,
}

/// The first `colr` box: `method` 1 carries `enum_cs` (16 sRGB, 17 greyscale,
/// 18 sYCC), `method` 2 an ICC profile of `icc_bytes` bytes.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Colour {
    pub method: u8,
    #[serde(default)]
    pub enum_cs: Option<u32>,
    #[serde(default)]
    pub icc_bytes: Option<u32>,
}

impl Jp2Entry {
    pub fn name(&self) -> String {
        format!("file{}", self.index)
    }

    /// The grading bound for channel `index`. A `bit_exact` entry is forced to
    /// zero whatever the manifest records, so a corpus edit cannot relax it.
    pub fn bound(&self, index: usize) -> Bound {
        if self.bit_exact {
            return Bound { pae: 0.0, mse: 0.0 };
        }
        Bound {
            pae: self.bounds.pae[index],
            mse: self.bounds.mse[index],
        }
    }
}

/// The decode options an entry is graded with. This is the one place #165
/// changes to ask for sRGB output on `needs_srgb` entries.
fn options_for(_entry: &Jp2Entry) -> DecodeOptions {
    DecodeOptions::default()
}

/// `component` shifted right by `shift` bits, the way `opj_decompress -p 8S`
/// scales to the reference's depth. The depth drops by the shift, so a wrong
/// `shift_right` shows up as a depth mismatch rather than a silent pass.
pub fn shifted(component: &Component, shift: u8) -> Component {
    Component::new(
        component.width,
        component.height,
        component.bit_depth.saturating_sub(shift),
        component.signed,
        component.x_sampling,
        component.y_sampling,
        component.samples.iter().map(|&s| s >> shift).collect(),
    )
}

/// Decode one entry with default options, shift, and grade every channel.
///
/// `srgb_available` says whether the decoder can produce sRGB output yet. While
/// it cannot, a `needs_srgb` entry that decodes reports *needs sRGB output*
/// instead of being graded against a reference it cannot match.
pub fn grade_entry(dir: &Path, entry: &Jp2Entry, srgb_available: bool) -> Grade {
    let graded = entry.graded_components;
    if graded == 0 {
        return Grade::Failed("manifest: entry grades no components".into());
    }
    if entry.references.len() != graded
        || entry.bounds.pae.len() != graded
        || entry.bounds.mse.len() != graded
    {
        return Grade::Failed(format!(
            "manifest: {graded} graded components but {} references, {} PAE, {} MSE bounds",
            entry.references.len(),
            entry.bounds.pae.len(),
            entry.bounds.mse.len(),
        ));
    }

    let bytes = match std::fs::read(dir.join(&entry.file)) {
        Ok(bytes) => bytes,
        Err(e) => return Grade::Failed(format!("cannot read {}: {e}", entry.file)),
    };

    // A panic is a contract violation, not a not-yet-implemented feature.
    let options = options_for(entry);
    let decoded = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        rust_j2k::decode_with(&bytes, options)
    }));
    let image = match decoded {
        Err(_) => return Grade::Failed("panicked (decode must return a typed error)".into()),
        Ok(Err(Error::Unsupported(what))) => return Grade::NotYetDecoded(what),
        Ok(Err(e)) => return Grade::Failed(format!("decoder rejected a valid file: {e}")),
        Ok(Ok(image)) => image,
    };

    if entry.needs_srgb && !srgb_available {
        return Grade::NotYetDecoded("needs sRGB output".into());
    }

    if image.components.len() < graded {
        return Grade::Failed(format!(
            "decoded {} components but the corpus grades {graded}",
            image.components.len()
        ));
    }

    let mut all_exact = true;
    for (index, reference_path) in entry.references.iter().enumerate() {
        let raw = match std::fs::read(dir.join(reference_path)) {
            Ok(raw) => raw,
            Err(e) => return Grade::Failed(format!("cannot read {reference_path}: {e}")),
        };
        let reference = match parse_pgx(&raw) {
            Ok(reference) => reference,
            Err(e) => return Grade::Failed(format!("cannot parse {reference_path}: {e}")),
        };
        let component = shifted(&image.components[index], entry.shift_right);
        match grade_component(&component, &reference, entry.bound(index)) {
            Ok(measured) => all_exact &= measured.exact(),
            Err(why) => return Grade::Failed(format!("channel {index}: {why}")),
        }
    }

    if all_exact {
        Grade::Exact
    } else {
        Grade::WithinClass
    }
}

pub fn load_manifest(dir: &Path) -> Jp2Manifest {
    let path = dir.join("manifest.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Decode and grade every entry, then enforce the ratchet against `in_class`.
pub fn run_ratchet(dir: &Path, in_class: &[&str], srgb_available: bool) {
    let manifest = load_manifest(dir);
    assert_eq!(
        manifest.compliance_class, 1,
        "the harness grades per-channel class-1 bounds"
    );
    let names: Vec<String> = manifest.entries.iter().map(Jp2Entry::name).collect();
    let graded: Vec<(&str, Grade)> = manifest
        .entries
        .iter()
        .zip(&names)
        .map(|(entry, name)| (name.as_str(), grade_entry(dir, entry, srgb_available)))
        .collect();
    ratchet("ISO/IEC 15444-4 JP2 reader files", &graded, in_class);
}
