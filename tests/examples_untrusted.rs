//! The `untrusted` example: a limit refusal, the opt-in retry past it, and
//! the partial decode of a cut-short file.
//!
//! Each test runs the example through `cargo run --example` (Cargo has no
//! path variable for examples), checks what it prints, and checks the same
//! outcome through the library, so the example and the API agree.
//!
//! The fixtures:
//!
//! - `psot0/single_psot0_no_eoc_trunc100.j2k` is cut short: its `EOC` and
//!   the last 100 bytes before it are gone (`tests/fixtures/psot0/README.md`).
//! - The first 971 bytes of `partial/tiles.j2k` end inside tile 1's
//!   tile-part, so tiles 2 and 3 are missing
//!   (`tests/fixtures/partial/README.md`). The test writes that prefix out.
//! - `tier1_work_bound/t1bomb_1024_64_88.j2k` asks for more code-block work
//!   than the default bound allows, and decodes under `WorkLimit::Raised(2)`
//!   (`tests/fixtures/tier1_work_bound/README.md`).
//! - `header/own_cod_lower_tile.j2k` holds a tile with more precincts than
//!   the default sample limit allows, and its refusal names a factor of 10
//!   (`tests/fixtures/README.md`).

#![cfg(not(target_family = "wasm"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rust_j2k::{DecodeOptions, Error, LimitKind, Raise, decode_with};

const CUT: &str = "psot0/single_psot0_no_eoc_trunc100.j2k";
const BOMB: &str = "tier1_work_bound/t1bomb_1024_64_88.j2k";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Run the example through `cargo run`, as `tests/examples_pnm.rs` runs
/// `decode`: `--profile test` reuses the library the tests built, so only
/// the example is compiled.
fn run_untrusted(args: &[&std::ffi::OsStr]) -> Output {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["run", "--quiet", "--offline", "--profile", "test"])
        .args(["--example", "untrusted", "--"])
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The report entries the example printed: the indented lines after
/// "complete at reduction".
fn entries(out: &str) -> Vec<&str> {
    out.lines()
        .skip_while(|line| !line.starts_with("complete at reduction"))
        .skip(1)
        .filter(|line| line.starts_with("  "))
        .collect()
}

/// A cut file: refused strict, then decoded with `strict` off and a report
/// that says what is missing.
fn assert_partial(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let strict = decode_with(&bytes, &DecodeOptions::default());
    assert!(matches!(strict, Err(Error::Codestream(_))), "{strict:?}");
    let lenient = DecodeOptions::default().with_strict(false);
    let image = decode_with(&bytes, &lenient).unwrap();
    assert!(!image.report.entries.is_empty());
    assert!(!image.report.is_complete());

    let run = run_untrusted(&[path.as_os_str()]);
    assert!(run.status.success(), "{run:?}");
    let out = stdout(&run);
    assert!(out.contains("refused as damaged or cut short"), "{out}");
    assert!(out.contains("decoding again with strict off"), "{out}");
    assert!(out.contains("complete: false"), "{out}");
    let printed = entries(&out);
    assert_eq!(printed.len(), image.report.entries.len(), "{out}");
    for (line, entry) in printed.iter().zip(&image.report.entries) {
        assert_eq!(line.trim_start(), entry.to_string());
    }
}

#[test]
fn a_cut_file_is_refused_strict_and_reported_with_strict_off() {
    assert_partial(&fixture(CUT));
}

#[test]
fn a_file_cut_inside_a_tile_part_reports_the_missing_tiles() {
    let whole = std::fs::read(fixture("partial/tiles.j2k")).unwrap();
    let cut = std::fs::read_to_string(fixture("partial/tiles.cut")).unwrap();
    let cut: usize = cut.trim().parse().unwrap();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tiles_cut.j2k");
    std::fs::write(&path, &whole[..cut]).unwrap();
    assert_partial(&path);
}

