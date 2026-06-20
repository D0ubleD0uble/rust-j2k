//! Error type for the decoder. One flat enum; each stage maps its failures to
//! the variant that names the layer, so a caller can tell a malformed header
//! from an unsupported feature from a Tier-1 decode fault.

use core::fmt;

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Error>;

/// A decode failure, tagged by the pipeline stage that raised it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a JPEG 2000 codestream, or a marker segment was truncated / malformed.
    Codestream(String),
    /// A required marker (SIZ / COD / QCD …) was missing or carried bad fields.
    Marker(String),
    /// A valid-but-out-of-scope feature for the GRIB2 subset (JP2 boxes,
    /// multiple components, a color transform, an unimplemented progression).
    Unsupported(String),
    /// Tier-2 packet or tag-tree parsing failed.
    Packet(String),
    /// Tier-1 (MQ arithmetic / EBCOT bit-plane) decode failed.
    Tier1(String),
    /// Declared geometry and decoded sample counts disagreed.
    Inconsistent(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Codestream(m) => write!(f, "codestream: {m}"),
            Error::Marker(m) => write!(f, "marker: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Packet(m) => write!(f, "packet: {m}"),
            Error::Tier1(m) => write!(f, "tier-1: {m}"),
            Error::Inconsistent(m) => write!(f, "inconsistent: {m}"),
        }
    }
}

impl std::error::Error for Error {}
