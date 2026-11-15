//! ISO/IEC 15444-4:2024 HTJ2K conformance grading harness.
//!
//! Three sets, each graded with the Part 1 comparator and ratchet
//! ([`support::part4`]):
//!
//! - The HT codestreams and JPH files (`tests/fixtures/conformance/ht/`). The
//!   `ds0`/`ds1` codestreams are graded against the Part 1 class-1 references
//!   of the codestream each was made from, within the T.803 Annex C bounds;
//!   `hifi_ht1_02` against its own references (Table C.8). The highest-bound
//!   JPH file of each set is graded as `fileN.jp2` is, and `file10_b12`
//!   bit-exactly; the files below it as coded, within their set's bound plus
//!   Kakadu's recorded error.
//! - Agreement with OpenJPEG 2.5.0 on the codestreams it decodes: its output
//!   (the ISO references where it matches them, its snapshot where not),
//!   within the agreement bound.
//! - The OpenJPH test codestreams (`tests/fixtures/openjph/`), against
//!   OpenJPEG's decode of each.
//! - The bare `jp2c` codestreams of the graded JPH files, bit-exactly against
//!   the components their references hold before any box is applied.
//!
//! An entry outside its `IN_CLASS` list must report *not yet decoded*
//! (`decode` returns `Unsupported`). Any other outcome fails, and an entry that
//! starts to pass must be added to its list.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use support::ht::{
    HtManifest, HtSignaling, OpenJphManifest, Passes, load_ht_manifest, load_openjph_manifest,
};
use support::jp2::{Digest, sha256_hex};
use support::part4::{self, Bounds, Grade, ratchet};
use support::pgx::{Pgx, parse_pgx};

/// The HT codestreams and JPH files that decode within their bounds today.
const IN_CLASS: &[&str] = &[
    "ds0_ht_01_b11",
    "ds0_ht_02_b11",
    "ds0_ht_02_b12",
    "ds0_ht_03_b11",
    "ds0_ht_03_b14",
    "ds0_ht_04_b11",
    "ds0_ht_04_b12",
    "ds0_ht_05_b11",
    "ds0_ht_05_b12",
    "ds0_ht_06_b11",
    "ds0_ht_06_b15",
    "ds0_ht_06_b18",
    "ds0_ht_07_b11",
    "ds0_ht_07_b15",
    "ds0_ht_07_b16",
    "ds0_ht_08_b11",
    "ds0_ht_08_b15",
    "ds0_ht_08_b16",
    "ds0_ht_09_b11",
    "ds0_ht_10_b11",
    "ds0_ht_11_b10",
    "ds0_ht_12_b11",
    "ds0_ht_13_b11",
    "ds0_ht_14_b11",
    "ds0_ht_15_b11",
    "ds0_ht_15_b14",
    "ds0_ht_16_b11",
    "ds1_ht_01_b11",
    "ds1_ht_01_b12",
    "ds1_ht_02_b11",
    "ds1_ht_02_b12",
    "ds1_ht_03_b11",
    "ds1_ht_03_b12",
    "ds1_ht_04_b9",
    "ds1_ht_05_b11",
    "ds1_ht_06_b11",
    "ds1_ht_07_b11",
    "hifi_ht1_02",
    // The JPH files.
    "file1_b11",
    "file1_b12",
    "file2_b11",
    "file2_b12",
    "file3_b11",
    "file3_b12",
    "file4_b11",
    "file4_b12",
    "file5_b11",
    "file5_b12",
    "file6_b11",
    "file6_b15",
    "file7_b11",
    "file7_b15",
    "file7_b19",
    "file8_b11",
    "file9_b11",
    "file9_b12",
    "file10_b11",
    "file10_b12",
];

/// The HT codestreams that decode within the agreement bound of OpenJPEG's
/// output today.
const AGREES_WITH_OPENJPEG: &[&str] = &[
    "ds0_ht_01_b11",
    "ds0_ht_09_b11",
    "ds0_ht_11_b10",
    "ds0_ht_12_b11",
    "ds0_ht_14_b11",
    "ds1_ht_04_b9",
    "ds1_ht_05_b11",
    "ds1_ht_06_b11",
    "ds1_ht_07_b11",
    "hifi_ht1_02",
];

/// The OpenJPH codestreams that decode within their bounds today.
const OPENJPH_IN_CLASS: &[&str] = &[
    "simple_dec_irv97_64x64_16bit",
    "simple_dec_irv97_64x64_16bit_gray",
    "simple_dec_irv97_64x64_gray_tiles",
    "simple_dec_irv97_64x64_tiles_yuv",
    "simple_dec_irv97_64x64_yuv",
    "simple_dec_rev53_64x64_tiles_yuv",
    "simple_dec_rev53_64x64_yuv",
    "simple_enc_irv97_64x64",
    "simple_enc_irv97_64x64_16bit",
    "simple_enc_irv97_64x64_16bit_gray",
    "simple_enc_irv97_64x64_yuv",
    "simple_enc_irv97_tall_narrow",
    "simple_enc_irv97_tall_narrow1",
    "simple_enc_rev53_64x64_yuv",
    "simple_enc_rev53_tall_narrow",
    "simple_enc_rev53_tall_narrow1",
];

/// The JPH files whose bare codestream decodes bit-exactly today.
const JPH_CODESTREAM_EXACT: &[&str] = &[
    "file10_b12",
    "file1_b12",
    "file2_b12",
    "file3_b12",
    "file4_b12",
    "file5_b12",
    "file7_b19",
    "file8_b11",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn part1_dir() -> PathBuf {
    root().join("conformance")
}

fn ht_dir() -> PathBuf {
    root().join("conformance/ht")
}

fn jp2_dir() -> PathBuf {
    root().join("conformance/jp2")
}

fn openjph_dir() -> PathBuf {
    root().join("openjph")
}

fn read_pgx(dir: &Path, path: &str) -> Pgx {
    let raw = std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let pgx = parse_pgx(&raw).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(
        pgx.samples.len(),
        pgx.width as usize * pgx.height as usize,
        "{path}: sample count disagrees with its header",
    );
    pgx
}

/// Decode every HT codestream and graded JPH file, and enforce the ratchet.
#[test]
fn grades_the_ht_set_by_compliance_class() {
    let dir = ht_dir();
    let manifest = load_ht_manifest(&dir);
    assert_eq!(
        manifest.compliance_class, 1,
        "the harness grades the class-1 bounds"
    );
    let mut graded: Vec<(&str, Grade)> = manifest
        .codestreams
        .iter()
        .map(|c| (c.name.as_str(), c.grade(&dir)))
        .collect();
    let jp2 = jp2_dir();
    graded.extend(
        manifest
            .jph
            .iter()
            .map(|j| (j.name.as_str(), j.grade(&dir, &jp2, &manifest.jph))),
    );
    assert_eq!(graded.len(), 41 + 20);
    ratchet("ISO/IEC 15444-4:2024 HTJ2K class-1", &graded, IN_CLASS);
    pin_refusals(&graded, codestream_refusals(&manifest));
    let codestreams = manifest
        .codestreams
        .iter()
        .map(|c| (c.name.as_str(), &c.ht, &c.passes));
    in_class_iff_decodable(codestreams, IN_CLASS);
}

/// The refusal of a MIXED code-block style.
const MIXED: &str = "code-block style (HTJ2K mixed-mode block coding)";

/// Why a codestream with `ht` signaling is not yet decoded: MIXED in its
/// main header. Every other one decodes.
fn refusal(ht: &HtSignaling) -> &'static str {
    if ht.coder_set == "MIXED" {
        MIXED
    } else {
        panic!("an HTONLY codestream is refused")
    }
}

