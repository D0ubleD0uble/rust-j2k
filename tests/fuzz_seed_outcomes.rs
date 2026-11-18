//! Pins what each `jp2_boxes` fuzz seed decodes to.
//!
//! `scripts/gen-jp2-fuzz-seeds.py` writes the seeds the CI fuzz smoke starts
//! from, and the smoke only asserts that nothing panics. If the script (or the
//! `ihdr` it writes) drifted so that every seed failed at the box layer, the
//! smoke would still pass while reaching none of `src/jp2/srgb.rs`. This test
//! runs the script into a scratch directory, wraps each seed the way
//! `fuzz/fuzz_targets/jp2_boxes.rs` does, and checks two things:
//!
//! - every seed decodes as coded (with the default color output), and its
//!   `ihdr` declares the pinned codestream's size and component count (the
//!   decoder checks the size but, like OpenJPEG, not the component count, so
//!   a seed with a wrong count would still decode);
//! - its `ColorOutput::Srgb` outcome matches
//!   `tests/fixtures/jp2_fuzz_seed_outcomes.tsv`, which also fixes the set of
//!   seed names.
//!
//! The codestream table below copies the target's; keep them in step.
//! Regenerate the table after an intended change with
//! `J2K_BLESS_SEED_OUTCOMES=1 cargo test --test fuzz_seed_outcomes`.
//! The script needs `python3` (standard library only), run with `-I`.

#[path = "support/synth.rs"]
mod synth;
use rust_j2k::{ColorOutput, ColorSpace, DecodeOptions, Error, Image, decode_with};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const TABLE: &str = "tests/fixtures/jp2_fuzz_seed_outcomes.tsv";

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn jp2c_payload(file: &[u8]) -> Vec<u8> {
    let mut at = 0;
    while at + 8 <= file.len() {
        let len = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        if &file[at + 4..at + 8] == b"jp2c" {
            return file[at + 8..at + len].to_vec();
        }
        at += len;
    }
    panic!("no jp2c box");
}

/// The pinned codestream for a variant byte (the target's table).
fn codestream(variant: u8) -> Vec<u8> {
    match (variant >> 4) & 7 {
        1 => fixture("multicomponent_subsampled_lossless.j2k"),
        2 => fixture("derived_quant_lossy.j2k"),
        3 => fixture("multicomponent_lossless.j2k"),
        4 => jp2c_payload(&fixture("jp2_sycc_420_odd.jp2")),
        5 => jp2c_payload(&fixture("jp2_sycc_422_odd.jp2")),
        6 => fixture("multicomponent_16bit_lossless.j2k"),
        7 => jp2c_payload(&fixture("jp2_sycc_420_even.jp2")),
        _ => fixture("regular_latlon_lossy.j2k"),
    }
}

/// The file the target builds from a seed (its `wrap`).
fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    synth::wrap_jp2_boxes(variant, &codestream(variant), body)
}

/// The first `ihdr` in the file's top-level `jp2h`, as (height, width, NC).
fn ihdr_of(file: &[u8]) -> Option<(u32, u32, u16)> {
    let be32 = |at: usize| u32::from_be_bytes(file[at..at + 4].try_into().unwrap());
    let mut at = 0;
    while at + 8 <= file.len() {
        let len = be32(at) as usize;
        if len < 8 {
            return None;
        }
        if &file[at + 4..at + 8] == b"jp2h" {
            let end = (at + len).min(file.len());
            let mut child = at + 8;
            while child + 8 <= end {
                let clen = be32(child) as usize;
                if &file[child + 4..child + 8] == b"ihdr" && child + 18 <= end {
                    let nc = u16::from_be_bytes(file[child + 16..child + 18].try_into().unwrap());
                    return Some((be32(child + 8), be32(child + 12), nc));
                }
                if clen < 8 {
                    return None;
                }
                child += clen;
            }
            return None;
        }
        at += len;
    }
    None
}

/// `ok <color space> <channels>`, or `<Error variant>: <message>`.
fn describe(result: Result<Image, Error>) -> (bool, String) {
    match result {
        Ok(image) => {
            let color = match image.jp2.as_ref().map(|m| &m.color) {
                Some(ColorSpace::RestrictedIcc(_)) => "Icc".to_string(),
                Some(other) => format!("{other:?}"),
                None => "raw".to_string(),
            };
            (true, format!("ok {color} {}", image.components.len()))
        }
        Err(e) => {
            let debug = format!("{e:?}");
            let variant = debug.split('(').next().unwrap();
            let text = e.to_string();
            let message = text.split_once(": ").map_or(text.as_str(), |(_, m)| m);
            (false, format!("{variant}: {message}"))
        }
    }
}

