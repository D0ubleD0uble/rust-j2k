//! The `thumbnail` and `stream` examples do what their headers say.
//!
//! Both run through `cargo run --example`, since Cargo has no path variable
//! for examples (see `tests/examples_pnm.rs`). `thumbnail` is checked on the
//! sizes the reduction and the window give, and on its bytes against
//! `decode_with` and the shared PNM writer. `stream` is checked on its
//! printed steps and on the final image it compares with `decode`.

#![cfg(not(target_family = "wasm"))]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rust_j2k::{DecodeOptions, Region, Window, decode_with, read_header};

#[path = "../examples/common/pnm.rs"]
mod pnm;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Run an example through `cargo run`; the nested build reuses the library
/// the tests built (`--profile test`, as in `tests/examples_pnm.rs`).
fn run_example(example: &str, args: &[&OsStr]) -> Output {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "--quiet", "--offline", "--profile", "test"])
        .args(["--example", example, "--"])
        .args(args)
        .output()
        .unwrap()
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// The width and height in a PGM or PPM header (the second and third
/// whitespace-separated tokens).
fn pnm_size(bytes: &[u8]) -> (u32, u32) {
    let blank = |b: &u8| b.is_ascii_whitespace();
    let tokens: Vec<&[u8]> = bytes.split(blank).take(3).collect();
    let num = |t: &[u8]| std::str::from_utf8(t).unwrap().parse().unwrap();
    (num(tokens[1]), num(tokens[2]))
}

fn run_thumbnail(stem: &str, rest: &[&str], out: &Path) -> Output {
    let input = fixture(stem);
    let mut args: Vec<&OsStr> = vec![input.as_os_str()];
    args.extend(rest.iter().map(OsStr::new));
    args.push(out.as_os_str());
    run_example("thumbnail", &args)
}

/// Both fixtures have at least 4 resolution levels (a largest reduction of
/// 3), and several quality layers.
const PRECINCTS: &str = "precincts_varied_sizes_lossless.j2k";
const FILES: [&str; 2] = [PRECINCTS, "layers/lrcp.j2k"];

#[test]
fn thumbnail_at_two_levels_is_a_quarter_size_rounded_up() {
    for stem in FILES {
        let bytes = std::fs::read(fixture(stem)).unwrap();
        let header = read_header(&bytes).unwrap();
        assert!(header.max_reduction >= 3, "{stem}");
        let out = out_path("thumb_two_levels.pnm");
        let _ = std::fs::remove_file(&out);
        let run = run_thumbnail(stem, &["2"], &out);
        assert!(run.status.success(), "{stem}: {run:?}");
        let written = std::fs::read(&out).unwrap();
        let want = (header.width.div_ceil(4), header.height.div_ceil(4));
        assert_eq!(pnm_size(&written), want, "{stem}");
        let said = String::from_utf8_lossy(&run.stdout);
        assert!(said.contains(&format!("{}x{}", want.0, want.1)), "{said}");
        // The same samples as the library gives at that reduction.
        let options = DecodeOptions::default().with_resolution_reduction(2);
        let image = decode_with(&bytes, &options).unwrap();
        assert_eq!(written, pnm::encode(&image).unwrap(), "{stem}");
    }
}

#[test]
fn thumbnail_of_a_window_is_the_windows_reduced_size() {
    // The edges are odd, so the reduced size rounds each one up first:
    // ceil(111/2) - ceil(17/2) is 47, not ceil(94/2).
    let stem = PRECINCTS;
    let out = out_path("thumb_window.pnm");
    let _ = std::fs::remove_file(&out);
    let run = run_thumbnail(stem, &["1", "17", "9", "111", "87"], &out);
    assert!(run.status.success(), "{run:?}");
    let written = std::fs::read(&out).unwrap();
    assert_eq!(pnm_size(&written), (47, 39));
    let bytes = std::fs::read(fixture(stem)).unwrap();
    let region = Region::Window(Window::new(17, 9, 111, 87));
    let options = DecodeOptions::default()
        .with_resolution_reduction(1)
        .with_region(region);
    let image = decode_with(&bytes, &options).unwrap();
    assert_eq!(written, pnm::encode(&image).unwrap());
}

#[test]
fn thumbnail_refuses_bad_arguments_with_a_message() {
    let out = out_path("thumb_bad.pnm");
    let _ = std::fs::remove_file(&out);
    let name = PRECINCTS;
    // More levels than the file has: the error names the file.
    let run = run_thumbnail(name, &["9"], &out);
    assert!(!run.status.success());
    let message = String::from_utf8_lossy(&run.stderr);
    assert!(message.contains(name), "{message}");
    // Not numbers, a negative number, and a missing coordinate.
    for rest in [&["two"][..], &["-1"], &["1", "1", "2", "3"]] {
        let run = run_thumbnail(name, rest, &out);
        assert!(!run.status.success(), "{rest:?}");
        let message = String::from_utf8_lossy(&run.stderr);
        assert!(
            message.contains("usage") || message.contains("levels"),
            "{message}"
        );
    }
    let run = run_thumbnail(name, &["0", "0", "x", "4", "4"], &out);
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("x1"));
    assert!(!out.exists());
}

fn run_stream(stem: &str, chunk: &str) -> Output {
    let input = fixture(stem);
    run_example("stream", &[input.as_os_str(), OsStr::new(chunk)])
}

#[test]
fn stream_refines_before_the_last_chunk_and_ends_equal_to_decode() {
    let stem = "layers/lrcp.j2k";
    let total = std::fs::read(fixture(stem)).unwrap().len();
    let run = run_stream(stem, "100");
    assert!(run.status.success(), "{run:?}");
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(said.contains("4 quality layers"), "{said}");
    // The step lines start with the bytes fed so far.
    let steps: Vec<(usize, &str)> = said
        .lines()
        .filter_map(|l| l.split_once(" bytes: "))
        .filter_map(|(n, rest)| Some((n.parse().ok()?, rest)))
        .collect();
    assert_eq!(steps.last().unwrap().0, total);
    let early = steps.iter().filter(|(n, _)| *n < total);
    let refines = early.filter(|(_, r)| r.starts_with("refined")).count();
    assert!(refines >= 1, "{said}");
    assert!(
        steps.iter().any(|(_, r)| r.starts_with("waiting")),
        "{said}"
    );
    assert!(said.contains("finished: 64 x 64"), "{said}");
    assert!(said.contains("as decode on the whole file: true"), "{said}");
}

#[test]
fn stream_reports_no_header_until_it_has_one() {
    // A 10-byte chunk cannot hold the main header.
    let run = run_stream("layers/lrcp.j2k", "10");
    assert!(run.status.success(), "{run:?}");
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(said.contains("10 bytes: no header yet"), "{said}");
    assert_eq!(said.matches("header is in").count(), 1, "{said}");
    assert!(said.contains("as decode on the whole file: true"), "{said}");
}

#[test]
fn stream_refuses_bad_arguments_and_a_missing_file() {
    for chunk in ["0", "-5", "many"] {
        let run = run_stream("layers/lrcp.j2k", chunk);
        assert!(!run.status.success(), "{chunk}");
        let message = String::from_utf8_lossy(&run.stderr);
        assert!(message.contains("chunk-bytes"), "{chunk}: {message}");
    }
    let run = run_stream("layers/no_such_file.j2k", "100");
    assert!(!run.status.success());
    let message = String::from_utf8_lossy(&run.stderr);
    assert!(message.contains("no_such_file.j2k"), "{message}");
}
