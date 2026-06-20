//! Stage 2 — Tier-2 packet decoding (ISO/IEC 15444-1 Annex B).
//!
//! Tile-part data is a sequence of *packets*, one per (resolution, layer,
//! component, precinct) in the header's progression order. Each packet header
//! says, via [tag trees](tagtree), which code-blocks are included, how many
//! bit-planes are all-zero, how many coding passes each contributes, and the
//! byte length of each contribution. This stage parses that structure and
//! hands Tier-1 the coded byte segments per code-block — it does **not** run
//! the arithmetic decoder.

pub mod bio;
pub mod tagtree;

use crate::Result;
use crate::codestream::Codestream;

/// The coded byte segments for every code-block in every subband, grouped so
/// Tier-1 can decode each block independently. (Concrete layout — indexed by
/// resolution / subband / block — to be defined as Tier-1's input firms up.)
#[derive(Debug, Default)]
pub struct CodedData {
    // TODO: per-subband code-block segments (data slice + pass count + zero
    // bit-planes + included-from-layer).
}

/// Parse all packets in the codestream's tile-parts into per-code-block coded
/// segments, following the COD progression order and precinct partition.
pub fn decode_packets(cs: &Codestream<'_>) -> Result<CodedData> {
    todo!("iterate packets in progression order; parse inclusion/zero-bitplane/length tag-trees")
}
