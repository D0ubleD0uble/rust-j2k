//! Error type for the decoder. One flat enum; each failure maps to the variant
//! that names its class, so a caller can tell a malformed input from an
//! unsupported feature. Structural corruption anywhere in the pipeline —
//! including packet headers and coded data — reports as [`Error::Codestream`].

use core::fmt;

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Error>;

/// A decode failure, tagged by the class of fault.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Not a JPEG 2000 codestream, or its structure is broken: a truncated or
    /// missing segment, lost sync, or corrupt packet / coded data.
    Codestream(String),
    /// A marker segment carried an illegal field encoding (a reserved value,
    /// a bad length, an out-of-range field).
    Marker(String),
    /// A valid-but-not-decoded feature (a JP2 container, a multi-tile grid,
    /// nonzero canvas offsets, an irreversible color transform).
    Unsupported(String),
    /// Declared geometry and decoded sample counts disagreed.
    Inconsistent(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Codestream(m) => write!(f, "codestream: {m}"),
            Error::Marker(m) => write!(f, "marker: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Inconsistent(m) => write!(f, "inconsistent: {m}"),
        }
    }
}

impl std::error::Error for Error {}
