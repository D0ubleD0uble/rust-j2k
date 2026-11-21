//! `Error::Inconsistent` means "this is our bug, please file it", so no input
//! the corpus or the fuzzers feed should ever give it. [`checked`] passes a
//! result through and panics on that variant.
//!
//! The differential fuzz targets compare two paths by their `Debug` text, so
//! two paths that both returned `Inconsistent` would agree. Passing each side
//! through [`checked`] makes that a failure.
//!
//! This file has no dependency but `rust_j2k`, so the fuzz targets include it
//! with `#[path]`, as they do `synth.rs`.

#![allow(dead_code)]

use rust_j2k::Error;

/// Panic if `error` is [`Error::Inconsistent`].
#[track_caller]
pub fn assert_not_inconsistent(error: &Error) {
    assert!(
        !matches!(error, Error::Inconsistent(_)),
        "internal error (a bug in the decoder): {error:?}"
    );
}

/// `result`, unchanged, after asserting that its error (if any) is not
/// [`Error::Inconsistent`].
#[track_caller]
pub fn checked<T>(result: Result<T, Error>) -> Result<T, Error> {
    if let Err(error) = &result {
        assert_not_inconsistent(error);
    }
    result
}