/// The expected refusal of each HT codestream of `manifest`, by name.
fn codestream_refusals(manifest: &HtManifest) -> impl Fn(&str) -> &'static str + '_ {
    |name| {
        let c = manifest.codestreams.iter().find(|c| c.name == name);
        let j = manifest.jph.iter().find(|j| j.name == name);
        match (c, j) {
            (Some(c), _) => refusal(&c.ht),
            (_, Some(j)) => refusal(&j.ht),
            _ => panic!("{name}: not in the manifest"),
        }
    }
}

/// An entry is in `in_class` exactly when it is HTONLY and each of its
/// code-blocks holds one HT set, by the Tier-2 walk's pass counts: the HT
/// decoder reads one HT set, with or without `RGN`. So the list is the
/// walk's, not a guess.
fn in_class_iff_decodable<'a>(
    entries: impl Iterator<Item = (&'a str, &'a HtSignaling, &'a Passes)>,
    in_class: &[&str],
) {
    for (name, ht, passes) in entries {
        let decodable =
            ht.coder_set == "HTONLY" && passes.walked.as_ref().is_some_and(|w| w.one_ht_set());
        assert_eq!(
            in_class.contains(&name),
            decodable,
            "{name}: in the list is not the same as HTONLY, one HT set"
        );
    }
}

/// Every entry not yet decoded is refused for the reason `expected` names
/// for it, so a refusal that moves to another stage is caught.
fn pin_refusals(graded: &[(&str, Grade)], expected: impl Fn(&str) -> &'static str) {
    for (name, grade) in graded {
        if let Grade::NotYetDecoded(why) = grade {
            let want = expected(name);
            assert!(
                why.contains(want),
                "{name}: refused for `{why}`, not `{want}`"
            );
        }
    }
}

/// Decode every HT codestream OpenJPEG decodes, and grade it against
/// OpenJPEG's output.
#[test]
fn grades_agreement_with_openjpeg() {
    let dir = ht_dir();
    let manifest = load_ht_manifest(&dir);
    let graded: Vec<(&str, Grade)> = manifest
        .codestreams
        .iter()
        .filter_map(|c| Some((c.name.as_str(), c.grade_agreement(&dir)?)))
        .collect();
    assert_eq!(graded.len(), 10);
    ratchet(
        "HTJ2K agreement with OpenJPEG 2.5.0",
        &graded,
        AGREES_WITH_OPENJPEG,
    );
    pin_refusals(&graded, codestream_refusals(&manifest));
}

/// Decode the bare codestream of every graded JPH file whose components a
/// reference holds, and grade it bit-exactly.
#[test]
fn grades_the_jph_codestreams() {
    let dir = ht_dir();
    let jp2 = jp2_dir();
    let manifest = load_ht_manifest(&dir);
    let graded: Vec<(&str, Grade)> = manifest
        .jph
        .iter()
        .filter_map(|j| Some((j.name.as_str(), j.grade_codestream(&dir, &jp2)?)))
        .collect();
    // Every graded file but two whose codestream components no reference
    // records: `file6_b15`, whose references are shifted, and `file9_b12`,
    // whose codestream holds palette indices.
    assert_eq!(graded.len(), 8);
    ratchet("JPH codestreams", &graded, JPH_CODESTREAM_EXACT);
    pin_refusals(&graded, codestream_refusals(&manifest));
    let graded_names: Vec<&str> = graded.iter().map(|g| g.0).collect();
    let files = manifest
        .jph
        .iter()
        .filter(|j| graded_names.contains(&j.name.as_str()))
        .map(|j| (j.name.as_str(), &j.ht, &j.passes));
    in_class_iff_decodable(files, JPH_CODESTREAM_EXACT);
}

/// Decode every OpenJPH codestream and grade it against OpenJPEG's decode.
#[test]
fn grades_the_openjph_codestreams() {
    let dir = openjph_dir();
    let manifest = load_openjph_manifest(&dir);
    let graded: Vec<(&str, Grade)> = manifest
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e.grade(&dir)))
        .collect();
    assert_eq!(graded.len(), 16);
    ratchet("OpenJPH test codestreams", &graded, OPENJPH_IN_CLASS);
    let manifest = load_openjph_manifest(&dir);
    pin_refusals(&graded, |name| {
        let e = manifest.entries.iter().find(|e| e.name == name).unwrap();
        refusal(&e.ht)
    });
    let entries = manifest
        .entries
        .iter()
        .map(|e| (e.name.as_str(), &e.ht, &e.passes));
    in_class_iff_decodable(entries, OPENJPH_IN_CLASS);
}

/// Every reference and OpenJPEG snapshot the manifests name parses, at the
/// depth of the component it grades, so a missing or damaged one fails here.
#[test]
fn every_reference_parses() {
    let ht = ht_dir();
    let manifest = load_ht_manifest(&ht);
    let mut parsed = 0;
    for c in &manifest.codestreams {
        let snapshot = c.openjpeg.snapshot.iter().flat_map(|s| &s.references);
        for (k, path) in c.references.iter().enumerate() {
            let pgx = read_pgx(&ht, path);
            assert_eq!(
                (u32::from(pgx.bit_depth), pgx.signed),
                (c.features.precision[k], c.features.signed[k]),
                "{}: {path}",
                c.name
            );
            parsed += 1;
        }
        for path in snapshot {
            read_pgx(&ht, path);
            parsed += 1;
        }
    }
    for j in &manifest.jph {
        for path in &j.references {
            read_pgx(&ht, path);
            parsed += 1;
        }
    }
    // 102 graded components, 10 snapshot planes, 5 for file10_b12.
    assert_eq!(parsed, 102 + 10 + 5);

    let dir = openjph_dir();
    let mut parsed = 0;
    for e in load_openjph_manifest(&dir).entries {
        for (k, path) in e.references.iter().enumerate() {
            let pgx = read_pgx(&dir, path);
            assert_eq!(
                (u32::from(pgx.bit_depth), pgx.signed),
                (e.features.precision[k], e.features.signed[k]),
                "{}: {path}",
                e.name
            );
            parsed += 1;
        }
    }
    assert_eq!(parsed, 42, "13 three-component files and 3 gray");
}

