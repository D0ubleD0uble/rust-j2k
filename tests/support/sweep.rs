//! How much of a corpus a slow sweep test runs.
//!
//! A test that decodes every committed file takes minutes in a debug build.
//! The full sweep runs in a release build, and when `CI` or
//! `RUST_J2K_FULL_SWEEP` is set, so CI still runs every file. A local debug run
//! takes a deterministic sample: every `stride`-th item, plus the ones the
//! test names as required.

#![allow(dead_code)]

/// Whether to run the whole corpus.
pub fn full() -> bool {
    !cfg!(debug_assertions)
        || std::env::var_os("CI").is_some()
        || std::env::var_os("RUST_J2K_FULL_SWEEP").is_some()
}

/// The step to take through the corpus: 1 for the full sweep, else `sampled`.
pub fn stride(sampled: usize) -> usize {
    if full() { 1 } else { sampled }
}
