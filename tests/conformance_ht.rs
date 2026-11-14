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
//!   bit-exactly.
//! - Agreement with OpenJPEG 2.5.0 on the codestreams it decodes: its output
//!   (the ISO references where it matches them, its snapshot where not),
//!   within the agreement bound.
//! - The OpenJPH test codestreams (`tests/fixtures/openjph/`), against
//!   OpenJPEG's decode of each.
//!
//! An entry outside its `IN_CLASS` list must report *not yet decoded*
//! (`decode` returns `Unsupported`). Any other outcome fails, and an entry that
//! starts to pass must be added to its list.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use support::ht::{HtManifest, OpenJphManifest, load_ht_manifest, load_openjph_manifest};
use support::jp2::{Digest, sha256_hex};
use support::part4::{self, Bounds, Grade, ratchet};
use support::pgx::{Pgx, parse_pgx};

/// The HT codestreams and JPH files that decode within their bounds today.
const IN_CLASS: &[&str] = &[];

/// The HT codestreams that decode within the agreement bound of OpenJPEG's
/// output today.
const AGREES_WITH_OPENJPEG: &[&str] = &[];

/// The OpenJPH codestreams that decode within their bounds today.
const OPENJPH_IN_CLASS: &[&str] = &[];

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
            .filter_map(|j| Some((j.name.as_str(), j.grade(&dir, &jp2)?))),
    );
    assert_eq!(graded.len(), 41 + 10);
    ratchet("ISO/IEC 15444-4:2024 HTJ2K class-1", &graded, IN_CLASS);
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
/// - OpenJPEG's error, recomputed from its snapshot, is the recorded one and
///   within the bound;
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
}

/// OpenJPEG's recorded error is what its output measures against the
/// references, and is within the bound.
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
                None => part4::metrics(&reference.samples, &reference.samples),
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

/// A measured pass histogram agrees with the flags derived from it.
fn check_passes(name: &str, passes: &support::ht::Passes) {
    let Some(m) = &passes.measured else {
        assert!(
            passes.placeholder.is_none() && passes.refinement.is_none(),
            "{name}"
        );
        return;
    };
    let counts: Vec<(u32, u64)> = m
        .histogram
        .iter()
        .map(|(k, v)| (k.parse().expect("pass count"), *v))
        .collect();
    assert_eq!(
        counts.iter().map(|(_, v)| v).sum::<u64>(),
        m.blocks,
        "{name}"
    );
    assert_eq!(
        passes.placeholder,
        Some(counts.iter().any(|&(p, _)| p > 3)),
        "{name}: placeholder"
    );
    assert_eq!(
        passes.refinement,
        Some(counts.iter().any(|&(p, _)| p > 1)),
        "{name}: refinement"
    );
}

/// Unknown manifest keys are rejected rather than ignored.
#[test]
fn the_manifests_reject_unknown_keys() {
    fn reject<T: serde::de::DeserializeOwned>(
        value: &serde_json::Value,
        mutate: &dyn Fn(&mut serde_json::Value),
        what: &str,
    ) {
        let mut v = value.clone();
        mutate(&mut v);
        let err = serde_json::from_value::<T>(v)
            .err()
            .unwrap_or_else(|| panic!("an unknown key {what} must be rejected"));
        assert!(err.to_string().contains("unknown field"), "{what}: {err}");
    }
    let text = std::fs::read_to_string(ht_dir().join("manifest.json")).expect("read");
    serde_json::from_str::<HtManifest>(&text).expect("committed manifest parses");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    let snap = v["codestreams"]
        .as_array()
        .and_then(|a| a.iter().position(|c| !c["openjpeg"]["snapshot"].is_null()))
        .expect("an entry with a snapshot");
    let measured = v["codestreams"]
        .as_array()
        .and_then(|a| a.iter().position(|c| !c["passes"]["measured"].is_null()))
        .expect("an entry with measured passes");
    let paths: &[(
        &str,
        &dyn Fn(&mut serde_json::Value) -> &mut serde_json::Value,
    )] = &[
        ("at the top level", &|v| v),
        ("on a codestream", &|v| &mut v["codestreams"][0]),
        ("in bounds", &|v| &mut v["codestreams"][0]["bounds"]),
        ("in tolerance", &|v| &mut v["codestreams"][0]["tolerance"]),
        (
            "in kakadu_stats",
            &|v| &mut v["codestreams"][0]["kakadu_stats"],
        ),
        ("in features", &|v| &mut v["codestreams"][0]["features"]),
        (
            "in cblksty",
            &|v| &mut v["codestreams"][0]["features"]["cblksty"],
        ),
        ("in ht", &|v| &mut v["codestreams"][0]["ht"]),
        ("in passes", &|v| &mut v["codestreams"][0]["passes"]),
        ("in a digest", &|v| &mut v["codestreams"][0]["file_digest"]),
        ("in openjpeg", &|v| &mut v["codestreams"][0]["openjpeg"]),
        ("on a JPH entry", &|v| &mut v["jph"][0]),
        ("in a JPH openjpeg", &|v| &mut v["jph"][0]["openjpeg"]),
    ];
    for (what, at) in paths {
        reject::<HtManifest>(&v, &|v| at(v)["surprise"] = 1.into(), what);
    }
    reject::<HtManifest>(
        &v,
        &|v| v["codestreams"][snap]["openjpeg"]["snapshot"]["surprise"] = 1.into(),
        "in a snapshot",
    );
    reject::<HtManifest>(
        &v,
        &|v| v["codestreams"][snap]["openjpeg"]["agreement_bound"]["surprise"] = 1.into(),
        "in an agreement bound",
    );
    reject::<HtManifest>(
        &v,
        &|v| v["codestreams"][measured]["passes"]["measured"]["surprise"] = 1.into(),
        "in measured passes",
    );

    let text = std::fs::read_to_string(openjph_dir().join("manifest.json")).expect("read");
    serde_json::from_str::<OpenJphManifest>(&text).expect("committed manifest parses");
    let v: serde_json::Value = serde_json::from_str(&text).expect("json");
    reject::<OpenJphManifest>(
        &v,
        &|v| v["surprise"] = 1.into(),
        "at the OpenJPH top level",
    );
    reject::<OpenJphManifest>(
        &v,
        &|v| v["entries"][0]["surprise"] = 1.into(),
        "on an OpenJPH entry",
    );
}
