//! The examples that print end quietly when stdout closes early.
//!
//! `example | head -n 1` closes the pipe after one line. Rust ignores
//! SIGPIPE, so the next `println!` fails with `BrokenPipe` and panics. Each
//! example is run with its stdout closed at once. It must not panic or print
//! an error, and its exit status must be the one an open pipe gives.

#![cfg(not(target_family = "wasm"))]

#[path = "support/run_example.rs"]
mod run_example;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Run with stdout closed and check that stderr is empty, which a panic or
/// an error message would fill. Returns the exit status code.
fn closed(example: &str, args: &[&OsStr]) -> Option<i32> {
    let run = run_example::run_example_closed_stdout(example, args);
    let message = String::from_utf8_lossy(&run.stderr);
    assert!(message.is_empty(), "{example}: {message}");
    run.status.code()
}

const WHOLE: &str = "multilayer_lossless.j2k";
const CUT: &str = "psot0/single_psot0_no_eoc_trunc100.j2k";
const BOMB: &str = "tier1_work_bound/t1bomb_1024_64_88.j2k";

#[test]
fn decode_writes_its_file_and_exits_0() {
    let (input, out) = (fixture(WHOLE), out_path("closed_decode.pgm"));
    let _ = std::fs::remove_file(&out);
    let args = [input.as_os_str(), out.as_os_str()];
    assert_eq!(closed("decode", &args), Some(0));
    assert!(out.exists());
}

#[test]
fn thumbnail_writes_its_file_and_exits_0() {
    let (input, out) = (fixture(WHOLE), out_path("closed_thumb.pgm"));
    let _ = std::fs::remove_file(&out);
    let args = [input.as_os_str(), OsStr::new("1"), out.as_os_str()];
    assert_eq!(closed("thumbnail", &args), Some(0));
    assert!(out.exists());
}

#[test]
fn info_exits_0() {
    let input = fixture("jp2_metadata_boxes.jp2");
    assert_eq!(closed("info", &[input.as_os_str()]), Some(0));
}

#[test]
fn stream_exits_0() {
    let input = fixture("layers/lrcp.j2k");
    let args = [input.as_os_str(), OsStr::new("100")];
    assert_eq!(closed("stream", &args), Some(0));
}

#[test]
fn grib2_exits_0() {
    let input = fixture("jpeg2000_regular_latlon.j2k");
    let rest = ["270.466796875", "-10", "0"];
    let mut args = vec![input.as_os_str()];
    args.extend(rest.iter().map(OsStr::new));
    assert_eq!(closed("grib2", &args), Some(0));
}

/// A closed pipe does not change what `untrusted` reports: 0 for a complete
/// image and 2 for a partial one.
#[test]
fn untrusted_keeps_its_exit_codes() {
    let whole = fixture(WHOLE);
    assert_eq!(closed("untrusted", &[whole.as_os_str()]), Some(0));
    let cut = fixture(CUT);
    assert_eq!(closed("untrusted", &[cut.as_os_str()]), Some(2));
}

/// A refusal still ends with 1 and its message, and no panic. Its message
/// goes to stderr, so `closed` can't be used.
#[test]
fn untrusted_still_fails_with_1_on_a_refusal() {
    let bomb = fixture(BOMB);
    let args = [bomb.as_os_str()];
    let run = run_example::run_example_closed_stdout("untrusted", &args);
    assert_eq!(run.status.code(), Some(1));
    let message = String::from_utf8_lossy(&run.stderr);
    assert!(message.contains("--allow-raise"), "{message}");
    assert!(!message.contains("panicked"), "{message}");
}

/// With stderr closed too, an error message must not panic either: the
/// status stays 1, not 101.
#[test]
fn an_error_with_both_outputs_closed_still_exits_1() {
    let missing = fixture("no_such_file.j2k");
    let args = [missing.as_os_str()];
    let run = run_example::run_example_closed_output("info", &args);
    assert_eq!(run.status.code(), Some(1));
    let bomb = fixture(BOMB);
    let args = [bomb.as_os_str()];
    let run = run_example::run_example_closed_output("untrusted", &args);
    assert_eq!(run.status.code(), Some(1));
}
