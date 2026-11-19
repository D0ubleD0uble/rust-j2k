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
const TARGET_HASH: u64 = 8717021346891458917;
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

// The fuzz targets that decode whole images skip an input that declares too
// many samples, so a run stays under the RSS limit, and cap the work their
// decodes do, so it stays under the timeout.
// No CI seed is near that size, so a skip that always or never fired would
// pass the smoke; this pins it from both sides.
#[path = "../fuzz/common/budget.rs"]
mod budget;

/// The budget of a target that holds one image at a time.
fn over_once(data: &[u8]) -> bool {
    budget::over(data, 1)
}

/// The side of a square image at the budget of [`over_once`].
const SIDE: u32 = 1 << 14;

#[test]
fn the_side_squares_to_the_budget() {
    assert_eq!(u64::from(SIDE) * u64::from(SIDE), budget::MAX_FUZZ_SAMPLES);
}

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
fn the_fuzz_skip_fires_just_over_the_budget_and_not_on_small_inputs() {
    // The budget is not over; one more row is.
    assert!(!over_once(&header_only(SIDE, SIDE)));
    assert!(over_once(&header_only(SIDE, SIDE + 1)));
    // The decoder's own default limit is at least twice that.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // On a 32-bit target this image is past the address cap, so the header
    // does not read and the skip does not apply.
    #[cfg(target_pointer_width = "64")]
    {
        let flat = std::fs::read(fixtures.join("sample_limit/flat_32769x16385_tiled_lossless.j2k"))
            .unwrap();
        assert!(over_once(&flat));
    }
    let small = std::fs::read(fixtures.join("regular_latlon_lossy.j2k")).unwrap();
    assert!(!over_once(&small));
    // 6000 x 4000 x 3 is 72M samples. The work cap bounds the time its
    // decodes take, so `decode` takes it.
    let rgb = std::fs::read(fixtures.join("sample_limit/rgb_6000x4000.jp2")).unwrap();
    assert!(!budget::over(&rgb, weight("decode")));
    assert!(!over_once(b"not a codestream"));
    assert!(!over_once(&[]));
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
    // The budget and a row more by SIZ, the budget by ihdr: the header does
    // not read.
    let over = jp2_with_ihdr(SIDE, SIDE, &header_only(SIDE, SIDE + 1));
    assert!(rust_j2k::read_header_with(&over, &opts).is_err());
    assert!(over_once(&over));
    // The same disagreement under the budget stays under it.
    let under = jp2_with_ihdr(SIDE, SIDE, &header_only(SIDE, SIDE - 1));
    assert!(rust_j2k::read_header_with(&under, &opts).is_err());
    assert!(!over_once(&under));
    // A box layer that agrees reads, and is over.
    let agrees = jp2_with_ihdr(SIDE, SIDE + 1, &header_only(SIDE, SIDE + 1));
    assert!(rust_j2k::read_header_with(&agrees, &opts).is_ok());
    assert!(over_once(&agrees));
    // Sub-sampling and an image offset shrink the count the way the decoder
    // sees it: 2 x 2 sub-sampling of 2 * SIDE x (2 * SIDE + 2) is SIDE x
    // (SIDE + 1) samples, and a vertical offset of 2 takes one row off.
    let (w, h) = (2 * SIDE, 2 * SIDE + 2);
    let sub = |yo: u32| {
        let siz = synth::siz_body([w, h, 0, yo, w, h, 0, yo], &[[7, 2, 2]]);
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
    assert!(over_once(&sub(0)));
    assert!(!over_once(&sub(2)));
}

/// A codestream main header of `comps` components, each `width` x `height`
/// with sub-sampling `xr` x `yr`, in one tile.
fn header_comps(width: u32, height: u32, comps: usize, xr: u8, yr: u8) -> Vec<u8> {
    let siz = synth::siz_body(
        [width, height, 0, 0, width, height, 0, 0],
        &vec![[7, xr, yr]; comps],
    );
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
fn the_fuzz_skip_counts_every_siz_component_of_a_palettized_file() {
    let bx = |kind: &[u8; 4], body: &[u8]| {
        [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
    };
    // 32 components of a quarter of the budget each; the palette maps only
    // component 0, so the header lists one output channel, under the budget.
    let cs = header_comps(SIDE / 2, SIDE / 2, 32, 1, 1);
    let ihdr = [
        &(SIDE / 2).to_be_bytes()[..],
        &(SIDE / 2).to_be_bytes(),
        &[0, 1, 7, 7, 0, 0],
    ]
    .concat();
    let colr = [&[1, 0, 0][..], &16u32.to_be_bytes()].concat();
    let pclr = [&[0, 2, 1, 7][..], &[0, 255]].concat();
    let cmap = [&[0, 0, 1, 0][..]].concat();
    let jp2 = [
        bx(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]),
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(
            b"jp2h",
            &[
                bx(b"ihdr", &ihdr),
                bx(b"colr", &colr),
                bx(b"pclr", &pclr),
                bx(b"cmap", &cmap),
            ]
            .concat(),
        ),
        bx(b"jp2c", &cs),
    ]
    .concat();
    let opts = rust_j2k::DecodeOptions::default().with_sample_limit(rust_j2k::WorkLimit::Unlimited);
    let header = rust_j2k::read_header_with(&jp2, &opts).expect("the palette file reads");
    assert_eq!(header.components.len(), 1);
    assert!(over_once(&jp2));
}

#[test]
fn the_fuzz_skip_counts_the_header_when_a_palette_makes_more_channels() {
    let bx = |kind: &[u8; 4], body: &[u8]| {
        [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
    };
    // One component of half the budget, which a palette maps to three
    // channels: SIZ counts half the budget, the header one and a half.
    let (width, height) = (SIDE, SIDE / 2);
    let cs = header_comps(width, height, 1, 1, 1);
    assert!(!over_once(&cs));
    let ihdr = [
        &height.to_be_bytes()[..],
        &width.to_be_bytes(),
        &[0, 1, 7, 7, 0, 0],
    ]
    .concat();
    let colr = [&[1, 0, 0][..], &16u32.to_be_bytes()].concat();
    let pclr = [&[0, 2, 3, 7, 7, 7][..], &[0, 0, 0, 255, 255, 255]].concat();
    let cmap = [0, 0, 1, 0, 0, 0, 1, 1, 0, 0, 1, 2];
    let jp2 = [
        bx(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]),
        bx(b"ftyp", b"jp2 \0\0\0\0jp2 "),
        bx(
            b"jp2h",
            &[
                bx(b"ihdr", &ihdr),
                bx(b"colr", &colr),
                bx(b"pclr", &pclr),
                bx(b"cmap", &cmap),
            ]
            .concat(),
        ),
        bx(b"jp2c", &cs),
    ]
    .concat();
    let opts = rust_j2k::DecodeOptions::default().with_sample_limit(rust_j2k::WorkLimit::Unlimited);
    let header = rust_j2k::read_header_with(&jp2, &opts).expect("the palette file reads");
    assert_eq!(header.components.len(), 3);
    assert_eq!(
        budget::declared(&jp2),
        3 * u128::from(budget::MAX_FUZZ_SAMPLES) / 2
    );
    assert!(over_once(&jp2));
}

#[test]
fn the_fuzz_skip_survives_zero_sub_sampling_decoys_and_repeated_markers() {
    // XRsiz = 0 is skipped, not divided by.
    assert!(!over_once(&jp2_with_ihdr(
        1,
        1,
        &header_comps(16, 16, 1, 0, 1)
    )));
    // A small pair ahead of the large real codestream does not hide it.
    let decoy = header_only(8, 8);
    let real = jp2_with_ihdr(SIDE, SIDE, &header_only(SIDE, SIDE + 1));
    assert!(over_once(&[decoy, real].concat()));
    // Repeated markers cost little: 256 KiB would take seconds if each pair
    // were parsed in full.
    let start = std::time::Instant::now();
    let repeated = [0xFF, 0x4F, 0xFF, 0x51].repeat(64 * 1024);
    assert!(!over_once(&repeated));
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn the_fuzz_budget_divides_by_the_weight_of_the_target() {
    // The budget shrinks with the weight.
    for weight in [1u64, 2, 4, 8] {
        let rows = u32::try_from(budget::MAX_FUZZ_SAMPLES / weight).unwrap();
        assert!(!budget::over(&header_only(1, rows), weight));
        assert!(budget::over(&header_only(1, rows + 1), weight));
    }
}

/// A codestream one sample wide and `rows` tall, in one tile, with 8
/// decomposition levels and 4 x 1024 code blocks: the shape that peaks highest
/// per sample (about 3.2 times its output) and has the most packets, each one
/// empty byte.
fn thin_codestream(rows: u32) -> Vec<u8> {
    let (levels, xb, yb) = (8u8, 0u8, 8u8);
    let siz = synth::siz_body([1, rows, 0, 0, 1, rows, 0, 0], &[[7, 1, 1]]);
    let mut out = [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &[0, 0, 0, 1, 0, levels, xb, yb, 0, 1]),
        synth::seg(
            0xFF5C,
            &synth::qcd_none_body(2, &vec![8; 3 * usize::from(levels) + 1]),
        ),
    ]
    .concat();
    // One packet per precinct of each resolution; a precinct is 2^15 square.
    let mut packets = 0usize;
    for r in 0..=u32::from(levels) {
        let d = 1u64 << (u32::from(levels) - r);
        packets += u64::from(rows).div_ceil(d).div_ceil(32768).max(1) as usize;
    }
    out.extend(synth::tile_part(0, 0, 1, &[], &vec![0u8; packets]));
    out.extend([0xFF, 0xD9]);
    out
}

/// A `side` x `side` codestream with an empty packet per resolution.
fn square_codestream(side: u32) -> Vec<u8> {
    square_ish_codestream(side, side)
}

/// A `width` x `height` codestream with an empty packet per resolution.
fn square_ish_codestream(width: u32, height: u32) -> Vec<u8> {
    let siz = synth::siz_body([width, height, 0, 0, width, height, 0, 0], &[[7, 1, 1]]);
    [
        vec![0xFF, 0x4F],
        synth::seg(0xFF51, &siz),
        synth::seg(0xFF52, &synth::cod_body(1, 5, 4, 0)),
        synth::seg(0xFF5C, &synth::qcd_none_body(2, &[8; 16])),
        synth::tile_part(0, 0, 1, &[], &[0u8; 6]),
        vec![0xFF, 0xD9],
    ]
    .concat()
}

/// A thin codestream of `samples` samples, padded with zeros to 16 KiB:
/// trailing zeros make the stateful targets feed and retry in more pieces.
fn thin(samples: u64) -> Vec<u8> {
    let mut file = thin_codestream(u32::try_from(samples).unwrap());
    file.resize(16 * 1024, 0);
    file
}

/// The body of a `jp2_boxes` input around `cs`, with variant bit 2 clear: a
/// whole file with its own `jp2c`, which wins over the pinned codestream.
fn boxes_body(cs: &[u8]) -> Vec<u8> {
    let ihdr = synth::ihdr_for(cs);
    let colr = synth::bx(b"colr", &synth::colr_enum(17));
    let body = [
        synth::JP2_SIGNATURE.to_vec(),
        synth::ftyp_jp2(),
        synth::bx(b"jp2h", &[ihdr, colr].concat()),
        synth::bx(b"jp2c", cs),
    ]
    .concat();
    [vec![0u8], body].concat()
}

/// A target's input around `file`, with the control bytes it reads first
/// (the default limits, no reduction, no window).
fn input_for(target: &str, file: Vec<u8>) -> Vec<u8> {
    match target {
        "retry_ladder" => [vec![0x02], file].concat(),
        "window_crop" => [vec![0; 8], file].concat(),
        "jp2_boxes" => boxes_body(&file),
        _ => file,
    }
}

/// The bytes a target hands the budget and its decodes, from its input:
/// `jp2_boxes` wraps its body around a pinned codestream, the others use the
/// file after any control bytes.
fn data_of(target: &str, input: &[u8]) -> Vec<u8> {
    match target {
        "jp2_boxes" => {
            synth::wrap_jp2_boxes(input[0], &fixture("regular_latlon_lossy.j2k"), &input[1..])
        }
        "retry_ladder" => input[1..].to_vec(),
        "window_crop" => input[8..].to_vec(),
        _ => input.to_vec(),
    }
}

/// The budgeted targets.
const BUDGETED: [&str; 6] = [
    "decode",
    "decode_partial",
    "incremental_decode",
    "jp2_boxes",
    "retry_ladder",
    "window_crop",
];

/// The file of `samples` samples for `target`'s inputs: thin, the shape that
/// peaks highest per sample, or for `jp2_boxes` square.
fn file_of(target: &str, samples: u64) -> Vec<u8> {
    if target == "jp2_boxes" {
        square_codestream(u32::try_from(samples.isqrt()).unwrap())
    } else {
        thin(samples)
    }
}

/// The inputs at each target's sample budget (#477): the target, the name of
/// the file `J2K_WRITE_FUZZ_BUDGET_INPUTS` writes, and the bytes the target
/// is given.
fn budget_inputs() -> Vec<(&'static str, String, Vec<u8>)> {
    BUDGETED
        .iter()
        .map(|&target| {
            let file = file_of(target, budget::samples(weight(target)));
            (target, format!("{target}_budget"), input_for(target, file))
        })
        .collect()
}

/// The inputs whose whole decode spends each target's work cap, to within a
/// sample, with the samples each declares: empty packets, so the work is
/// `CAP_WORK_PER_SAMPLE` for each sample. These are the slowest inputs the
/// cap lets through, as every decode reconstructs all of them.
fn cap_inputs() -> Vec<(&'static str, String, Vec<u8>, u64)> {
    BUDGETED
        .iter()
        .map(|&target| {
            let samples = budget::cap(work_weight(target)) / rust_j2k::fuzz::CAP_WORK_PER_SAMPLE;
            let file = file_of(target, samples);
            (
                target,
                format!("{target}_cap"),
                input_for(target, file),
                samples,
            )
        })
        .collect()
}

/// The weight each target passes to `budget::over`: the most images it
/// holds at once.
fn weight(target: &str) -> u64 {
    match target {
        "incremental_decode" => 8,
        "jp2_boxes" => 4,
        _ => 2,
    }
}

/// The weight each budgeted target passes to `budget::options`: how many
/// code-block bounds' worth of work one execution can spend (see
/// `budget::options`), so that the work cap divided by it keeps an
/// execution under the timeout.
fn work_weight(target: &str) -> u64 {
    match target {
        // See `incremental_checked_tries_stay_within_the_work_weight`.
        "incremental_decode" => 64,
        // The first decode at up to 2 bounds, the default 1, the steps 2
        // and 4, and the decode with no limits 4: 13.
        "retry_ladder" => 16,
        // The whole and the part.
        "window_crop" => 2,
        // The plain decode at 2 bounds and three more at 1.
        "decode" => 5,
        // Four decodes; `jp2_boxes` runs three.
        _ => 4,
    }
}

// Writes the inputs to a directory when `J2K_WRITE_FUZZ_BUDGET_INPUTS` names
// one, to run them through the real targets:
// `fuzz/target/*/release/<target> -rss_limit_mb=4096 -timeout=30 -runs=1 <file>`.
#[test]
fn the_inputs_at_each_fuzz_budget_read_and_are_not_skipped() {
    let opts = rust_j2k::DecodeOptions::default();
    let dir = std::env::var_os("J2K_WRITE_FUZZ_BUDGET_INPUTS");
    for (target, name, input) in budget_inputs() {
        let data = data_of(target, &input);
        let w = weight(target);
        assert_eq!(
            budget::declared(&data),
            u128::from(budget::samples(w)),
            "{name} sits exactly at the budget of {target}"
        );
        assert!(!budget::over(&data, w), "{name}, {target}");
        assert!(
            budget::over(&data, w * 2),
            "{name}, a budget twice as tight"
        );
        rust_j2k::read_header_with(&data, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
        // The work cap refuses it before any tile is reconstructed.
        assert_cap_refuses(&data, work_weight(target), &name);
        if let Some(dir) = &dir {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(Path::new(dir).join(&name), &input).unwrap();
        }
    }
}

/// Assert that a decode of `data` under the work cap of `work_weight` is
/// refused by the code-block work bound.
fn assert_cap_refuses(data: &[u8], work_weight: u64, name: &str) {
    match rust_j2k::decode_with(data, &budget::options(work_weight)) {
        Err(Error::Limit(limit)) => {
            assert_eq!(limit.kind(), rust_j2k::LimitKind::CodeBlockWork, "{name}")
        }
        other => panic!("{name}: {:?}", other.map(|_| ())),
    }
}

// Also written to `J2K_WRITE_FUZZ_BUDGET_INPUTS`.
#[test]
fn the_inputs_at_each_work_cap_fit_it_and_one_more_row_does_not() {
    let dir = std::env::var_os("J2K_WRITE_FUZZ_BUDGET_INPUTS");
    for (target, name, input, samples) in cap_inputs() {
        let data = data_of(target, &input);
        let w = work_weight(target);
        assert_eq!(budget::declared(&data), u128::from(samples), "{name}");
        assert!(!budget::over(&data, weight(target)), "{name}");
        // Empty packets: the work is the samples' charge alone, within a
        // sample of the cap.
        let per = rust_j2k::fuzz::CAP_WORK_PER_SAMPLE;
        assert!(samples * per <= budget::cap(w), "{name}");
        assert!((samples + 1) * per > budget::cap(w), "{name}");
        let more = if target == "jp2_boxes" {
            let side = u32::try_from(samples.isqrt()).unwrap();
            data_of(target, &boxes_body(&square_ish_codestream(side, side + 1)))
        } else {
            thin(samples + 1)
        };
        assert_cap_refuses(&more, w, &format!("{name} and a row"));
        if let Some(dir) = &dir {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(Path::new(dir).join(&name), &input).unwrap();
        }
    }
}

/// Hash of `fuzz/fuzz_targets/incremental_decode.rs` from its `fuzz_target!`
/// to the end, comments left out: the pieces, the due rule, the counter of
/// checked tries and the draws of the changes and the probe, which
/// `incremental_checked_tries_stay_within_the_work_weight` relies on. If it
/// changed, check that test's count of bounds, then update this hash.
const INCREMENTAL_HASH: u64 = 15289928561592361447;

#[test]
fn incremental_checked_tries_stay_within_the_work_weight() {
    let target = include_str!("../fuzz/fuzz_targets/incremental_decode.rs").replace('\r', "");
    let body: String = target[target.find("fuzz_target!(").expect("the target")..]
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        fnv(&body),
        INCREMENTAL_HASH,
        "incremental_decode's schedule changed: check the bounds below, then update INCREMENTAL_HASH"
    );
    let line = target
        .lines()
        .find(|l| l.starts_with("const MAX_CHECKED_TRIES: u32 = "))
        .expect("the count of checked tries");
    let checked: u64 = line["const MAX_CHECKED_TRIES: u32 = ".len()..]
        .trim_end_matches(';')
        .parse()
        .unwrap();
    // Each checked try runs up to three decodes besides the refine (the
    // one-shot, `zero` and the probe) at up to 4 bounds each; the refines
    // share 4 bounds, and `finish` and the last `decode_with` take 4 each.
    // A try past the count only refines, whatever the input's length. The
    // reason a refine waits is checked from its result and the one-shot
    // decode already run, so the check adds no decode; a refine that waits
    // on what earlier refines spent counts the work of the tiles past the
    // cut without decoding them, within the bounds a refine has.
    assert!(
        12 * checked + 4 + 8 <= work_weight("incremental_decode"),
        "{checked} checked tries"
    );
}

#[test]
fn each_fuzz_target_passes_the_weight_this_file_pins() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/fuzz_targets");
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let target = path.file_stem().unwrap().to_str().unwrap().to_string();
        // Comments don't count: a call left behind in one is not a call.
        let source: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let calls: Vec<&str> = source
            .match_indices("budget::over(")
            .map(|(i, _)| &source[i..])
            .collect();
        if target == "header" {
            assert!(
                source.contains("budget::declared(data)"),
                "header must bound its decode by the SIZ count"
            );
            assert!(calls.is_empty(), "header uses `declared`");
            continue;
        }
        if calls.is_empty() {
            // Targets that decode no whole image. A new target is not on this
            // list until someone decides it needs no budget.
            assert!(
                ["tile_body", "icc_srgb", "mq", "tier1_block", "ht_block"]
                    .contains(&target.as_str()),
                "{target} calls neither `budget::over` nor `budget::declared`; \
                 budget it, or add it to the unbudgeted list"
            );
            continue;
        }
        assert_eq!(calls.len(), 1, "{target}: one budget call");
        let call = calls[0];
        let args = &call[..call.find(')').unwrap()];
        let n: u64 = args.rsplit(',').next().unwrap().trim().parse().unwrap();
        assert_eq!(n, weight(&target), "{target}: weight in the target");
        // Every decode starts from the capped options, made once: none from
        // the defaults, which have no cap.
        let caps: Vec<&str> = source
            .match_indices("budget::options(")
            .map(|(i, _)| &source[i + "budget::options(".len()..])
            .collect();
        assert_eq!(caps.len(), 1, "{target}: one `budget::options` call");
        let n: u64 = caps[0][..caps[0].find(')').unwrap()]
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            n,
            work_weight(&target),
            "{target}: work weight in the target"
        );
        if target == "decode" {
            // The plain decode takes 2 of the 5 bounds.
            assert!(
                source.contains(".with_code_block_work_limit(rust_j2k::WorkLimit::Raised(2))"),
                "decode: the plain decode at 2 bounds"
            );
        }
        for uncapped in ["DecodeOptions::default()", "rust_j2k::decode("] {
            assert!(
                !source.contains(uncapped),
                "{target}: `{uncapped}` decodes without the fuzz cap; start from `budget::options`"
            );
        }
        seen.push(target);
    }
    seen.sort();
    let mut expected: Vec<&str> = BUDGETED.to_vec();
    expected.sort();
    assert_eq!(seen, expected, "every budgeted target has an input above");
}