/// FNV-1a over a source region with `\r` removed, so a CRLF checkout hashes
/// the same.
fn fnv(text: &str) -> u64 {
    text.bytes()
        .filter(|&b| b != b'\r')
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// Hash of `fuzz/fuzz_targets/jp2_boxes.rs` from its `synth` include to
/// the end of the file, which this file copies. If it changed, update the
/// copy here, then both hashes.
const TARGET_HASH: u64 = 5019425586612062705;
/// Hash of this file's copy and its use (from the `synth` include up to the test),
/// without the two hash lines. An edit to either copy fails until both
/// are reviewed.
/// Hash of `tests/support/synth.rs` from its `ihdr_for` to the end of the
/// file (`ihdr_for` and `wrap_jp2_boxes`), which the target and this test
/// share. If it changed, check both callers, then update this hash.
const SYNTH_HASH: u64 = 13200082622892353052;
const TEST_HASH: u64 = 902390728654288137;

fn target_hash() -> u64 {
    let target = include_str!("../fuzz/fuzz_targets/jp2_boxes.rs");
    let start = target
        .find("#[path = \"../../tests/support/synth.rs\"]")
        .expect("target include");
    fnv(&target[start..])
}

fn synth_hash() -> u64 {
    let synth = include_str!("support/synth.rs");
    let start = synth
        .find("// `ihdr_for` and `wrap_jp2_boxes` must stay")
        .expect("the shared wrap");
    fnv(&synth[start..])
}

fn test_hash() -> u64 {
    let this = include_str!("fuzz_seed_outcomes.rs").replace('\r', "");
    let start = this
        .find("#[path = \"support/synth.rs\"]")
        .expect("the copy");
    let end = this.find("\n#[test]\n").expect("the test");
    assert!(start < end, "the copy must come before the test");
    let region: String = this[start..end]
        .lines()
        .filter(|l| {
            !l.starts_with("const TARGET_HASH")
                && !l.starts_with("const SYNTH_HASH")
                && !l.starts_with("const TEST_HASH")
        })
        .collect::<Vec<_>>()
        .join("\n");
    fnv(&region)
}

/// Each seed's name, a description of what is wrong with it as coded (empty if
/// nothing), and its sRGB outcome. `None` if `python3` is not installed.
fn outcomes() -> Option<BTreeMap<String, (String, String)>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("jp2_fuzz_seeds_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = match Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/gen-jp2-fuzz-seeds.py"))
        .arg(&dir)
        .output()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            assert!(
                std::env::var_os("CI").is_none(),
                "python3 is not on PATH, and this test must run in CI"
            );
            eprintln!("fuzz_seed_outcomes: SKIPPED, python3 is not installed");
            return None;
        }
        Err(e) => panic!("run python3: {e}"),
    };
    assert!(
        out.status.success(),
        "gen-jp2-fuzz-seeds.py failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut map = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        let data = std::fs::read(entry.path()).unwrap();
        let (&variant, body) = data.split_first().expect("a seed holds a variant byte");
        let file = wrap(variant, body);
        let options = DecodeOptions::default().with_resolution_reduction(variant >> 7);
        let (decodes, coded) = describe(decode_with(&file, &options));
        let cs = codestream(variant);
        let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
        let area = (
            be32(12) - be32(20),
            be32(8) - be32(16),
            u16::from_be_bytes(cs[40..42].try_into().unwrap()),
        );
        let problem = if !decodes {
            coded
        } else if ihdr_of(&file) != Some(area) {
            "ihdr is not the codestream's".to_string()
        } else {
            String::new()
        };
        let options = options.with_color(ColorOutput::Srgb);
        let (_, srgb) = describe(decode_with(&file, &options));
        map.insert(name, (problem, srgb));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Some(map)
}

#[test]
fn every_seed_decodes_as_coded_and_srgb_matches_the_table() {
    assert_eq!(
        target_hash(),
        TARGET_HASH,
        "fuzz/fuzz_targets/jp2_boxes.rs changed: update the copy in tests/fuzz_seed_outcomes.rs, then both hashes"
    );
    assert_eq!(
        synth_hash(),
        SYNTH_HASH,
        "tests/support/synth.rs's wrap_jp2_boxes or ihdr_for changed: check the target and this test, then update SYNTH_HASH and TEST_HASH"
    );
    assert_eq!(
        test_hash(),
        TEST_HASH,
        "tests/fuzz_seed_outcomes.rs's copy of the target changed: check it against fuzz/fuzz_targets/jp2_boxes.rs, then update both hashes"
    );
    let Some(got) = outcomes() else { return };
    let table_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE);

    let broken: Vec<_> = got
        .iter()
        .filter(|(_, (problem, _))| !problem.is_empty())
        .map(|(n, (problem, _))| format!("{n}: {problem}"))
        .collect();
    assert!(
        broken.is_empty(),
        "seeds that do not decode as coded or whose `ihdr` is not the codestream's:\n{}",
        broken.join("\n")
    );

    if std::env::var_os("J2K_BLESS_SEED_OUTCOMES").is_some_and(|v| v == "1") {
        let mut text = String::from("# seed<TAB>sRGB outcome (tests/fuzz_seed_outcomes.rs)\n");
        for (name, (_, srgb)) in &got {
            text.push_str(&format!("{name}\t{srgb}\n"));
        }
        std::fs::write(&table_path, text).unwrap();
        return;
    }

    let table = std::fs::read_to_string(&table_path).expect("the committed outcome table");
    let want: BTreeMap<&str, &str> = table
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.split_once('\t').expect("name<TAB>outcome"))
        .collect();

    let mut diffs = Vec::new();
    for (name, (_, srgb)) in &got {
        match want.get(name.as_str()) {
            None => diffs.push(format!("{name}: not in the table (got {srgb})")),
            Some(w) if w != srgb => diffs.push(format!("{name}: table {w:?}, got {srgb:?}")),
            _ => {}
        }
    }
    for name in want.keys() {
        if !got.contains_key(*name) {
            diffs.push(format!("{name}: in the table, no longer generated"));
        }
    }
    assert!(
        diffs.is_empty(),
        "seed outcomes changed:\n{}",
        diffs.join("\n")
    );
}

