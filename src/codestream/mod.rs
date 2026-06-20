//! Stage 1 — codestream parsing (ISO/IEC 15444-1 Annex A).
//!
//! Walks the marker segments of a raw J2K codestream: the main header
//! (SIZ / COD / QCD, plus optional COC / QCC / RGN / POC / COM), then the
//! tile-parts (SOT … SOD … data). Produces a [`MainHeader`] of decode
//! parameters and the byte ranges of each tile's packet data — everything the
//! later stages need, with no interpretation of the entropy-coded bytes yet.

pub mod markers;

use crate::Result;
use markers::{Cod, Qcd, Siz};

/// Parsed main-header decode parameters. COC/QCC/RGN component overrides will
/// live here too once needed; for the single-component subset the defaults
/// usually suffice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHeader {
    pub siz: Siz,
    pub cod: Cod,
    pub qcd: Qcd,
}

/// One tile-part: its tile index and the slice of packet data between SOD and
/// the next marker. Multiple tile-parts can carry one tile; the GRIB2 common
/// case is a single tile in a single part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilePart<'a> {
    pub tile_index: u16,
    pub data: &'a [u8],
}

/// A parsed codestream: main header plus the tile-part data ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codestream<'a> {
    pub header: MainHeader,
    pub tile_parts: Vec<TilePart<'a>>,
}

/// Parse a raw codestream (must start with SOC, end with EOC).
///
/// Rejects the JP2 box wrapper (callers pass the bare codestream) and anything
/// outside the single-component subset with [`Error::Unsupported`].
///
/// [`Error::Unsupported`]: crate::Error::Unsupported
pub fn parse(bytes: &[u8]) -> Result<Codestream<'_>> {
    todo!("verify SOC, parse main-header markers into MainHeader, collect tile-parts")
}
