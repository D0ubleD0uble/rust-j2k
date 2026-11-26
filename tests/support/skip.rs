//! Skipping a test whose fixture the published package leaves out.
//!
//! `Cargo.toml` excludes `tests/fixtures/conformance/`, `jp2_real/`, `bench/`
//! and `openjph/` from the package: the conformance files are licensed for
//! conformance testing only, and the others are large. A test that reads one
//! calls [`excluded`] first and returns when it is `true`.
//!
//! The test skips only when [`MARKER`] is missing. The marker is a file the
//! package also leaves out, so a checkout of the repository always has it. In
//! the repository a missing fixture therefore fails the test, which reads it
//! with a hard `expect` after the skip check; only the unpacked package skips.
//! A test that skips does so for all of its fixtures at once.
//!
//! This file is shared: `tests/support/mod.rs` and the `src/` unit tests
//! (through `#[path]` in `src/lib.rs`) both include it, and a few integration
//! tests include it alone.

use std::io::Write;
use std::panic::Location;
use std::path::Path;

/// An empty tracked file that `Cargo.toml` excludes from the package, so it
/// is in every checkout of the repository and never in the package. The skip
/// depends on it: keep it in the repository and in the `exclude` list.
const MARKER: &str = "tests/fixtures/.repo-only";

/// `true` when this is the published package, which has no excluded fixtures.
/// The caller then returns. Says on stderr which test was skipped.
///
/// Panics instead when the `CI` environment variable is set (GitHub Actions
/// sets it) and the marker is missing: CI runs the repository, so a missing or
/// moved marker must turn it red rather than skip every test quietly.
///
/// The line goes straight to stderr rather than through `eprintln!`, so the
/// test harness shows it for a passing test without `--nocapture`.
#[track_caller]
pub fn excluded() -> bool {
    decide(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(MARKER).exists(),
        std::env::var_os("CI").is_some(),
    )
}

/// The decision of [`excluded`] for a given marker state and `CI` setting;
/// `src/lib.rs` tests it.
#[track_caller]
pub fn decide(marker_present: bool, ci: bool) -> bool {
    if marker_present {
        return false;
    }
    let at = Location::caller();
    assert!(
        !ci,
        "{}:{}: the marker {MARKER} is missing while CI is set; it must be in the repository",
        at.file(),
        at.line()
    );
    let thread = std::thread::current();
    let _ = writeln!(
        std::io::stderr(),
        "skipped {} ({}:{}): the fixtures it reads are left out of the published package",
        thread.name().unwrap_or("a test"),
        at.file(),
        at.line()
    );
    true
}
