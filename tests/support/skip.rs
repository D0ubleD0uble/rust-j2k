//! Skipping a test whose fixture the published package leaves out.
//!
//! `Cargo.toml` excludes `tests/fixtures/conformance/`, `jp2_real/`, `bench/`
//! and `openjph/` from the package: the conformance files are licensed for
//! conformance testing only, and the others are large. A test that reads one
//! calls [`excluded`] first and returns when it is `true`.
//!
//! The test skips only when [`MARKER`] is missing. The marker is a file the
//! package also leaves out, so a checkout of the repository always has it. In
//! the repository a missing fixture therefore still fails the test; only the
//! unpacked package skips.
//!
//! This file is shared: `tests/support/mod.rs` and the `src/` unit tests
//! (through `#[path]` in `src/lib.rs`) both include it, and a few integration
//! tests include it alone.

use std::io::Write;
use std::panic::Location;
use std::path::Path;

/// A file that is in every checkout of the repository and not in the package.
/// `deny.toml` is tracked, is not in `.gitignore`, and `Cargo.toml` excludes it.
const MARKER: &str = "deny.toml";

/// `true` when this is the published package, which has no excluded fixtures.
/// The caller then returns. Says on stderr which test was skipped.
///
/// The line goes straight to stderr rather than through `eprintln!`, so the
/// test harness shows it for a passing test without `--nocapture`.
#[track_caller]
pub fn excluded() -> bool {
    if Path::new(env!("CARGO_MANIFEST_DIR")).join(MARKER).exists() {
        return false;
    }
    let at = Location::caller();
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
