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

/// One subband (or the coarsest LL): a row-major coefficient grid plus its
/// tile-component origin. `data.len() == width * height`, addressed as
/// `data[y * width + x]`.
///
/// `origin` is the `(x, y)` tile-component coordinate of the top-left sample.
/// Phase 1 decodes a single tile at the canvas origin with no precincts, so the
/// origins are even and the inverse DWT assumes even parity (index 0 of every
/// interleaved row/column is a low-pass sample — see [`crate::dwt`]). The field
/// is carried so the odd-parity placement Phase 2 needs has somewhere to live.
#[derive(Debug, Clone, PartialEq)]
pub struct Band<T> {
    pub origin: (u32, u32),
    pub width: usize,
    pub height: usize,
    pub data: Vec<T>,
}

/// The three detail subbands added at one resolution level (ISO xob/yob from
/// Table F.1): `hl` is high-pass horizontally / low-pass vertically, `lh` the
/// reverse, `hh` high-pass in both.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailBands<T> {
    pub hl: Band<T>,
    pub lh: Band<T>,
    pub hh: Band<T>,
}

/// All subbands of one tile-component: the coarsest `ll` plus the detail bands
/// added at each resolution level, **coarsest first**. `levels.len()` equals the
/// COD decomposition-level count; an empty `levels` means no transform was
/// applied and `ll` is the image itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Bands<T> {
    pub ll: Band<T>,
    pub levels: Vec<DetailBands<T>>,
}

/// Quantized wavelet coefficients flowing Tier-1 → dequant → inverse DWT. The
/// reversible (5/3) path stays integer so it can reconstruct bit-exactly; the
/// irreversible (9/7) path is real-valued once dequantized, so it carries
/// `f32`. The arm matches the COD transform.
#[derive(Debug, Clone, PartialEq)]
pub enum SubbandCoeffs {
    /// 5/3 reversible: exact integer coefficients.
    Reversible(Bands<i32>),
    /// 9/7 irreversible: real-valued coefficients.
    Irreversible(Bands<f32>),
}

/// Decode every code-block's MQ/EBCOT stream into quantized subband
/// coefficients, assembling code-blocks back into their subbands.
pub fn decode_code_blocks(header: &MainHeader, coded: &CodedData) -> Result<SubbandCoeffs> {
    todo!("per code-block: run passes::decode_block, place coefficients into subbands")
}
