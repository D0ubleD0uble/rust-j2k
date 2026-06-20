//! Stage 3 — Tier-1 / EBCOT block coding (ISO/IEC 15444-1 Annex C-D).
//!
//! The core and the hardest part. Each code-block's coded bytes are an MQ
//! arithmetic-coded bit-plane stream. From the most significant non-zero plane
//! down, every plane is decoded by up to three passes — significance
//! propagation, magnitude refinement, cleanup — each forming contexts from the
//! 3×3 neighbourhood and reading binary decisions from the [`mq`] decoder.
//! The result is the quantized wavelet coefficients of one subband block.

pub mod mq;
pub mod passes;

use crate::Result;
use crate::codestream::MainHeader;
use crate::tier2::CodedData;

/// Quantized wavelet coefficients, organised by resolution level and subband
/// (LL at the top level, then HL/LH/HH per level), each a 2-D grid. (Concrete
/// container to be defined alongside the DWT stage that consumes it.)
#[derive(Debug, Default)]
pub struct SubbandCoeffs {
    // TODO: per-(level, subband) coefficient grids with their origins.
}

/// Decode every code-block's MQ/EBCOT stream into quantized subband
/// coefficients, assembling code-blocks back into their subbands.
pub fn decode_code_blocks(header: &MainHeader, coded: &CodedData) -> Result<SubbandCoeffs> {
    todo!("per code-block: run passes::decode_block, place coefficients into subbands")
}