#[test]
fn a_limit_refusal_is_printed_and_not_retried_by_default() {
    let bytes = std::fs::read(fixture(BOMB)).unwrap();
    let defaults = DecodeOptions::default();
    let Err(Error::Limit(limit)) = decode_with(&bytes, &defaults) else {
        panic!("the code-block work bound refuses this file");
    };
    assert_eq!(limit.kind(), LimitKind::CodeBlockWork);
    assert_eq!(limit.retry_with(), Some(Raise::Raised(2)));

    let run = run_untrusted(&[fixture(BOMB).as_os_str()]);
    assert!(!run.status.success(), "{run:?}");
    let out = stdout(&run);
    assert!(
        out.contains(&format!("refused by a limit: {limit}")),
        "{out}"
    );
    assert!(out.contains("kind: CodeBlockWork"), "{out}");
    assert!(out.contains("retry with: Raised(2)"), "{out}");
    assert!(out.contains("refused at: tile 0"), "{out}");
    assert!(!out.contains("retrying"), "{out}");
    assert!(!out.contains("decoded"), "{out}");
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("--allow-raise"), "{err}");
    assert!(err.contains("t1bomb_1024_64_88.j2k"), "{err}");
}

#[test]
fn allow_raise_retries_once_with_raised_for_and_decodes() {
    let bytes = std::fs::read(fixture(BOMB)).unwrap();
    let defaults = DecodeOptions::default();
    let Err(Error::Limit(limit)) = decode_with(&bytes, &defaults) else {
        panic!("the code-block work bound refuses this file");
    };
    let raised = DecodeOptions::default().raised_for(&limit).unwrap();
    let image = decode_with(&bytes, &raised).unwrap();

    let flag = std::ffi::OsStr::new("--allow-raise");
    let run = run_untrusted(&[flag, fixture(BOMB).as_os_str()]);
    assert!(run.status.success(), "{run:?}");
    let out = stdout(&run);
    assert!(out.contains("kind: CodeBlockWork"), "{out}");
    assert!(out.contains("retrying once with the limit raised"), "{out}");
    let size = format!(": {}x{}, 1 channel", image.width, image.height);
    assert!(out.contains(&size), "{out}");
    assert!(out.contains("complete: true"), "{out}");
}

#[test]
fn allow_raise_stops_past_the_example_factor_cap() {
    let name = "header/own_cod_lower_tile.j2k";
    let bytes = std::fs::read(fixture(name)).unwrap();
    let defaults = DecodeOptions::default();
    let Err(Error::Limit(limit)) = decode_with(&bytes, &defaults) else {
        panic!("a tile cap refuses this file");
    };
    assert_eq!(limit.retry_with(), Some(Raise::Raised(10)));

    let flag = std::ffi::OsStr::new("--allow-raise");
    let run = run_untrusted(&[flag, fixture(name).as_os_str()]);
    assert!(!run.status.success(), "{run:?}");
    let out = stdout(&run);
    assert!(out.contains("retry with: Raised(10)"), "{out}");
    assert!(!out.contains("retrying"), "{out}");
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("a factor of 10, past"), "{err}");
}

#[test]
fn input_that_is_not_jpeg_2000_fails_with_its_name() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hello.j2k");
    std::fs::write(&path, b"hello").unwrap();
    let run = run_untrusted(&[path.as_os_str()]);
    assert!(!run.status.success(), "{run:?}");
    assert!(!stdout(&run).contains("decoded"));
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("hello.j2k"), "{err}");
}

#[test]
fn bad_arguments_print_the_usage() {
    let flag = std::ffi::OsStr::new("--allow-raise");
    for args in [&[][..], &[flag][..]] {
        let run = run_untrusted(args);
        assert!(!run.status.success(), "{run:?}");
        let err = String::from_utf8_lossy(&run.stderr);
        assert!(err.contains("usage: untrusted"), "{err}");
    }
}
