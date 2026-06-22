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

use crate::codestream::MainHeader;
use crate::codestream::markers::Transform;
use crate::tier1::mq::MqDecoder;
use crate::tier1::passes::{BlockState, MAX_BIT_PLANES, Orientation, decode_block};
use crate::tier2::{BandKind, CodeBlock, CodedData, Resolution, Subband};
use crate::{Error, Result};

/// One subband (or the coarsest LL): a row-major coefficient grid plus its
/// tile-component origin. `data.len() == width * height`, addressed as
/// `data[y * width + x]`.
///
/// `origin` is the `(x, y)` tile-component coordinate of the top-left sample.
/// The decoder currently handles a single tile at the canvas origin with no
/// precincts, so the origins are even and the inverse DWT assumes even parity
/// (index 0 of every interleaved row/column is a low-pass sample — see
/// [`crate::dwt`]). The field is carried so the odd-parity placement that
/// multi-tile and precinct support will need has somewhere to live.
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
///
/// The arithmetic is integer regardless of filter — Tier-1 recovers the
/// quantized indices — so the COD transform only fixes how those indices are
/// carried onward: the 5/3 reversible path keeps them as `i32` (the inverse is
/// exact), while the 9/7 irreversible path widens them to `f32` for [`dequant`]
/// to scale by the subband step. [`Bands`] mirrors [`CodedData`]: the coarsest
/// resolution's lone `NLLL` band becomes `ll`, and each finer resolution's
/// `HL/LH/HH` triple becomes one `levels` entry, coarsest first.
///
/// [`dequant`]: crate::quant::dequantize
pub fn decode_code_blocks(header: &MainHeader, coded: &CodedData<'_>) -> Result<SubbandCoeffs> {
    match header.cod.transform {
        Transform::Reversible53 => Ok(SubbandCoeffs::Reversible(assemble(header, coded, |q| q)?)),
        Transform::Irreversible97 => {
            Ok(SubbandCoeffs::Irreversible(assemble(header, coded, |q| {
                q as f32
            })?))
        }
    }
}

