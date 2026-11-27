//! The `untrusted` example: a limit refusal, the opt-in retry past it, and
//! the partial decode of a cut-short file.
//!
//! The library tests check, through the public API, what each fixture
//! gives, so the example's claims hold on every target. The process tests,
//! not on wasm, run the example through `cargo run --example` (Cargo has no
//! path variable for examples) and check what it prints and its exit
//! status: 0 for a complete image, 2 for one with data missing, 1 for an
//! error.
//!
//! The fixtures:
//!
//! - `psot0/single_psot0_no_eoc_trunc100.j2k` is cut short: its `EOC` and
//!   the last 100 bytes before it are gone (`tests/fixtures/psot0/README.md`).
//! - The first 971 bytes of `partial/tiles.j2k` end inside tile 1's
//!   tile-part, so tiles 2 and 3 are missing
//!   (`tests/fixtures/partial/README.md`).
//! - `tier1_work_bound/t1bomb_1024_64_88.j2k` asks for more code-block work
//!   than the default bound allows, and decodes under `WorkLimit::Raised(2)`
//!   (`tests/fixtures/tier1_work_bound/README.md`).
//! - `work_bound/sweep_pcrl_1024_64.j2k` is refused by the packet work bound
//!   at the default and again at `Raised(2)`, which names `Raised(4)`.
//! - `header/own_cod_lower_tile.j2k` holds a tile with more precincts than
//!   the default sample limit allows, and its refusal names a factor of 10
//!   (`tests/fixtures/README.md`).

#[path = "../examples/common/capped.rs"]
mod capped;

use std::path::{Path, PathBuf};

use rust_j2k::{DecodeOptions, Error, LimitDetail, LimitKind, Raise};
use rust_j2k::{Image, decode_with};

const CUT: &str = "psot0/single_psot0_no_eoc_trunc100.j2k";
const BOMB: &str = "tier1_work_bound/t1bomb_1024_64_88.j2k";
const SWEEP: &str = "work_bound/sweep_pcrl_1024_64.j2k";
const PAST_CAP: &str = "header/own_cod_lower_tile.j2k";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap()
}

/// The first 971 bytes of `partial/tiles.j2k`, as `tiles.cut` gives.
fn tiles_cut() -> Vec<u8> {
    let mut whole = read("partial/tiles.j2k");
    let cut = std::fs::read_to_string(fixture("partial/tiles.cut")).unwrap();
    whole.truncate(cut.trim().parse().unwrap());
    whole
}

fn limit_at(bytes: &[u8], options: &DecodeOptions) -> LimitDetail {
    match decode_with(bytes, options) {
        Err(Error::Limit(limit)) => limit,
        other => panic!("expected a limit refusal, got {other:?}"),
    }
}

/// A cut file: refused strict, then decoded with `strict` off and a report
/// that says what is missing.
fn partial_image(bytes: &[u8]) -> Image {
    let strict = decode_with(bytes, &DecodeOptions::default());
    assert!(matches!(strict, Err(Error::Codestream(_))), "{strict:?}");
    let lenient = DecodeOptions::default().with_strict(false);
    let image = decode_with(bytes, &lenient).unwrap();
    assert!(!image.report.entries.is_empty());
    assert!(!image.report.is_complete());
    image
}

#[test]
fn a_cut_file_is_refused_strict_and_decodes_with_strict_off() {
    partial_image(&read(CUT));
    partial_image(&tiles_cut());
}

#[test]
fn the_work_bound_fixture_decodes_once_raised() {
    let bytes = read(BOMB);
    let limit = limit_at(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::CodeBlockWork);
    assert_eq!(limit.retry_with(), Some(Raise::Raised(2)));
    let raised = DecodeOptions::default().raised_for(&limit).unwrap();
    let image = decode_with(&bytes, &raised).unwrap();
    assert!(image.report.is_complete());
}

#[test]
fn the_sweep_fixture_is_refused_again_after_one_raise() {
    let bytes = read(SWEEP);
    let limit = limit_at(&bytes, &DecodeOptions::default());
    assert_eq!(limit.kind(), LimitKind::PacketWork);
    let raised = DecodeOptions::default().raised_for(&limit).unwrap();
    let again = limit_at(&bytes, &raised);
    assert_eq!(again.retry_with(), Some(Raise::Raised(4)));
}

#[test]
fn the_tile_cap_fixture_names_a_factor_past_the_example_cap() {
    let limit = limit_at(&read(PAST_CAP), &DecodeOptions::default());
    assert_eq!(limit.retry_with(), Some(Raise::Raised(10)));
}

#[test]
fn the_entry_list_is_capped_and_counts_the_rest() {
    let at_cap: Vec<usize> = (0..capped::SHOWN).collect();
    let lines = capped::capped(&at_cap);
    assert_eq!(lines.len(), capped::SHOWN);
    assert_eq!(lines.last().unwrap(), &(capped::SHOWN - 1).to_string());

    let past: Vec<usize> = (0..capped::SHOWN + 4).collect();
    let lines = capped::capped(&past);
    assert_eq!(lines.len(), capped::SHOWN + 1);
    assert_eq!(lines[capped::SHOWN - 1], (capped::SHOWN - 1).to_string());
    assert_eq!(lines[capped::SHOWN], "and 4 more");
}

#[cfg(not(target_family = "wasm"))]
#[path = "support/run_example.rs"]
mod run_example;

#[cfg(not(target_family = "wasm"))]
mod process {
    use super::*;
    use std::ffi::OsStr;
    use std::process::Output;

