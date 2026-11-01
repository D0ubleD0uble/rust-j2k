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

use rust_j2k::{ColourOutput, Component, DecodeOptions, Error};
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
    /// The reference is sRGB display output (sYCC or an ICC profile
    /// converted), so the entry is graded with `ColourOutput::Srgb`.
    pub needs_srgb: bool,
    pub boxes: Boxes,
    pub colour: Colour,
    /// The codestream's parameters, in the Part 1 manifest's shape.
    pub features: Features,
    /// One `.pgx` per graded channel, in channel order.
    pub references: Vec<String>,
    /// Size and SHA-256 of the `.jp2`, so a truncated or swapped file is caught.
    pub file_digest: Digest,
    /// Size and SHA-256 of each reference plane, in `references` order.
    pub reference_digests: Vec<Digest>,
    pub bounds: Bounds,
    /// OpenJPEG's per-component decode of the codestream as coded, for the
    /// `needs_srgb` entries. The default (as coded) decode is graded against
    /// it bit-exactly.
    pub as_coded: Option<AsCoded>,
}

/// As-coded references: one `.pgx` per codestream component (native size and
/// depth, no colour conversion, no shift) and the digest of each.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsCoded {
    pub references: Vec<String>,
    pub reference_digests: Vec<Digest>,
}

/// A committed file's byte size and lowercase hex SHA-256.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Digest {
    pub size: u64,
    pub sha256: String,
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

/// The decode options an entry is graded with: sRGB output for the
/// `needs_srgb` entries, whose references are display output, and the default
/// (as coded) for the rest.
pub fn options_for(entry: &Jp2Entry) -> DecodeOptions {
    let mut options = DecodeOptions::default();
    if entry.needs_srgb {
        options.colour = ColourOutput::Srgb;
    }
    options
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

/// Decode one entry with [`options_for`], shift, and grade every channel.
///
/// An entry with `as_coded` references is first decoded with the default
/// options and checked bit-exactly against them, so the as-coded output is
/// graded alongside the sRGB output.
pub fn grade_entry(dir: &Path, entry: &Jp2Entry) -> Grade {
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
    let decode = |options: DecodeOptions| match panic::catch_unwind(panic::AssertUnwindSafe(|| {
        rust_j2k::decode_with(&bytes, options)
    })) {
        Err(_) => Err(Grade::Failed(
            "panicked (decode must return a typed error)".into(),
        )),
        Ok(Err(Error::Unsupported(what))) => Err(Grade::NotYetDecoded(what)),
        Ok(Err(e)) => Err(Grade::Failed(format!("decoder rejected a valid file: {e}"))),
        Ok(Ok(image)) => Ok(image),
    };

    if let Some(as_coded) = &entry.as_coded {
        let image = match decode(DecodeOptions::default()) {
            Ok(image) => image,
            Err(grade) => return grade,
        };
        if let Err(why) = grade_as_coded(dir, as_coded, &image.components) {
            return Grade::Failed(format!("as coded: {why}"));
        }
    }

    let image = match decode(options_for(entry)) {
        Ok(image) => image,
        Err(grade) => return grade,
    };

    // Exactly the graded channels: greyscale files decode to one component,
    // colour files (and the palettised file9, once expanded) to three.
    if let Err(why) = check_component_count(image.components.len(), graded) {
        return Grade::Failed(why);
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

/// Compare every decoded component with its as-coded reference, with zero
/// tolerance. The count, size, depth, and every sample must match; no shift is
/// applied and chroma stays at its own size.
pub fn grade_as_coded(
    dir: &Path,
    as_coded: &AsCoded,
    components: &[Component],
) -> Result<(), String> {
    check_component_count(components.len(), as_coded.references.len())?;
    for (index, (path, component)) in as_coded.references.iter().zip(components).enumerate() {
        let raw = std::fs::read(dir.join(path)).map_err(|e| format!("cannot read {path}: {e}"))?;
        let reference = parse_pgx(&raw).map_err(|e| format!("cannot parse {path}: {e}"))?;
        let exact = Bound { pae: 0.0, mse: 0.0 };
        grade_component(component, &reference, exact)
            .map_err(|why| format!("component {index}: {why}"))?;
    }
    Ok(())
}

/// The decode must have exactly the channels the corpus grades; an extra one
/// (an alpha plane, an unexpanded palette) is a mismatch, not a pass.
pub fn check_component_count(decoded: usize, graded: usize) -> Result<(), String> {
    if decoded == graded {
        Ok(())
    } else {
        Err(format!(
            "decoded {decoded} components but the corpus grades {graded}"
        ))
    }
}

pub fn load_manifest(dir: &Path) -> Jp2Manifest {
    let path = dir.join("manifest.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Decode and grade every entry, then enforce the ratchet against `in_class`.
pub fn run_ratchet(dir: &Path, in_class: &[&str]) {
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
        .map(|(entry, name)| (name.as_str(), grade_entry(dir, entry)))
        .collect();
    ratchet("ISO/IEC 15444-4 JP2 reader files", &graded, in_class);
}

/// SHA-256 (FIPS 180-4) of `data` as lowercase hex. Inline so the test suite
/// needs no hashing dependency; checked against the standard test vectors.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}