// The fuzz targets `decode`, `decode_partial` and `incremental_decode` skip an
// input that declares too many samples, so a run stays under the RSS limit.
// No CI seed is near that size, so a skip that always or never fired would
// pass the smoke; this pins it from both sides.
#[path = "../fuzz/common/budget.rs"]
mod budget;

/// A codestream main header and first `SOT` marker of one component,
/// `width` x `height`, in one tile.
fn header_only(width: u32, height: u32) -> Vec<u8> {
    let siz = synth::siz_body([width, height, 0, 0, width, height, 0, 0], &[[7, 1, 1]]);
    [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 6, 0, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 19])),
        vec![0xFF, 0x90],
    ]
    .concat()
}

#[test]
fn the_fuzz_skip_fires_just_over_2_pow_28_samples_and_not_on_small_inputs() {
    // 2^28 samples is not over; one more row is.
    assert!(!budget::over(&header_only(16384, 16384)));
    assert!(budget::over(&header_only(16384, 16385)));
    // The decoder's own default limit is twice that, so this decodes on its own.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // On a 32-bit target this image is past the address cap, so the header
    // does not read and the skip does not apply.
    #[cfg(target_pointer_width = "64")]
    {
        let flat = std::fs::read(fixtures.join("sample_limit/flat_32769x16385_tiled_lossless.j2k"))
            .unwrap();
        assert!(budget::over(&flat));
    }
    let rgb = std::fs::read(fixtures.join("sample_limit/rgb_6000x4000.jp2")).unwrap();
    assert!(!budget::over(&rgb));
    assert!(!budget::over(b"not a codestream"));
    assert!(!budget::over(&[]));
}

/// A JP2 file around `codestream` whose `ihdr` box declares `width` x `height`
/// of one 8-bit component, whatever the codestream says.
fn jp2_with_ihdr(width: u32, height: u32, codestream: &[u8]) -> Vec<u8> {
    let bx = |kind: &[u8; 4], body: &[u8]| {
        [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
    };
    let ihdr = [
        &height.to_be_bytes()[..],
        &width.to_be_bytes(),
        &[0, 1, 7, 7, 0, 0],
    ]
    .concat();
    let colr = [&[1, 0, 0][..], &16u32.to_be_bytes()].concat();
    [
        bx(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]),
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(b"jp2h", &[bx(b"ihdr", &ihdr), bx(b"colr", &colr)].concat()),
        bx(b"jp2c", codestream),
    ]
    .concat()
}

#[test]
fn the_fuzz_skip_counts_siz_when_the_ihdr_box_disagrees() {
    let opts = rust_j2k::DecodeOptions::default().with_sample_limit(rust_j2k::WorkLimit::Unlimited);
    // 2^28 + 16384 samples by SIZ, 2^28 by ihdr: the header does not read.
    let over = jp2_with_ihdr(16384, 16384, &header_only(16384, 16385));
    assert!(rust_j2k::read_header_with(&over, &opts).is_err());
    assert!(budget::over(&over));
    // The same disagreement under the budget stays under it.
    let under = jp2_with_ihdr(16384, 16384, &header_only(16384, 16383));
    assert!(rust_j2k::read_header_with(&under, &opts).is_err());
    assert!(!budget::over(&under));
    // A box layer that agrees reads, and the header path counts it.
    assert!(budget::over(&jp2_with_ihdr(
        16384,
        16385,
        &header_only(16384, 16385)
    )));
    // Sub-sampling and an image offset shrink the count the way the decoder
    // sees it: 2 x 2 sub-sampling of 32768 x 32770 is 16384 x 16385 samples,
    // and a vertical offset of 2 takes one row off.
    let sub = |yo: u32| {
        let siz = synth::siz_body([32768, 32770, 0, yo, 32768, 32770, 0, yo], &[[7, 2, 2]]);
        let cs = [
            vec![0xFF, 0x4F],
            synth::seg(0xFF51, &siz),
            synth::seg(0xFF52, &synth::cod_body(1, 6, 0, 0)),
            synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 19])),
            vec![0xFF, 0x90],
        ]
        .concat();
        jp2_with_ihdr(1, 1, &cs)
    };
    assert!(budget::over(&sub(0)));
    assert!(!budget::over(&sub(2)));
}