    fn run_untrusted(args: &[&OsStr]) -> Output {
        super::run_example::run_example("untrusted", args)
    }

    fn stdout(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn stderr(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
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

    /// The example on a cut file: it says it decodes again with strict off,
    /// prints every report entry, and exits with 2.
    fn assert_partial(path: &Path, image: &Image) {
        let run = run_untrusted(&[path.as_os_str()]);
        assert_eq!(run.status.code(), Some(2), "{run:?}");
        let out = stdout(&run);
        assert!(out.contains("refused as damaged or cut short"), "{out}");
        assert!(out.contains("decoding again with strict off"), "{out}");
        assert!(out.contains("complete: false"), "{out}");
        let printed = entries(&out);
        let want = capped::capped(&image.report.entries);
        assert_eq!(printed.len(), want.len(), "{out}");
        for (line, entry) in printed.iter().zip(&want) {
            assert_eq!(line.trim_start(), entry);
        }
    }

    #[test]
    fn a_cut_file_is_reported_with_strict_off_and_exits_2() {
        let image = partial_image(&read(CUT));
        assert_partial(&fixture(CUT), &image);
    }

    /// The example reads a path, so the prefix is written to a file: there
    /// is no portable path for stdin.
    #[test]
    fn a_file_cut_inside_a_tile_part_reports_the_missing_tiles() {
        let bytes = tiles_cut();
        let image = partial_image(&bytes);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let path = dir.join("tiles_cut.j2k");
        std::fs::write(&path, &bytes).unwrap();
        assert_partial(&path, &image);
    }

    #[test]
    fn a_whole_file_exits_0() {
        let path = fixture("jpeg2000_regular_latlon.j2k");
        let run = run_untrusted(&[path.as_os_str()]);
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        let out = stdout(&run);
        assert!(out.contains("complete: true"), "{out}");
        assert!(!out.contains("strict off"), "{out}");
    }

    #[test]
    fn a_limit_refusal_is_printed_and_not_retried_by_default() {
        let limit = limit_at(&read(BOMB), &DecodeOptions::default());
        let run = run_untrusted(&[fixture(BOMB).as_os_str()]);
        assert_eq!(run.status.code(), Some(1), "{run:?}");
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
        let err = stderr(&run);
        assert!(err.contains("--allow-raise"), "{err}");
        assert!(err.contains("t1bomb_1024_64_88.j2k"), "{err}");
    }

    #[test]
    fn allow_raise_retries_once_with_raised_for_and_decodes() {
        let flag = OsStr::new("--allow-raise");
        let run = run_untrusted(&[flag, fixture(BOMB).as_os_str()]);
        assert_eq!(run.status.code(), Some(0), "{run:?}");
        let out = stdout(&run);
        assert!(out.contains("kind: CodeBlockWork"), "{out}");
        assert_eq!(out.matches("retrying once").count(), 1, "{out}");
        assert!(out.contains(": 1024x1024, 1 channel"), "{out}");
        assert!(out.contains("complete: true"), "{out}");
    }

    #[test]
    fn allow_raise_raises_only_once() {
        let flag = OsStr::new("--allow-raise");
        let run = run_untrusted(&[flag, fixture(SWEEP).as_os_str()]);
        assert_eq!(run.status.code(), Some(1), "{run:?}");
        let out = stdout(&run);
        assert_eq!(out.matches("retrying").count(), 1, "{out}");
        assert_eq!(out.matches("refused by a limit:").count(), 2, "{out}");
        assert!(!out.contains("decoded"), "{out}");
        let err = stderr(&run);
        assert!(err.contains("refused after one raise"), "{err}");
    }

    #[test]
    fn allow_raise_stops_past_the_example_factor_cap() {
        let flag = OsStr::new("--allow-raise");
        let run = run_untrusted(&[flag, fixture(PAST_CAP).as_os_str()]);
        assert_eq!(run.status.code(), Some(1), "{run:?}");
        let out = stdout(&run);
        assert!(out.contains("retry with: Raised(10)"), "{out}");
        assert!(!out.contains("retrying"), "{out}");
        let err = stderr(&run);
        assert!(err.contains("a factor of 10, past"), "{err}");
    }

    #[test]
    fn input_that_is_not_jpeg_2000_fails_with_its_name() {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let path = dir.join("hello.j2k");
        std::fs::write(&path, b"hello").unwrap();
        let run = run_untrusted(&[path.as_os_str()]);
        assert_eq!(run.status.code(), Some(1), "{run:?}");
        assert!(!stdout(&run).contains("decoded"));
        let err = stderr(&run);
        assert!(err.contains("hello.j2k"), "{err}");
    }

    /// The name is printed with `{:?}`, so a control character in it comes
    /// out escaped.
    #[cfg(unix)]
    #[test]
    fn a_control_character_in_the_name_is_escaped() {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let path = dir.join("bell\u{7}.j2k");
        std::fs::write(&path, b"hello").unwrap();
        let run = run_untrusted(&[path.as_os_str()]);
        assert_eq!(run.status.code(), Some(1), "{run:?}");
        let err = stderr(&run);
        assert!(err.contains("bell\\u{7}.j2k"), "{err}");
        assert!(!err.contains('\u{7}'), "{err}");
    }

    #[test]
    fn bad_arguments_print_the_usage() {
        let flag = OsStr::new("--allow-raise");
        for args in [&[][..], &[flag][..]] {
            let run = run_untrusted(args);
            assert_eq!(run.status.code(), Some(1), "{run:?}");
            let err = stderr(&run);
            assert!(err.contains("usage: untrusted"), "{err}");
        }
    }
}
