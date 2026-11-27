//! The `decode` example writes the samples the oracle snapshots hold.
//!
//! `examples/common/pnm.rs` is the writer both `examples/decode.rs` and this
//! test include. Part one decodes a fixture, encodes it with the writer, parses
//! the PGM or PPM back with a reader written here, and compares every sample
//! with the fixture's `.expected.json`, which OpenJPEG produced. Part two runs
//! the `decode` example (through `cargo run --example`, since Cargo has no
//! path variable for examples) on the same files and checks its output file
//! the same way.

mod support;

#[path = "../examples/common/pnm.rs"]
mod pnm;

use std::path::{Path, PathBuf};
#[cfg(not(target_family = "wasm"))]
use std::process::Command;

use support::Expected;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn expected(stem: &str) -> Expected {
    let path = fixture(&format!("{stem}.expected.json"));
    Expected::from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// What a PGM or PPM reader sees: the magic, size, maxval, and the samples.
struct Pnm {
    magic: String,
    width: u32,
    height: u32,
    maxval: u32,
    samples: Vec<u32>,
}

/// Parse a binary PGM or PPM as the Netpbm specification lays it out: four
/// whitespace-separated header tokens, one whitespace byte, then the raster.
fn parse_pnm(bytes: &[u8]) -> Pnm {
    let mut at = 0;
    let mut token = || {
        while bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        String::from_utf8(bytes[start..at].to_vec()).unwrap()
    };
    let magic = token();
    let width: u32 = token().parse().unwrap();
    let height: u32 = token().parse().unwrap();
    let maxval: u32 = token().parse().unwrap();
    let raster = &bytes[at + 1..];
    let channels = if magic == "P5" { 1 } else { 3 };
    assert!((1..=65535).contains(&maxval));
    let wide = maxval > 255;
    let count = (width * height) as usize * channels;
    assert_eq!(
        raster.len(),
        count * if wide { 2 } else { 1 },
        "raster size"
    );
    let samples = if wide {
        raster
            .chunks(2)
            .map(|b| u32::from(u16::from_be_bytes([b[0], b[1]])))
            .collect()
    } else {
        raster.iter().map(|&b| u32::from(b)).collect()
    };
    Pnm {
        magic,
        width,
        height,
        maxval,
        samples,
    }
}

/// Check a parsed PNM against the snapshot, sample by sample.
fn assert_matches(pnm: &Pnm, snapshot: &Expected, magic: &str) {
    let first = &snapshot.components[0];
    assert_eq!(pnm.magic, magic);
    assert_eq!((pnm.width, pnm.height), (first.width, first.height));
    assert_eq!(pnm.maxval, (1u32 << first.bit_depth) - 1);
    let offset = if first.signed {
        1i64 << (first.bit_depth - 1)
    } else {
        0
    };
    let channels = snapshot.components.len();
    assert_eq!(pnm.samples.len(), first.samples.len() * channels);
    for (i, got) in pnm.samples.iter().enumerate() {
        let plane = &snapshot.components[i % channels];
        let want = i64::from(plane.samples[i / channels]) + offset;
        assert_eq!(i64::from(*got), want, "sample {i}");
    }
}

/// (fixture stem, extension, magic): one channel at 8 bits, three channels,
/// 13 bits (written as two bytes), unsigned 16 bits, and signed 16 bits
/// (shifted to unsigned).
const CASES: [(&str, &str, &str); 5] = [
    ("multilayer_lossless", "j2k", "P5"),
    ("multicomponent_lossless", "j2k", "P6"),
    ("eta_lambert_lossless", "j2k", "P5"),
    ("jpeg2000_regular_latlon", "j2k", "P5"),
    ("jp2_signed16", "jp2", "P5"),
];

#[test]
fn encoded_bytes_match_the_oracle_samples() {
    for (stem, ext, magic) in CASES {
        let bytes = std::fs::read(fixture(&format!("{stem}.{ext}"))).unwrap();
        let image = rust_j2k::decode(&bytes).unwrap();
        let encoded = pnm::encode(&image).unwrap();
        assert_matches(&parse_pnm(&encoded), &expected(stem), magic);
    }
}

#[test]
fn the_writer_refuses_what_a_pnm_file_cannot_hold() {
    // Four channels, and channels of different sizes (subsampled).
    for stem in ["jp2_cdef_rgba", "jp2_sycc_420_even"] {
        let bytes = std::fs::read(fixture(&format!("{stem}.jp2"))).unwrap();
        let image = rust_j2k::decode(&bytes).unwrap();
        assert!(pnm::encode(&image).is_err(), "{stem}");
    }
    // A depth past 16 bits.
    let bytes = std::fs::read(fixture("multilayer_lossless.j2k")).unwrap();
    let mut image = rust_j2k::decode(&bytes).unwrap();
    image.components[0].bit_depth = 24;
    assert!(pnm::encode(&image).unwrap_err().contains("24-bit"));
}

#[test]
fn the_writer_clamps_values_outside_the_declared_depth() {
    // A palette channel can hold a value past its depth.
    let samples = vec![300, -5, 7];
    let component = rust_j2k::Component::new(3, 1, 8, false, 1, 1, samples);
    let image = rust_j2k::Image::new(3, 1, vec![component]);
    let encoded = pnm::encode(&image).unwrap();
    assert!(encoded.starts_with(b"P5\n3 1\n255\n"));
    assert_eq!(&encoded[encoded.len() - 3..], [255, 0, 7]);
}

/// Run the `decode` example through `cargo run`, since Cargo gives a test no
/// path to an example. The nested build compiles the example binary itself
/// (about 0.2 s), because a run of one test target or `--all-targets` may not
/// build it. `--profile test` reuses the library the tests already built, so
/// only the example is compiled and then launched. The run follows
/// `CARGO_TARGET_DIR` from the environment, but not a
/// `--target-dir` flag given to the outer `cargo test`.
#[cfg(not(target_family = "wasm"))]
fn run_decode(input: &Path, output: &Path) -> std::process::Output {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "--quiet", "--offline", "--profile", "test"])
        .args(["--example", "decode", "--"])
        .arg(input)
        .arg(output)
        .output()
        .unwrap()
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn the_decode_example_writes_the_oracle_samples() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for (stem, ext, magic) in CASES {
        let out = dir.join(format!("{stem}.pnm"));
        let status = run_decode(&fixture(&format!("{stem}.{ext}")), &out);
        assert!(status.status.success(), "{stem}: {status:?}");
        let bytes = std::fs::read(&out).unwrap();
        assert_matches(&parse_pnm(&bytes), &expected(stem), magic);
    }
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn the_decode_example_fails_clearly_on_four_channels() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rgba.pnm");
    let _ = std::fs::remove_file(&out);
    let run = run_decode(&fixture("jp2_cdef_rgba.jp2"), &out);
    assert!(!run.status.success());
    let message = String::from_utf8_lossy(&run.stderr);
    assert!(message.contains("1 or 3 channels"), "{message}");
    assert!(message.contains("jp2_cdef_rgba.jp2"), "{message}");
    assert!(!out.exists());
}
