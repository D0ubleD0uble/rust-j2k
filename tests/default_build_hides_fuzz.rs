//! A build with default features does not export `rust_j2k::fuzz`.
//!
//! This crate's tests run with the private `__fuzz` feature on, through the
//! self dev-dependency in `Cargo.toml`, so an in-crate `compile_fail` doctest
//! would see the hooks. This test builds a scratch crate that depends on the
//! library the way a downstream crate does, with default features, and checks
//! that it can use the public API but cannot name `rust_j2k::fuzz`.

use std::path::Path;
use std::process::Command;

/// `cargo check` of a scratch crate with default features whose `main.rs` is
/// `main`; returns whether it built and what cargo said.
fn check_downstream(name: &str, main: &str) -> (bool, String) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("scratch crate dir");
    let lib = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
         [dependencies]\nrust-j2k = {{ path = {lib:?} }}\n\n[workspace]\n"
    );
    std::fs::write(root.join("Cargo.toml"), manifest).expect("scratch manifest");
    std::fs::write(root.join("src/main.rs"), main).expect("scratch main");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(["check", "--offline", "--quiet", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", root.join("target"))
        .output()
        .expect("run cargo");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_downstream_crate_with_default_features_cannot_name_the_fuzz_hooks() {
    // The control: the same scratch crate builds against the public API, so
    // the failure below is the missing module and not the setup.
    let (ok, stderr) = check_downstream(
        "downstream_control",
        "fn main() { let _ = rust_j2k::decode(&[]); }\n",
    );
    assert!(ok, "the control crate failed to build:\n{stderr}");

    let (ok, stderr) = check_downstream(
        "downstream_fuzz",
        "fn main() { rust_j2k::fuzz::mq_stream(&[]); }\n",
    );
    assert!(!ok, "rust_j2k::fuzz is exported by default features");
    assert!(
        stderr.contains("could not find `fuzz`"),
        "unexpected failure:\n{stderr}"
    );
}
