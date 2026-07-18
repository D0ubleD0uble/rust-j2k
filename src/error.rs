//! Error type for the decoder. One flat enum; each failure maps to the variant
//! that names its class, so a caller can tell a malformed input from an
//! unsupported feature: [`Error::Codestream`] for structural damage,
//! [`Error::Marker`] for an illegal field encoding, [`Error::Unsupported`] for
//! a valid-but-out-of-subset feature, [`Error::Limit`] for an input past a
//! decoder resource guard, [`Error::InvalidOptions`] for bad caller options,
//! and [`Error::Inconsistent`] for a violated internal invariant. Structural
//! corruption anywhere in the pipeline — including packet headers and coded
//! data — reports as [`Error::Codestream`].

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
    /// A valid-but-not-decoded feature: a JP2 container, an HTJ2K codestream,
    /// a Part 2 extension. A newer version of the decoder may accept the same
    /// input.
    Unsupported(String),
    /// The input exceeded one of the decoder's resource guards — a hostile or
    /// absurd declaration (sample area, code-block count, precinct count,
    /// bit-plane depth, progression volumes) past a cap, not a missing
    /// feature.
    Limit(String),
    /// The caller passed invalid [`DecodeOptions`](crate::DecodeOptions) for
    /// this codestream — the input itself may be fine.
    InvalidOptions(String),
    /// A decoder invariant was violated (declared geometry and decoded state
    /// disagreed where they never should); please file a bug.
    Inconsistent(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Codestream(m) => write!(f, "codestream: {m}"),
            Error::Marker(m) => write!(f, "marker: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Limit(m) => write!(f, "limit: {m}"),
            Error::InvalidOptions(m) => write!(f, "invalid options: {m}"),
            Error::Inconsistent(m) => write!(f, "inconsistent: {m}"),
        }
    }
}

impl std::error::Error for Error {}
