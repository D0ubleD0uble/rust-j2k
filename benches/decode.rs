//! Decode benchmark over committed fixtures, using only `std`.
//!
//! Run with `cargo bench --bench decode`. See "Benchmarks" in CONTRIBUTING.md.
//!
//! Environment:
//! - `RUST_J2K_BENCH_ITERS`: timed iterations per file (default 10).
//! - `RUST_J2K_BENCH_DIR`: also time every `.j2k`, `.j2c`, `.jp2` and `.jpx`
//!   file in this directory, in name order, after the built-in set.
//!
//! Any command-line argument that does not start with `-` is a substring
//! filter on the file name (cargo passes its own `--bench` flag, which is
//! ignored).
//!
//! The conformance and `jp2_real` fixtures are left out of the published
//! package, so every file is read at run time and a missing one is skipped
//! with a note.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rust_j2k::{DecodeOptions, decode_with};

/// The built-in set, in the order it is printed. Paths are relative to the
/// crate root.
const FIXTURES: &[&str] = &[
    "tests/fixtures/conformance/codestreams/p0_07.j2k",
    "tests/fixtures/conformance/codestreams/p0_08.j2k",
    "tests/fixtures/conformance/codestreams/p0_05.j2k",
    "tests/fixtures/conformance/codestreams/p1_03.j2k",
    "tests/fixtures/conformance/jp2/files/file7.jp2",
    "tests/fixtures/jpeg2000_regular_latlon.j2k",
];

fn main() {
    let iters = std::env::var("RUST_J2K_BENCH_ITERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(10);
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut files: Vec<PathBuf> = FIXTURES.iter().map(|rel| root.join(rel)).collect();
    if let Some(dir) = std::env::var_os("RUST_J2K_BENCH_DIR") {
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                let mut extra: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| matches!(e, "j2k" | "j2c" | "jp2" | "jpx"))
                    })
                    .collect();
                extra.sort();
                files.extend(extra);
            }
            Err(e) => eprintln!("note: cannot read RUST_J2K_BENCH_DIR: {e}"),
        }
    }

    let options = DecodeOptions::default();
    println!("rust-j2k decode bench: 1 warm-up, then {iters} timed iterations; times in ms");
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9}",
        "file", "median", "min", "max", "KiB"
    );
    for path in &files {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!("{name:<28} skipped: cannot read {} ({e})", path.display());
                continue;
            }
        };
        // The warm-up decode.
        if let Err(e) = decode_with(&bytes, &options) {
            println!("{name:<28} skipped: decode failed ({e})");
            continue;
        }
        let mut times: Vec<f64> = (0..iters)
            .map(|_| {
                let start = Instant::now();
                let image = decode_with(black_box(&bytes), black_box(&options));
                let elapsed = start.elapsed();
                drop(black_box(image));
                elapsed.as_secs_f64() * 1e3
            })
            .collect();
        times.sort_by(f64::total_cmp);
        let mid = times.len() / 2;
        let median = if times.len() % 2 == 1 {
            times[mid]
        } else {
            (times[mid - 1] + times[mid]) / 2.0
        };
        println!(
            "{:<28} {:>9.2} {:>9.2} {:>9.2} {:>9}",
            name,
            median,
            times[0],
            times[times.len() - 1],
            bytes.len().div_ceil(1024)
        );
    }
}