fn check_digest(dir: &Path, path: &str, digest: &Digest) {
    let bytes = std::fs::read(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(bytes.len() as u64, digest.size, "{path}: size");
    assert_eq!(sha256_hex(&bytes), digest.sha256, "{path}: sha256");
}

/// Every file the manifests name matches its recorded size and SHA-256.
#[test]
fn committed_bytes_match_the_manifest_digests() {
    let ht = ht_dir();
    let manifest = load_ht_manifest(&ht);
    for c in &manifest.codestreams {
        check_digest(&ht, &c.codestream, &c.file_digest);
        assert_eq!(c.references.len(), c.reference_digests.len(), "{}", c.name);
        for (path, digest) in c.references.iter().zip(&c.reference_digests) {
            check_digest(&ht, path, digest);
        }
        if let Some(s) = &c.openjpeg.snapshot {
            assert_eq!(s.references.len(), s.reference_digests.len(), "{}", c.name);
            for (path, digest) in s.references.iter().zip(&s.reference_digests) {
                check_digest(&ht, path, digest);
            }
        }
    }
    for j in &manifest.jph {
        check_digest(&ht, &j.file, &j.file_digest);
        assert_eq!(j.references.len(), j.reference_digests.len(), "{}", j.name);
        for (path, digest) in j.references.iter().zip(&j.reference_digests) {
            check_digest(&ht, path, digest);
        }
    }
    let dir = openjph_dir();
    for e in load_openjph_manifest(&dir).entries {
        check_digest(&dir, &e.file, &e.file_digest);
        assert_eq!(e.references.len(), e.reference_digests.len(), "{}", e.name);
        for (path, digest) in e.references.iter().zip(&e.reference_digests) {
            check_digest(&dir, path, digest);
        }
    }
}

/// The files under `dir`'s `subdirs`, as paths relative to `dir`.
fn files_under(dir: &Path, subdirs: &[&str]) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    for sub in subdirs {
        for entry in std::fs::read_dir(dir.join(sub)).unwrap_or_else(|e| panic!("{sub}: {e}")) {
            let name = entry.expect("dir entry").file_name();
            files.insert(format!("{sub}/{}", name.to_string_lossy()));
        }
    }
    files
}

/// Every committed codestream, JPH file, reference and snapshot is in a
/// manifest, so nothing sits in the corpus ungraded.
#[test]
fn every_committed_file_is_in_a_manifest() {
    let ht = ht_dir();
    let manifest = load_ht_manifest(&ht);
    let mut listed = BTreeSet::new();
    for c in &manifest.codestreams {
        listed.insert(c.codestream.clone());
        // The Part 1 references live outside `ht/`.
        listed.extend(
            c.references
                .iter()
                .filter(|r| !r.starts_with("../"))
                .cloned(),
        );
        listed.extend(
            c.openjpeg
                .snapshot
                .iter()
                .flat_map(|s| s.references.clone()),
        );
    }
    for j in &manifest.jph {
        listed.insert(j.file.clone());
        listed.extend(j.references.iter().cloned());
    }
    assert_eq!(
        files_under(&ht, &["codestreams", "jph", "references", "oracle"]),
        listed,
        "ht/: committed files and manifest disagree"
    );

    let dir = openjph_dir();
    let mut listed = BTreeSet::new();
    for e in load_openjph_manifest(&dir).entries {
        listed.insert(e.file);
        listed.extend(e.references);
    }
    assert_eq!(
        files_under(&dir, &["files", "references"]),
        listed,
        "openjph/: committed files and manifest disagree"
    );
}

/// The Part 1 tolerances in T.803 Tables C.6 and C.7 that differ from the
/// Part 1 manifest, which follows OpenJPEG's conformance run: (source,
/// component, T.803 MSE).
const T803_PART1_MSE: &[(&str, usize, f64)] = &[
    ("p0_05", 0, 0.319),
    ("p0_05", 1, 0.323),
    ("p0_05", 2, 0.317),
    ("p1_03", 0, 0.311),
    ("p1_03", 1, 0.280),
    ("p1_03", 2, 0.267),
    ("p1_05", 1, 9.716),
];

