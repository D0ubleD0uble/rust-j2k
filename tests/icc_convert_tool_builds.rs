//! The `scripts/icc_convert` test tool still builds and is formatted.
//!
//! The tool sits in its own workspace (see its `Cargo.toml`), so
//! `cargo clippy --all-targets`, `cargo test` and `cargo fmt --all` at the
//! repository root do not see it. This test builds it with `cargo check` and
//! checks its formatting, so a change to the library that breaks it fails here.
//! The package leaves `scripts/` out, so the test skips there.

#![cfg(not(target_family = "wasm"))]

use std::path::Path;
use std::process::Command;

#[path = "support/skip.rs"]
mod skip;

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

#[test]
fn the_icc_convert_tool_builds_and_is_formatted() {
    if skip::excluded() {
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = root.join("scripts/icc_convert/Cargo.toml");
    // Its own target directory, so the nested build does not wait on the lock
    // of the build that is running this test.
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("icc_convert_target");

    let out = cargo()
        .args([
            "check",
            "--offline",
            "--locked",
            "--quiet",
            "--manifest-path",
        ])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .expect("run cargo check");
    assert!(
        out.status.success(),
        "scripts/icc_convert does not build (after a version bump, edit the rust-j2k \
version in scripts/icc_convert/Cargo.lock by hand, as RELEASING.md step 2 says):\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let have_rustfmt = cargo()
        .args(["fmt", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !have_rustfmt {
        eprintln!("skipped the format check: rustfmt is not installed");
        return;
    }
    let out = cargo()
        .args(["fmt", "--check", "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("run cargo fmt");
    assert!(
        out.status.success(),
        "scripts/icc_convert is not formatted:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
