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
//! `wrap` and the codestream table below copy the target's; keep them in step.
//! Regenerate the table after an intended change with
//! `J2K_BLESS_SEED_OUTCOMES=1 cargo test --test fuzz_seed_outcomes`.
//! The script needs `python3` (standard library only), run with `-I`.

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

const SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// A box with a 32-bit `LBox`.
fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// An `ihdr` matching a codestream's SIZ (as the fuzz target writes it).
fn ihdr_for(codestream: &[u8]) -> Vec<u8> {
    let be32 = |at: usize| u32::from_be_bytes(codestream[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, x0, y0) = (be32(8), be32(12), be32(16), be32(20));
    let mut payload = (ysiz - y0).to_be_bytes().to_vec();
    payload.extend_from_slice(&(xsiz - x0).to_be_bytes());
    payload.extend_from_slice(&codestream[40..42]);
    payload.extend_from_slice(&[7, 7, 0, 0]);
    bx(b"ihdr", &payload)
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
    let codestream = codestream(variant);
    let mut file = Vec::new();
    if variant & 0x04 != 0 {
        file.extend_from_slice(&SIGNATURE);
        file.extend_from_slice(&bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
        if variant & 0x08 != 0 {
            let mut children = ihdr_for(&codestream);
            children.extend_from_slice(body);
            file.extend_from_slice(&bx(b"jp2h", &children));
        } else {
            file.extend_from_slice(body);
        }
    } else {
        file.extend_from_slice(body);
    }
    match variant & 0x03 {
        0 => file.extend_from_slice(&bx(b"jp2c", &codestream)),
        1 => {
            file.extend_from_slice(&1u32.to_be_bytes());
            file.extend_from_slice(b"jp2c");
            file.extend_from_slice(&(16 + codestream.len() as u64).to_be_bytes());
            file.extend_from_slice(&codestream);
        }
        2 => {
            file.extend_from_slice(&0u32.to_be_bytes());
            file.extend_from_slice(b"jp2c");
            file.extend_from_slice(&codestream);
        }
        _ => {
            file.extend_from_slice(&bx(b"jp2c", &codestream));
            file.extend_from_slice(body);
        }
    }
    file
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

/// Each seed's name, whether it decodes as coded, and its sRGB outcome.
fn outcomes() -> BTreeMap<String, (bool, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("jp2_fuzz_seeds");
    let _ = std::fs::remove_dir_all(&dir);
    let out = Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/gen-jp2-fuzz-seeds.py"))
        .arg(&dir)
        .output()
        .expect("run python3 to generate the seeds");
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
        let (decodes, _) = describe(decode_with(&file, &options));
        let cs = codestream(variant);
        let be32 = |at: usize| u32::from_be_bytes(cs[at..at + 4].try_into().unwrap());
        let area = (
            be32(12) - be32(20),
            be32(8) - be32(16),
            u16::from_be_bytes(cs[40..42].try_into().unwrap()),
        );
        let coded_ok = decodes && ihdr_of(&file) == Some(area);
        let options = options.with_color(ColorOutput::Srgb);
        let (_, srgb) = describe(decode_with(&file, &options));
        map.insert(name, (coded_ok, srgb));
    }
    map
}

#[test]
fn every_seed_decodes_as_coded_and_srgb_matches_the_table() {
    let got = outcomes();
    let table_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(TABLE);

    let broken: Vec<_> = got
        .iter()
        .filter(|(_, (ok, _))| !ok)
        .map(|(n, _)| n)
        .collect();
    assert!(
        broken.is_empty(),
        "seeds that do not decode as coded or whose `ihdr` is not the codestream's:{broken:?}"
    );

    if std::env::var_os("J2K_BLESS_SEED_OUTCOMES").is_some() {
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