/// T.803 Tables C.1bis (`ds0`) and C.4bis (`ds1`) as printed: per codestream,
/// `Dref` and the additional peak and MSE of each component.
#[rustfmt::skip]
const T803_ALLOWANCE: &[(&str, u32, &[f64], &[f64])] = &[
    ("ds0_ht_01_b11", 8, &[0.0], &[0.0]),
    ("ds0_ht_02_b11", 8, &[1.0], &[0.001]),
    ("ds0_ht_02_b12", 8, &[0.0], &[0.0]),
    ("ds0_ht_03_b11", 4, &[17.0], &[0.15]),
    ("ds0_ht_03_b14", 4, &[0.0], &[0.0]),
    ("ds0_ht_04_b11", 8, &[2.0, 2.0, 2.0], &[0.1, 0.1, 0.1]),
    ("ds0_ht_04_b12", 8, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds0_ht_05_b11", 8, &[0.0, 1.0, 1.0, 2.0], &[0.0, 0.001, 0.001, 0.001]),
    ("ds0_ht_05_b12", 8, &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0]),
    ("ds0_ht_06_b11", 12, &[2500.0, 0.0, 0.0, 200.0], &[75000.0, 0.0, 0.0, 2000.0]),
    ("ds0_ht_06_b15", 12, &[0.0, 0.0, 0.0, 12.0], &[0.0, 0.0, 0.0, 10.0]),
    ("ds0_ht_06_b18", 12, &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0]),
    ("ds0_hm_06_b11", 12, &[0.0, 0.0, 0.0, 200.0], &[0.0, 0.0, 0.0, 2000.0]),
    ("ds0_hm_06_b18", 12, &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0]),
    ("ds0_ht_07_b11", 12, &[40.0, 40.0, 50.0], &[25.0, 25.0, 25.0]),
    ("ds0_ht_07_b15", 12, &[2.0, 2.0, 2.0], &[0.075, 0.05, 0.075]),
    ("ds0_ht_07_b16", 12, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds0_ht_08_b11", 12, &[40.0, 40.0, 40.0], &[45.0, 30.0, 45.0]),
    ("ds0_ht_08_b15", 12, &[0.0, 1.0, 0.0], &[0.0, 0.001, 0.0]),
    ("ds0_ht_08_b16", 12, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds0_ht_09_b11", 8, &[0.0], &[0.0]),
    ("ds0_ht_10_b11", 8, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds0_ht_11_b10", 8, &[0.0], &[0.0]),
    ("ds0_ht_12_b11", 8, &[0.0], &[0.0]),
    ("ds0_ht_13_b11", 8, &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0]),
    ("ds0_ht_14_b11", 8, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds0_ht_15_b11", 4, &[17.0], &[0.15]),
    ("ds0_ht_15_b14", 4, &[0.0], &[0.0]),
    ("ds0_hm_15_b8", 4, &[0.0], &[0.0]),
    ("ds0_ht_16_b11", 8, &[0.0], &[0.0]),
    ("ds1_ht_01_b11", 8, &[1.0], &[0.001]),
    ("ds1_ht_01_b12", 8, &[0.0], &[0.0]),
    ("ds1_ht_02_b11", 8, &[2.0, 2.0, 2.0], &[0.1, 0.1, 0.1]),
    ("ds1_ht_02_b12", 8, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds1_ht_03_b11", 8, &[0.0, 1.0, 2.0, 2.0], &[0.0, 0.03, 0.05, 0.001]),
    ("ds1_ht_03_b12", 8, &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 0.0, 0.0]),
    ("ds1_ht_04_b9", 8, &[0.0], &[0.0]),
    ("ds1_ht_05_b11", 8, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds1_ht_06_b11", 8, &[0.0, 0.0, 0.0], &[0.0, 0.0, 0.0]),
    ("ds1_ht_07_b11", 8, &[0.0, 0.0], &[0.0, 0.0]),
];

/// T.803 Table C.8 as printed: the peak and MSE bounds of `hifi_ht1_02`, then
/// of `hifi_p1_02`, the Part 1 codestream it was made from.
const T803_C8: [&[f64]; 4] = [
    &[47.0, 36.0, 42.0],
    &[82.0, 65.0, 86.0],
    &[43.0, 33.0, 38.0],
    &[80.0, 62.0, 72.0],
];

const EPS: f64 = 1e-9;

fn bounds_at(bounds: &Bounds, k: usize) -> (f64, f64) {
    (bounds.pae[k], bounds.mse[k])
}

/// Each HT bound follows the T.803 rule and passes its cross-checks:
///
/// - bound = Part 1 tolerance + additional tolerance, the peak scaled by
///   `2^(D - Dref)` and rounded up and the MSE by `4^(D - Dref)`;
/// - the Part 1 tolerance is the Part 1 manifest's, except where T.803 differs
///   ([`T803_PART1_MSE`]), and the references, reduction and graded count are
///   those of the Part 1 entry;
/// - the bound is at least the Part 1 tolerance plus the error Kakadu recorded
///   between the transcode and its source, at the component's own depth;
/// - the additional tolerance, `Dref` and the C.8 bounds are those T.803
///   prints, transcribed here apart from the manifest ([`T803_ALLOWANCE`],
///   [`T803_C8`]);
/// - an entry is `bit_exact` exactly when every bound is zero.
#[test]
fn every_bound_passes_its_cross_check() {
    let ht = ht_dir();
    let manifest = load_ht_manifest(&ht);
    let part1 = part4::load_manifest(&part1_dir());
    for c in &manifest.codestreams {
        let name = &c.name;
        let t = &c.tolerance;
        let n = c.graded_components;
        assert_eq!(t.depth.len(), n, "{name}");
        assert_eq!(t.part1.pae.len(), n, "{name}");
        assert_eq!(t.part1.mse.len(), n, "{name}");
        assert_eq!(c.bounds.pae.len(), n, "{name}");
        assert_eq!(c.bounds.mse.len(), n, "{name}");
        let all_zero = c.bounds.pae.iter().chain(&c.bounds.mse).all(|&b| b == 0.0);
        assert_eq!(c.bit_exact, all_zero, "{name}: bit_exact");

        // The allowance the manifest applies is the one T.803 prints.
        if name == "hifi_ht1_02" {
            let [pae, mse, p1_pae, p1_mse] = T803_C8;
            assert_eq!(c.bounds.pae, pae, "{name}: C.8 peak");
            assert_eq!(c.bounds.mse, mse, "{name}: C.8 MSE");
            assert_eq!(t.part1.pae, p1_pae, "{name}: C.8 hifi_p1_02 peak");
            assert_eq!(t.part1.mse, p1_mse, "{name}: C.8 hifi_p1_02 MSE");
            assert!(t.additional.is_none() && t.reference_depth.is_none());
        } else {
            let &(_, dref, pae, mse) = T803_ALLOWANCE
                .iter()
                .find(|(n, ..)| n == name)
                .unwrap_or_else(|| panic!("{name}: not in T803_ALLOWANCE"));
            let add = t.additional.as_ref().expect("additional tolerance");
            assert_eq!(t.reference_depth, Some(dref), "{name}: Dref");
            assert_eq!(add.pae, pae, "{name}: additional peak");
            assert_eq!(add.mse, mse, "{name}: additional MSE");
        }

        for k in 0..n {
            let d = t.depth[k];
            assert_eq!(d, c.features.precision[k], "{name} c{k}: depth");
            let (pae, mse) = bounds_at(&c.bounds, k);
            let (p1_pae, p1_mse) = bounds_at(&t.part1, k);
            match (&t.additional, t.reference_depth) {
                (Some(add), Some(dref)) => {
                    let shift = f64::from(d) - f64::from(dref);
                    let add_pae = (add.pae[k] * 2f64.powf(shift)).ceil();
                    let add_mse = add.mse[k] * 4f64.powf(shift);
                    assert_eq!(pae, p1_pae + add_pae, "{name} c{k}: PAE rule");
                    assert!(
                        (mse - (p1_mse + add_mse)).abs() < EPS,
                        "{name} c{k}: MSE rule"
                    );
                }
                (None, None) => {
                    assert_eq!(t.table, "C.8", "{name}: only C.8 gives bounds whole");
                    assert!(pae >= p1_pae && mse >= p1_mse, "{name} c{k}: below hifi_p1");
                }
                _ => panic!("{name}: additional and reference_depth go together"),
            }

            if let Some(stats) = &c.kakadu_stats {
                // Kakadu quotes 4-bit components at 8 bits.
                let at = f64::from(stats.depth.unwrap_or(d)) - f64::from(d);
                let k_pae = stats.pae[k] / 2f64.powf(at);
                let k_mse = stats.mse[k] / 4f64.powf(at);
                assert!(
                    pae + EPS >= p1_pae + k_pae,
                    "{name} c{k}: PAE bound {pae} below {p1_pae} + Kakadu {k_pae}"
                );
                assert!(
                    mse + EPS >= p1_mse + k_mse,
                    "{name} c{k}: MSE bound {mse} below {p1_mse} + Kakadu {k_mse}"
                );
            }
        }

        if c.source == "hifi_p1_02" {
            assert_eq!(name, "hifi_ht1_02");
            assert_eq!(t.table, "C.8");
            continue;
        }
        let p1 = part1
            .entries
            .iter()
            .find(|e| e.name() == c.source)
            .unwrap_or_else(|| panic!("{name}: no Part 1 entry {}", c.source));
        assert_eq!(c.graded_components, p1.graded_components, "{name}");
        assert_eq!(c.reduction, p1.reduction, "{name}");
        let want: Vec<String> = p1
            .references
            .class1
            .iter()
            .map(|r| format!("../{r}"))
            .collect();
        assert_eq!(c.references, want, "{name}: references");
        assert_eq!(t.part1.pae, p1.bounds_class1.pae, "{name}: Part 1 PAE");
        for k in 0..n {
            let want = T803_PART1_MSE
                .iter()
                .find(|(s, i, _)| *s == c.source && *i == k)
                .map_or(p1.bounds_class1.mse[k], |(_, _, mse)| *mse);
            assert_eq!(t.part1.mse[k], want, "{name} c{k}: Part 1 MSE");
        }
        let table = if name.starts_with("ds0_") {
            "C.1bis"
        } else {
            "C.4bis"
        };
        assert_eq!(t.table, table, "{name}");
    }

    // The table covers exactly the derived-set codestreams.
    let names: BTreeSet<&str> = manifest
        .codestreams
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let table: BTreeSet<&str> = T803_ALLOWANCE.iter().map(|(n, ..)| *n).collect();
    assert_eq!(table.len(), T803_ALLOWANCE.len(), "a name listed twice");
    assert_eq!(
        names.difference(&table).copied().collect::<Vec<_>>(),
        ["hifi_ht1_02"]
    );
    assert_eq!(table.difference(&names).count(), 0);
}

/// The bare codestream of every JPH file, graded or not, decodes to exactly
/// the samples OpenJPEG 2.5.0 decodes from it, by the digest of each
/// component that `regenerate.py` recorded.
#[test]
fn every_jph_codestream_matches_openjpeg() {
    let dir = ht_dir();
    let manifest = load_ht_manifest(&dir);
    for j in &manifest.jph {
        let name = &j.name;
        let bytes = std::fs::read(dir.join(&j.file)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let [start, end] = j.codestream_range;
        let image = rust_j2k::decode(&bytes[start..end]).unwrap_or_else(|e| panic!("{name}: {e}"));
        let got: Vec<String> = image
            .components
            .iter()
            .map(|c| {
                let le: Vec<u8> = c.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                sha256_hex(&le)
            })
            .collect();
        assert_eq!(got, j.openjpeg.codestream_sha256, "{name}");
    }
    assert_eq!(manifest.jph.len(), 20);
}

/// OpenJPEG's recorded error is within the bound. For the 4 codestreams with
/// a snapshot it is recomputed from the snapshot against the references; the
/// 6 that matched the references when the manifest was written must record
/// zero.
#[test]
fn openjpeg_output_is_within_every_bound() {
    let ht = ht_dir();
    let manifest = load_ht_manifest(&ht);
    let mut decoded = Vec::new();
    for c in &manifest.codestreams {
        let o = &c.openjpeg;
        let name = &c.name;
        if !o.decoded {
            assert!(o.message.is_some(), "{name}: a refusal records its message");
            assert!(o.metrics.is_none() && o.agreement_bound.is_none() && o.snapshot.is_none());
            continue;
        }
        decoded.push(name.as_str());
        let metrics = o.metrics.as_ref().expect("metrics");
        let matches = o.matches_references.expect("matches_references");
        assert_eq!(
            matches,
            o.snapshot.is_none(),
            "{name}: snapshot iff it differs"
        );
        assert!(o.agreement_bound.is_some(), "{name}");
        for k in 0..c.graded_components {
            let reference = read_pgx(&ht, &c.references[k]);
            let measured = match &o.snapshot {
                Some(s) => {
                    let opj = read_pgx(&ht, &s.references[k]);
                    assert_eq!(
                        (opj.width, opj.height, opj.bit_depth, opj.signed),
                        (
                            reference.width,
                            reference.height,
                            reference.bit_depth,
                            reference.signed
                        ),
                        "{name} c{k}: snapshot format"
                    );
                    part4::metrics(&opj.samples, &reference.samples)
                }
                // No snapshot: OpenJPEG's output equaled the references when
                // the manifest was written. That is not recomputed here; the
                // recorded figures must say so.
                None => {
                    assert_eq!(
                        (metrics.pae[k], metrics.mse[k]),
                        (0.0, 0.0),
                        "{name} c{k}: matches_references with a nonzero error"
                    );
                    continue;
                }
            };
            assert_eq!(measured.pae, metrics.pae[k], "{name} c{k}: PAE");
            assert!(
                (measured.mse - metrics.mse[k]).abs() < 1e-6,
                "{name} c{k}: MSE {} vs recorded {}",
                measured.mse,
                metrics.mse[k]
            );
            let (pae, mse) = bounds_at(&c.bounds, k);
            assert!(
                measured.pae <= pae && measured.mse <= mse + EPS,
                "{name} c{k}: OpenJPEG {measured} exceeds the bound"
            );
        }
    }
    assert_eq!(
        decoded,
        [
            "ds0_ht_01_b11",
            "ds0_ht_09_b11",
            "ds0_ht_11_b10",
            "ds0_ht_12_b11",
            "ds0_ht_14_b11",
            "ds1_ht_04_b9",
            "ds1_ht_05_b11",
            "ds1_ht_06_b11",
            "ds1_ht_07_b11",
            "hifi_ht1_02",
        ]
    );
}

/// The magnitude bound in a name (`..._b11`), if it has one.
fn name_bound(name: &str) -> Option<u32> {
    name.rsplit_once("_b")?.1.parse().ok()
}

/// The manifests record the corpus T.803 describes, so an edit cannot quietly
/// change the bar.
#[test]
fn the_manifest_pins_the_grading_bar() {
    let manifest = load_ht_manifest(&ht_dir());
    let names: Vec<&str> = manifest
        .codestreams
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    let count = |prefix: &str| names.iter().filter(|n| n.starts_with(prefix)).count();
    assert_eq!(names.len(), 41);
    assert_eq!(count("ds0_ht_"), 27);
    assert_eq!(count("ds0_hm_"), 3);
    assert_eq!(count("ds1_ht_"), 10);
    assert_eq!(count("hifi_ht1_02"), 1);

    for c in &manifest.codestreams {
        let name = &c.name;
        assert_eq!(c.ht.pcap, 0x0002_0000, "{name}: Pcap signals Part 15 only");
        assert!(!c.ht.multiht, "{name}");
        let mixed = name.starts_with("ds0_hm_");
        let style = if mixed { 0xC0 } else { 0x40 };
        assert_eq!(
            c.ht.coder_set,
            if mixed { "MIXED" } else { "HTONLY" },
            "{name}"
        );
        assert!(
            c.features.cblksty_bytes.iter().all(|&b| b & 0xC0 == style),
            "{name}: code-block style {:?}",
            c.features.cblksty_bytes
        );
        if let Some(b) = name_bound(name) {
            assert_eq!(c.ht.magb_bound, b, "{name}: CAP B");
        }
        assert_eq!(c.codestream, format!("codestreams/{name}.j2k"));
        let bytes = std::fs::read(ht_dir().join(&c.codestream)).expect("read");
        assert_eq!(&bytes[..4], [0xFF, 0x4F, 0xFF, 0x51], "{name}: SOC, SIZ");
        assert!(c.features.markers_main.iter().any(|m| m == "CAP"), "{name}");
        check_passes(name, &c.passes);
    }

    assert_eq!(manifest.jph.len(), 20);
    let mut graded = Vec::new();
    for j in &manifest.jph {
        let name = &j.name;
        assert_eq!(j.file, format!("jph/{name}.jph"));
        assert_eq!(j.ht.pcap, 0x0002_0000, "{name}");
        assert_eq!(Some(j.magb_bound), name_bound(name), "{name}");
        assert_eq!(j.ht.magb_bound, j.magb_bound, "{name}: CAP B");
        assert_eq!(j.graded_as.is_some(), j.highest_bound, "{name}");
        assert_eq!(j.not_graded.is_some(), j.graded_as.is_none(), "{name}");
        let bytes = std::fs::read(ht_dir().join(&j.file)).expect("read");
        let [start, end] = j.codestream_range;
        assert_eq!(&bytes[start..start + 4], [0xFF, 0x4F, 0xFF, 0x51], "{name}");
        assert_eq!(&bytes[end - 2..end], [0xFF, 0xD9], "{name}: EOC");
        assert_eq!(j.boxes.last().map(String::as_str), Some("jp2c"), "{name}");
        check_passes(name, &j.passes);
        if let Some(how) = &j.graded_as {
            graded.push((j.index, how.as_str()));
        }
    }
    assert_eq!(
        graded,
        [
            (1, "file1"),
            (2, "file2"),
            (3, "file3"),
            (4, "file4"),
            (5, "file5"),
            (6, "file6"),
            (7, "file7"),
            (8, "file8"),
            (9, "file9"),
            (10, "references"),
        ]
    );

    let openjph = load_openjph_manifest(&openjph_dir());
    assert_eq!(openjph.entries.len(), 16);
    for e in &openjph.entries {
        assert_eq!(e.ht.pcap, 0x0002_0000, "{}", e.name);
        let reversible = e.name.contains("_rev53_");
        assert_eq!(e.features.reversible, reversible, "{}", e.name);
        // Exact for 5/3; the irreversible bounds of the existing fixtures for
        // 9/7 (PAE 1 up to 12 bits, 5 at 16).
        assert_eq!(e.bit_exact, reversible, "{}", e.name);
        if !reversible {
            let deep = e.features.precision.iter().any(|&p| p > 12);
            let want = if deep { (5.0, 25.0) } else { (1.0, 1.0) };
            for k in 0..e.graded_components {
                assert_eq!(bounds_at(&e.bounds, k), want, "{} c{k}", e.name);
            }
        }
        check_passes(&e.name, &e.passes);
    }
}

/// The pass counts agree with the flags derived from them: from the Tier-2
/// walk where there is one, otherwise from OpenJPEG's count. Where both
/// counted, they agree.
fn check_passes(name: &str, passes: &support::ht::Passes) {
    let counts = |histogram: &std::collections::BTreeMap<String, u64>, blocks: u64| {
        let counts: Vec<(u32, u64)> = histogram
            .iter()
            .map(|(k, v)| (k.parse().expect("pass count"), *v))
            .collect();
        assert_eq!(counts.iter().map(|(_, v)| v).sum::<u64>(), blocks, "{name}");
        counts
    };
    let refinement = |counts: &[(u32, u64)]| Some(counts.iter().any(|&(p, _)| p > 1));
    if let Some(m) = &passes.measured {
        let counts = counts(&m.histogram, m.blocks);
        if let Some(w) = &passes.walked {
            assert_eq!(
                (w.blocks, &w.histogram, w.zero_length_refinement),
                (m.blocks, &m.histogram, m.zero_length_refinement),
                "{name}: walked and measured"
            );
        }
        // OpenJPEG refuses a block with placeholder passes.
        assert_eq!(passes.placeholder, Some(false), "{name}: placeholder");
        assert_eq!(passes.refinement, refinement(&counts), "{name}: refinement");
    }
    match &passes.walked {
        Some(w) => {
            let counts = counts(&w.histogram, w.blocks);
            assert!(w.placeholder <= w.blocks, "{name}");
            assert_eq!(
                passes.placeholder,
                Some(w.placeholder > 0),
                "{name}: placeholder"
            );
            assert_eq!(passes.refinement, refinement(&counts), "{name}: refinement");
        }
        None if passes.measured.is_none() => assert!(
            passes.placeholder.is_none() && passes.refinement.is_none(),
            "{name}"
        ),
        None => {}
    }
}

/// Unknown manifest keys are rejected rather than ignored, at every level.
#[test]
fn the_manifests_reject_unknown_keys() {
    /// Add an unknown key to the object at JSON pointer `at` and expect `T` to
    /// refuse it.
    fn reject<T: serde::de::DeserializeOwned>(value: &serde_json::Value, at: &str) {
        let mut v = value.clone();
        let object = v
            .pointer_mut(at)
            .and_then(serde_json::Value::as_object_mut)
            .unwrap_or_else(|| panic!("no object at {at}"));
        object.insert("surprise".into(), 1.into());
        let err = serde_json::from_value::<T>(v)
            .err()
            .unwrap_or_else(|| panic!("an unknown key at `{at}` must be rejected"));
        assert!(err.to_string().contains("unknown field"), "{at}: {err}");
    }
    let text = std::fs::read_to_string(ht_dir().join("manifest.json")).expect("read");
    serde_json::from_str::<HtManifest>(&text).expect("committed manifest parses");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    let find = |has: &str| {
        v["codestreams"]
            .as_array()
            .and_then(|a| a.iter().position(|c| !c.pointer(has).unwrap().is_null()))
            .unwrap_or_else(|| panic!("an entry with {has}"))
    };
    let snap = find("/openjpeg/snapshot");
    let measured = find("/passes/measured");
    let at = [
        String::new(),
        "/codestreams/0".into(),
        "/codestreams/0/bounds".into(),
        "/codestreams/0/tolerance".into(),
        "/codestreams/0/tolerance/part1".into(),
        "/codestreams/0/tolerance/additional".into(),
        "/codestreams/0/kakadu_stats".into(),
        "/codestreams/0/features".into(),
        "/codestreams/0/features/cblksty".into(),
        "/codestreams/0/ht".into(),
        "/codestreams/0/passes".into(),
        "/codestreams/0/file_digest".into(),
        "/codestreams/0/reference_digests/0".into(),
        "/codestreams/0/openjpeg".into(),
        format!("/codestreams/{snap}/openjpeg/metrics"),
        format!("/codestreams/{snap}/openjpeg/agreement_bound"),
        format!("/codestreams/{snap}/openjpeg/snapshot"),
        format!("/codestreams/{measured}/passes/measured"),
        "/codestreams/0/passes/walked".into(),
        "/jph/0".into(),
        "/jph/0/features".into(),
        "/jph/0/ht".into(),
        "/jph/0/passes".into(),
        "/jph/0/kakadu_stats".into(),
        "/jph/0/openjpeg".into(),
    ];
    for pointer in &at {
        reject::<HtManifest>(&v, pointer);
    }

    let text = std::fs::read_to_string(openjph_dir().join("manifest.json")).expect("read");
    serde_json::from_str::<OpenJphManifest>(&text).expect("committed manifest parses");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    for pointer in [
        "",
        "/entries/0",
        "/entries/0/features",
        "/entries/0/ht",
        "/entries/0/bounds",
    ] {
        reject::<OpenJphManifest>(&v, pointer);
    }
}

/// Under a layer cap, the `b11` and `b12` transcodes of a codestream decode
/// bit-identically while the kept layers give `b11`'s blocks no refinement
/// pass, and differ from the first cap that does. The two are independent
/// transcodes of one Part 1 codestream: `b12` codes every block in a cleanup
/// pass alone, `b11` some in a cleanup pass and two refinement passes in
/// later layers. No oracle grades the capped images: OpenJPEG loses sync on
/// the placeholder passes, and the Part 4 references are of every layer. So
/// the agreement checks the cleanup-only decode. The difference at the next
/// cap shows only that `b11` gains refinement passes there, not that they
/// decode right; `ht_refinement_matches_kakadu_stats` grades those.
#[test]
fn a_cap_before_the_refinement_layers_agrees_across_transcodes() {
    use rust_j2k::{DecodeOptions, decode_with, read_header};
    for (base, last) in [
        ("ds0_ht_02", 4),
        ("ds0_ht_04", 18),
        ("ds0_ht_05", 4),
        ("ds1_ht_01", 3),
        ("ds1_ht_02", 16),
        ("ds1_ht_03", 1),
    ] {
        let read = |bound: &str| {
            let path = ht_dir().join(format!("codestreams/{base}_{bound}.j2k"));
            std::fs::read(&path).unwrap_or_else(|e| panic!("{base}_{bound}: {e}"))
        };
        let (b11, b12) = (read("b11"), read("b12"));
        let header = read_header(&b11).unwrap();
        for cap in 1..=last + 1 {
            let options = DecodeOptions::default().with_max_quality_layers(Some(cap));
            let decode = |bytes: &[u8], bound: &str| {
                decode_with(bytes, &options)
                    .unwrap_or_else(|e| panic!("{base}_{bound} at -l {cap}: {e}"))
            };
            let (a, b) = (decode(&b11, "b11"), decode(&b12, "b12"));
            assert_eq!(
                (a.width, a.height, a.components.len()),
                (header.width, header.height, header.components.len()),
                "{base}_b11 at -l {cap}"
            );
            if cap <= last {
                assert!(a == b, "{base} at -l {cap}: b11 and b12 differ");
            } else {
                assert!(a != b, "{base} at -l {cap}: b11 and b12 agree");
            }
        }
    }
}

/// `ds1_ht_04_b9`, `ds1_ht_05_b11` and `ds1_ht_06_b11` are HT transcodes of
/// `p1_04`, `p1_05` and `p1_06` that keep every coded bit, and T.803 C.1.10
/// and C.1.11 test that a decoder gives the same image from both ("decoded
/// J2K and HTJ2K codestreams produce identical images"). So the HT decode
/// must equal the Part 1 decode exactly, a stronger check than the class-1
/// bound. `ds1_ht_05_b11` and `ds1_ht_06_b11` hold refinement segments of
/// zero length, which T.814 B.3 makes `Z_blk = 1`: cleanup pass only.
#[test]
fn ht_transcodes_decode_as_their_part1_source() {
    for (ht, part1) in [
        ("ds1_ht_04_b9", "p1_04"),
        ("ds1_ht_05_b11", "p1_05"),
        ("ds1_ht_06_b11", "p1_06"),
    ] {
        let read = |path: PathBuf| std::fs::read(&path).unwrap_or_else(|e| panic!("{e}"));
        let a = rust_j2k::decode(&read(ht_dir().join(format!("codestreams/{ht}.j2k"))))
            .unwrap_or_else(|e| panic!("{ht}: {e}"));
        let b = rust_j2k::decode(&read(part1_dir().join(format!("codestreams/{part1}.j2k"))))
            .unwrap_or_else(|e| panic!("{part1}: {e}"));
        assert!(a == b, "{ht} decodes unlike {part1}");
    }
}

/// Irreversible transcodes whose figures this decoder matches although its
/// 9/7 pipeline is not Kakadu's: on the others the float rounding differs.
const IRREVERSIBLE_STATS_MATCH: &[&str] = &["ds0_ht_02_b11", "ds1_ht_01_b11"];

/// Each HT transcode's manifest entry quotes Kakadu's PAE and MSE against
/// its Part 1 source, per component at full resolution (`kakadu_stats`).
/// Those figures grade the refinement passes, which no other oracle reaches
/// on the multi-layer files: this decoder's HT decode against its own decode
/// of the source must give the same PAE, and the same MSE to the six or
/// seven significant digits quoted. The irreversible files are graded only
/// where the transcode is exact (every figure zero) or in
/// `IRREVERSIBLE_STATS_MATCH`.
#[test]
fn ht_refinement_matches_kakadu_stats() {
    let manifest = load_ht_manifest(&ht_dir());
    let read = |path: PathBuf| std::fs::read(&path).unwrap_or_else(|e| panic!("{e}"));
    let mut refined = 0;
    for e in &manifest.codestreams {
        let Some(k) = &e.kakadu_stats else { continue };
        if !IN_CLASS.contains(&e.name.as_str()) {
            continue;
        }
        let exact = k.pae.iter().all(|&p| p == 0.0);
        if e.features.irreversible_anywhere
            && !exact
            && !IRREVERSIBLE_STATS_MATCH.contains(&e.name.as_str())
        {
            continue;
        }
        let ht = rust_j2k::decode(&read(ht_dir().join(&e.codestream)))
            .unwrap_or_else(|err| panic!("{}: {err}", e.name));
        let source = part1_dir().join(format!("codestreams/{}.j2k", e.source));
        let part1 =
            rust_j2k::decode(&read(source)).unwrap_or_else(|err| panic!("{}: {err}", e.source));
        for (c, (a, b)) in ht.components.iter().zip(&part1.components).enumerate() {
            let Some((&pae, &mse)) = k.pae.get(c).zip(k.mse.get(c)) else {
                break;
            };
            // Kakadu quotes 4-bit components at 8 bits.
            let d = e.features.precision[c];
            let at = f64::from(k.depth.unwrap_or(d)) - f64::from(d);
            let (pae, mse) = (pae / 2f64.powf(at), mse / 4f64.powf(at));
            let diffs = || a.samples.iter().zip(&b.samples).map(|(p, q)| p - q);
            let ours_pae = diffs().map(i32::abs).max().unwrap_or(0);
            let ours_mse =
                diffs().map(|d| f64::from(d).powi(2)).sum::<f64>() / a.samples.len() as f64;
            assert_eq!(f64::from(ours_pae), pae, "{} component {c}: PAE", e.name);
            assert!(
                (ours_mse - mse).abs() <= 1e-6 * mse.max(1.0),
                "{} component {c}: MSE {ours_mse} against Kakadu's {mse}",
                e.name
            );
        }
        let walked = e.passes.walked.as_ref().expect("walked");
        if walked.histogram.keys().any(|n| n != "1") {
            refined += 1;
        }
    }
    assert_eq!(refined, 17, "files with refinement passes graded");
}

/// JPH files where Kakadu's quoted MSE differs from this decoder's by more
/// than the `1e-6` relative rule allows: `file7_b15` (16-bit, MSE about 32,
/// 2.3e-6 and 4.5e-6 apart on components 0 and 1, the PAE equal on every
/// component). No whole sum of squared differences gives Kakadu's figure
/// when divided by the sample count and rounded to its six decimals: on
/// component 0, with 307200 samples, 31.932053 needs a sum in
/// [9809526.53, 9809526.84), which holds no integer. So Kakadu's figure is
/// not an exact MSE, and those components are held to `1e-5`.
const KAKADU_MSE_INEXACT: &[&str] = &["file7_b15"];

/// Each JPH file's manifest entry quotes Kakadu's PAE and MSE against its
/// source, per component, as coded: `fileN.jp2` for `file1`..`file9`, the
/// `reference_jph/` images for `file10`. This decoder's default decode (the
/// palette and channel definitions applied, no color conversion) must give
/// the same PAE and the same MSE to the digits quoted, against its own
/// decode of `fileN.jp2` (graded against the ISO references in
/// `conformance_jp2.rs`) or against the `file10` references. At the highest
/// bound every figure is zero, so that file decodes exactly as `fileN.jp2`
/// does.
#[test]
fn jph_files_match_kakadu_stats() {
    let dir = ht_dir();
    let manifest = load_ht_manifest(&dir);
    let read = |path: PathBuf| std::fs::read(&path).unwrap_or_else(|e| panic!("{e}"));
    let mut exact = 0;
    let mut inexact = BTreeSet::new();
    for j in &manifest.jph {
        let name = &j.name;
        let ours =
            rust_j2k::decode(&read(dir.join(&j.file))).unwrap_or_else(|e| panic!("{name}: {e}"));
        let source: Vec<Vec<i32>> = if j.index == 10 {
            let highest = manifest
                .jph
                .iter()
                .find(|h| h.index == 10 && h.highest_bound)
                .expect("file10_b12");
            highest
                .references
                .iter()
                .map(|r| read_pgx(&dir, r).samples)
                .collect()
        } else {
            let path = jp2_dir().join(format!("files/file{}.jp2", j.index));
            let image = rust_j2k::decode(&read(path)).unwrap_or_else(|e| panic!("{name}: {e}"));
            image.components.into_iter().map(|c| c.samples).collect()
        };
        let k = &j.kakadu_stats;
        assert_eq!(ours.components.len(), source.len(), "{name}: components");
        assert_eq!(k.pae.len(), source.len(), "{name}: Kakadu PAE figures");
        assert_eq!(k.mse.len(), source.len(), "{name}: Kakadu MSE figures");
        for (c, (a, b)) in ours.components.iter().zip(&source).enumerate() {
            assert_eq!(a.samples.len(), b.len(), "{name} component {c}: samples");
            let (pae, mse) = (k.pae[c], k.mse[c]);
            let diffs = || a.samples.iter().zip(b).map(|(p, q)| p - q);
            let ours_pae = diffs().map(i32::abs).max().unwrap_or(0);
            let ours_mse =
                diffs().map(|d| f64::from(d).powi(2)).sum::<f64>() / a.samples.len() as f64;
            assert_eq!(f64::from(ours_pae), pae, "{name} component {c}: PAE");
            let gap = (ours_mse - mse).abs();
            if gap <= 1e-6 * mse.max(1.0) {
                continue;
            }
            // Past the rule: only on a listed file, and only where no whole
            // sum of squared differences over the sample count rounds to
            // Kakadu's six decimals, so its figure is not exact. This
            // decoder's is exact, and its codestream decode is OpenJPEG's to
            // the bit (`every_jph_codestream_matches_openjpeg`).
            let count = a.samples.len() as f64;
            let six = |x: f64| format!("{x:.6}");
            let unreachable = [(mse * count).floor(), (mse * count).ceil()]
                .into_iter()
                .all(|sum| six(sum / count) != six(mse));
            assert!(
                KAKADU_MSE_INEXACT.contains(&name.as_str())
                    && unreachable
                    && gap <= 1e-5 * mse.max(1.0),
                "{name} component {c}: MSE {ours_mse} against Kakadu's {mse}"
            );
            inexact.insert(name.as_str());
        }
        if j.highest_bound {
            exact += 1;
        }
    }
    assert_eq!(exact, 10, "highest-bound files graded");
    assert_eq!(
        inexact,
        KAKADU_MSE_INEXACT.iter().copied().collect(),
        "the files whose MSE needs the wider tolerance"
    );
}

/// `file10` has no `colr` box, which T.814 D.2 allows when `UnkC` is set, and
/// a `cdef` box with four application-defined channels (`Typ` 3, D.6.3) whose
/// `Asoc` values are past the last channel, then an opacity channel. Both
/// `read_header` and `decode` report the five channels with their `Asoc` as
/// stored and no color space; the channels keep the codestream's order.
/// OpenJPEG 2.5.0 refuses both files ("Invalid component index 18283").
#[test]
fn file10_reports_application_defined_channels_and_no_color_space() {
    use rust_j2k::{Association, ChannelDefinition, ChannelType, ColorOutput, ColorSpace};
    let application = |asoc| ChannelDefinition {
        kind: ChannelType::ApplicationDefined,
        association: Association::ApplicationDefined(asoc),
    };
    let want = [
        application(18284),
        application(21100),
        application(17004),
        application(18284),
        ChannelDefinition {
            kind: ChannelType::Opacity,
            association: Association::WholeImage,
        },
    ];
    for name in ["file10_b11", "file10_b12"] {
        let bytes = std::fs::read(ht_dir().join(format!("jph/{name}.jph"))).unwrap();
        let header = rust_j2k::read_header(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let image = rust_j2k::decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        for jp2 in [header.jp2.as_ref(), image.jp2.as_ref()] {
            let jp2 = jp2.expect("JPH metadata");
            assert_eq!(jp2.file_type.brand, *b"jph ", "{name}");
            assert_eq!(jp2.color, ColorSpace::Unknown, "{name}");
            assert!(jp2.color_specifications.is_empty(), "{name}: no colr");
            assert_eq!(jp2.channels, want, "{name}");
        }
        assert_eq!(header.components.len(), 5, "{name}");
        assert_eq!(image.components.len(), 5, "{name}");
        // The channel definitions move no channel: the file decodes as its
        // bare codestream does.
        let [start, end] = load_ht_manifest(&ht_dir())
            .jph
            .iter()
            .find(|j| j.name == name)
            .expect("in the manifest")
            .codestream_range;
        let bare = rust_j2k::decode(&bytes[start..end]).unwrap();
        assert!(image.components == bare.components, "{name}");
        // Without a color specification there is no sRGB conversion.
        let mut srgb = rust_j2k::DecodeOptions::default();
        srgb.color = ColorOutput::Srgb;
        assert!(
            matches!(
                rust_j2k::decode_with(&bytes, &srgb),
                Err(rust_j2k::Error::Unsupported(m)) if m.contains("no color specification")
            ),
            "{name}"
        );
    }
}

/// The OpenJPH `.jph` files decode through the file reader to the same
/// components as their bare `jp2c` codestreams, which
/// `grades_the_openjph_codestreams` grades against OpenJPEG.
#[test]
fn openjph_jph_files_decode_as_their_codestream() {
    let dir = openjph_dir();
    let mut checked = 0;
    for e in load_openjph_manifest(&dir).entries {
        if !e.file.ends_with(".jph") {
            continue;
        }
        let bytes = std::fs::read(dir.join(&e.file)).unwrap();
        let [start, end] = e.codestream_range;
        let file = rust_j2k::decode(&bytes).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        let bare = rust_j2k::decode(&bytes[start..end]).unwrap();
        assert!(file.components == bare.components, "{}", e.name);
        assert_eq!(
            file.jp2.as_ref().map(|j| j.file_type.brand),
            Some(*b"jph "),
            "{}",
            e.name
        );
        checked += 1;
    }
    assert_eq!(checked, 7);
}
