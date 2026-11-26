//! Skipping a test whose fixture the published package leaves out.
//!
//! `Cargo.toml` excludes `tests/fixtures/conformance/`, `jp2_real/`, `bench/`
//! and `openjph/` from the package: the conformance files are licensed for
//! conformance testing only, and the others are large. A test that reads one
//! calls [`excluded`] first and returns when it is `true`.
//!
//! The test skips only when [`MARKER`] is missing and the crate directory is
//! not a git checkout. The marker is a file the package also leaves out, so a
//! checkout of the repository always has it. In the repository a missing
//! fixture therefore fails the test, which reads it with a hard `expect` after
//! the skip check. A missing marker in a git checkout (a `.git` directory, or
//! a `.git` file in a worktree, next to `Cargo.toml`) panics, so a moved or
//! deleted marker fails every checkout instead of skipping quietly. The
//! unpacked package, `cargo vendor` output and distro builds have no `.git`,
//! so they skip. A test that reads only excluded fixtures skips whole with
//! [`excluded`]; one that mixes them calls [`excluded_path`] per entry.
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
const MARKER: &str = "tests/fixtures/REPO_ONLY";

/// The directories under `tests/fixtures/` that `Cargo.toml` excludes from the
/// package. A test in `src/lib.rs` checks this list against `exclude`.
pub const EXCLUDED_DIRS: [&str; 4] = ["conformance/", "jp2_real/", "bench/", "openjph/"];

/// `true` when this is the published package, which has no excluded fixtures.
/// The caller then returns. Says on stderr which test was skipped.
///
/// Panics instead when the marker is missing in a git checkout.
///
/// The line goes straight to stderr rather than through `eprintln!`, so the
/// test harness shows it for a passing test without `--nocapture`.
#[allow(dead_code)]
#[track_caller]
pub fn excluded() -> bool {
    excluded_as(None)
}

/// [`excluded`] for something that is not a test (the bench), named in the
/// skip line as `what`.
#[allow(dead_code)]
#[track_caller]
pub fn excluded_named(what: &str) -> bool {
    excluded_as(Some(what))
}

/// `true` when `rel`, a path under `tests/fixtures/`, is left out of the
/// published package: the marker is missing (the same rule as [`excluded`],
/// panic included) and `rel` is under an excluded directory. A test that
/// mixes packaged and excluded fixtures calls it per entry and `continue`s
/// when it is `true`. Says on stderr which entry was skipped.
#[allow(dead_code)]
#[track_caller]
pub fn excluded_path(rel: &str) -> bool {
    let skip = is_excluded_path(rel);
    if skip {
        let at = Location::caller();
        let thread = std::thread::current();
        let _ = writeln!(
            std::io::stderr(),
            "skipped {rel} in {} ({}:{}): the fixture is left out of the published package",
            thread.name().unwrap_or("a test"),
            at.file(),
            at.line()
        );
    }
    skip
}

/// [`excluded_path`] without the stderr line, for building a list of entries
/// whose skip is reported where each is used.
#[allow(dead_code)]
#[track_caller]
pub fn is_excluded_path(rel: &str) -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let skip = decide(root.join(MARKER).exists(), root.join(".git").exists());
    skip && under_excluded_dir(rel)
}

/// Whether `rel` is under one of [`EXCLUDED_DIRS`].
#[allow(dead_code)]
pub fn under_excluded_dir(rel: &str) -> bool {
    EXCLUDED_DIRS.iter().any(|dir| rel.starts_with(dir))
}

#[track_caller]
fn excluded_as(what: Option<&str>) -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let skip = decide(root.join(MARKER).exists(), root.join(".git").exists());
    if skip {
        let at = Location::caller();
        let thread = std::thread::current();
        let name = what.or(thread.name()).unwrap_or("a test");
        let _ = writeln!(
            std::io::stderr(),
            "skipped {} ({}:{}): the fixtures it reads are left out of the published package",
            name,
            at.file(),
            at.line()
        );
    }
    skip
}

/// Whether to skip for a marker state and a git checkout, panicking where a
/// skip would hide a missing marker. `src/lib.rs` tests it.
#[track_caller]
pub fn decide(marker_present: bool, git_checkout: bool) -> bool {
    if marker_present {
        return false;
    }
    let at = Location::caller();
    assert!(
        !git_checkout,
        "{}:{}: the marker {MARKER} is missing in a git checkout; it must be in the repository",
        at.file(),
        at.line()
    );
    true
}