/// Decode every subband into a [`Bands`] pyramid, converting each quantized
/// index with `convert` (identity for the reversible path, `i32 as f32` for the
/// irreversible one).
///
/// Subbands run in QCD order — LL, then `HL, LH, HH` per resolution level
/// coarsest-first — which sets each band's magnitude bit-plane count `Mb`
/// (guard bits + quantization exponent − 1) that Tier-1 needs to place bits at
/// their true weights.
fn assemble<T, F>(header: &MainHeader, coded: &CodedData<'_>, convert: F) -> Result<Bands<T>>
where
    T: Copy + Default,
    F: Fn(i32) -> T + Copy,
{
    let style = header.cod.code_block_style;
    let mut resolutions = coded.resolutions.iter();
    let coarsest = resolutions
        .next()
        .ok_or_else(|| Error::Inconsistent("Tier-2 produced no resolutions to decode".into()))?;
    // Subband index 0 is the LL band; the detail bands count up from 1.
    let ll = decode_subband(
        subband_of(coarsest, BandKind::Ll)?,
        numbps(header, 0)?,
        style,
        convert,
    )?;

    let mut levels = Vec::with_capacity(coded.resolutions.len().saturating_sub(1));
    for (level, resolution) in resolutions.enumerate() {
        let base = 1 + level * 3;
        levels.push(DetailBands {
            hl: decode_subband(
                subband_of(resolution, BandKind::Hl)?,
                numbps(header, base)?,
                style,
                convert,
            )?,
            lh: decode_subband(
                subband_of(resolution, BandKind::Lh)?,
                numbps(header, base + 1)?,
                style,
                convert,
            )?,
            hh: decode_subband(
                subband_of(resolution, BandKind::Hh)?,
                numbps(header, base + 2)?,
                style,
                convert,
            )?,
        });
    }

    Ok(Bands { ll, levels })
}

/// The magnitude bit-plane count `Mb` for subband index `band` (ISO E.1):
/// `guard_bits + ε_b − 1`, where the exponent `ε_b` comes from the shared
/// [`Qcd::subband_step`] mapping (so it always matches the dequant step).
fn numbps(header: &MainHeader, band: usize) -> Result<u32> {
    let qcd = &header.qcd;
    let (exp, _) = qcd.subband_step(band).ok_or_else(|| {
        Error::Inconsistent(format!(
            "QCD carries {} step sizes but subband {band} needs one",
            qcd.steps.len()
        ))
    })?;
    // Mb = guard + ε − 1, floored at 0 (a band with no magnitude planes decodes
    // to all zeros).
    Ok((u32::from(qcd.guard_bits) + u32::from(exp)).saturating_sub(1))
}

/// The subband of orientation `kind` within one resolution. Tier-2 always emits
/// the full set for the resolution (one `Ll`, or `Hl/Lh/Hh`), so a miss is an
/// internal inconsistency rather than malformed input.
fn subband_of<'a, 'b>(res: &'a Resolution<'b>, kind: BandKind) -> Result<&'a Subband<'b>> {
    res.subbands
        .iter()
        .find(|s| s.kind == kind)
        .ok_or_else(|| Error::Inconsistent(format!("resolution is missing its {kind:?} subband")))
}

/// Decode one subband's code-blocks into a row-major coefficient [`Band`].
/// `numbps` is the band's magnitude bit-plane count `Mb`. Absent blocks (no
/// coding passes) are left at the band's zero fill.
fn decode_subband<T, F>(sb: &Subband<'_>, numbps: u32, style: u8, convert: F) -> Result<Band<T>>
where
    T: Copy + Default,
    F: Fn(i32) -> T,
{
    let orient = match sb.kind {
        BandKind::Ll => Orientation::Ll,
        BandKind::Hl => Orientation::Hl,
        BandKind::Lh => Orientation::Lh,
        BandKind::Hh => Orientation::Hh,
    };

    let mut data = vec![T::default(); sb.width * sb.height];
    for block in &sb.blocks {
        // An absent block contributes nothing; its samples stay zero.
        if block.num_passes == 0 {
            continue;
        }
        // The double-scale reconstruction shifts `1 << (Mb − zero_bit_planes)`;
        // reject high-dynamic-range subbands that would overflow `i32` rather
        // than panic, matching OpenJPEG's `bpno_plus_one >= 31` guard.
        let top = numbps.saturating_sub(block.zero_bit_planes);
        if top > MAX_BIT_PLANES {
            return Err(Error::Unsupported(format!(
                "code-block needs {top} bit-planes, over the {MAX_BIT_PLANES}-plane limit"
            )));
        }
        let mut state = BlockState::new(block.width as u32, block.height as u32);
        let mut mq = MqDecoder::new(block.segment);
        decode_block(
            &mut mq,
            &mut state,
            orient,
            numbps,
            block.num_passes,
            block.zero_bit_planes,
            style,
        );
        place_block(&mut data, sb.width, block, &state, &convert);
    }

    Ok(Band {
        origin: sb.origin,
        width: sb.width,
        height: sb.height,
        data,
    })
}

/// Copy a decoded block's coefficients into its place in the subband grid,
/// converting each with `convert`. `band_width` is the destination stride.
fn place_block<T, F>(
    data: &mut [T],
    band_width: usize,
    block: &CodeBlock<'_>,
    state: &BlockState,
    convert: &F,
) where
    F: Fn(i32) -> T,
{
    for row in 0..block.height {
        let dst = (block.y + row) * band_width + block.x;
        let src = row * block.width;
        for col in 0..block.width {
            data[dst + col] = convert(state.coeffs[src + col]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tier2::{BandKind, CodeBlock, Subband};

    /// A single-cell LL subband carrying one code-block with the given pass and
    /// zero-bitplane counts.
    fn one_block_subband(num_passes: u32, zero_bit_planes: u32) -> Subband<'static> {
        Subband {
            kind: BandKind::Ll,
            origin: (0, 0),
            width: 1,
            height: 1,
            block_cols: 1,
            block_rows: 1,
            blocks: vec![CodeBlock {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                num_passes,
                zero_bit_planes,
                segment: &[0x80],
            }],
        }
    }

    /// A subband whose coded bit-plane count `Mb − zero_bit_planes` exceeds the
    /// `i32` double-scale limit is rejected as unsupported, not decoded into an
    /// overflow.
    #[test]
    fn excessive_bit_planes_rejected() {
        let sb = one_block_subband(1, 0);
        let err =
            decode_subband::<i32, _>(&sb, MAX_BIT_PLANES + 1, 0, |q| q).expect_err("must reject");
        assert!(matches!(err, Error::Unsupported(_)), "got {err:?}");
    }

    /// The largest in-range bit-plane count decodes without error or overflow.
    #[test]
    fn max_bit_planes_accepted() {
        let sb = one_block_subband(1, 0);
        let band = decode_subband::<i32, _>(&sb, MAX_BIT_PLANES, 0, |q| q).expect("in range");
        assert_eq!(band.data.len(), 1);
    }
}
