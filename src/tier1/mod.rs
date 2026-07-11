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
use crate::tier1::passes::{
    BlockParams, BlockState, MAX_BIT_PLANES, Orientation, decode_block, top_coded_plane,
};
use crate::tier2::{BandKind, CodeBlock, CodedData, ComponentCoded, Resolution, Subband};
use crate::{Error, Result};

/// One subband (or the coarsest LL): a row-major coefficient grid plus its
/// tile-component origin. `data.len() == width * height`, addressed as
/// `data[y * width + x]`.
///
/// `origin` is the `(x, y)` tile-component coordinate of the top-left sample —
/// the band's `tbx0`/`tby0` (ISO Eq. B-15), not an offset within the tile. The
/// inverse DWT reads the interleave parity straight off it (see [`crate::dwt`]),
/// so it is load-bearing, not bookkeeping: a tile away from the canvas origin,
/// or a sub-sampled component, puts a band at an odd origin, and a band at an
/// odd origin starts on a high-pass sample.
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
/// to scale by the subband step. [`Bands`] mirrors one component of
/// [`CodedData`]: the coarsest resolution's lone `NLLL` band becomes `ll`, and
/// each finer resolution's `HL/LH/HH` triple becomes one `levels` entry,
/// coarsest first.
///
/// Returns one [`SubbandCoeffs`] per component, in SIZ order. Components are
/// independent here: no inter-component transform is applied.
///
/// `reduction` is the resolution reduction: that many of each component's
/// finest resolutions are dropped from the pyramid, their code-blocks skipped
/// rather than decoded. The caller has already checked it leaves every
/// component at least its coarsest resolution.
///
/// [`dequant`]: crate::quant::dequantize
pub fn decode_code_blocks(
    header: &MainHeader,
    coded: &CodedData<'_>,
    reduction: u8,
) -> Result<Vec<SubbandCoeffs>> {
    coded
        .components
        .iter()
        .enumerate()
        .map(|(index, component)| {
            // The wavelet is per component: a COC can put one component on 5/3
            // and another on 9/7.
            let transform = header
                .components
                .get(index)
                .ok_or_else(|| {
                    Error::Inconsistent(format!("no coding parameters for component {index}"))
                })?
                .coding
                .transform;
            match transform {
                // Drop the double-scale half bit. On the reversible arm this is
                // OpenJPEG's integer `datap[i] /= 2`, exact by construction.
                Transform::Reversible53 => Ok(SubbandCoeffs::Reversible(assemble(
                    header,
                    index,
                    component,
                    reduction,
                    |q| q / 2,
                )?)),
                // On the irreversible arm the half bit must survive into the
                // float: OpenJPEG multiplies by `0.5f * band->stepsize`, so the
                // halving happens in float, not as an integer truncation. `q`
                // can be odd, and dropping its low bit here loses up to half a
                // quantization step on every coefficient.
                Transform::Irreversible97 => Ok(SubbandCoeffs::Irreversible(assemble(
                    header,
                    index,
                    component,
                    reduction,
                    |q| q as f32,
                )?)),
            }
        })
        .collect()
}

/// Decode every subband into a [`Bands`] pyramid, converting each quantized
/// index with `convert` (identity for the reversible path, `i32 as f32` for the
/// irreversible one).
///
/// Subbands run in QCD order — LL, then `HL, LH, HH` per resolution level
/// coarsest-first — which sets each band's magnitude bit-plane count `Mb`
/// (guard bits + quantization exponent − 1) that Tier-1 needs to place bits at
/// their true weights.
fn assemble<T, F>(
    header: &MainHeader,
    comp: usize,
    coded: &ComponentCoded<'_>,
    reduction: u8,
    convert: F,
) -> Result<Bands<T>>
where
    T: Copy + Default,
    F: Fn(i32) -> T + Copy,
{
    let params = BlockParams {
        style: header.components[comp].coding.code_block_style,
        roi_shift: header.components[comp].roi_shift,
    };
    // A resolution reduction drops that many of the finest levels: their
    // packets were parsed (the codestream's framing demands it) but their
    // code-blocks never reach the bit-plane decoder. The caller has checked
    // `reduction` against this component's decomposition count, so `keep`
    // cannot underflow past the coarsest resolution.
    let keep = coded
        .resolutions
        .len()
        .saturating_sub(1 + usize::from(reduction));
    let mut resolutions = coded.resolutions.iter();
    let coarsest = resolutions
        .next()
        .ok_or_else(|| Error::Inconsistent("Tier-2 produced no resolutions to decode".into()))?;
    // Subband index 0 is the LL band; the detail bands count up from 1.
    let ll = decode_subband(
        subband_of(coarsest, BandKind::Ll)?,
        numbps(header, comp, 0)?,
        params,
        convert,
    )?;

    let mut levels = Vec::with_capacity(keep);
    for (level, resolution) in resolutions.take(keep).enumerate() {
        let base = 1 + level * 3;
        levels.push(DetailBands {
            hl: decode_subband(
                subband_of(resolution, BandKind::Hl)?,
                numbps(header, comp, base)?,
                params,
                convert,
            )?,
            lh: decode_subband(
                subband_of(resolution, BandKind::Lh)?,
                numbps(header, comp, base + 1)?,
                params,
                convert,
            )?,
            hh: decode_subband(
                subband_of(resolution, BandKind::Hh)?,
                numbps(header, comp, base + 2)?,
                params,
                convert,
            )?,
        });
    }

    Ok(Bands { ll, levels })
}

/// The magnitude bit-plane count `Mb` for subband index `band` (ISO E-2):
/// `guard_bits + ε_b − 1`, where the exponent `ε_b` comes from the shared
/// [`Qcd::subband_step`] mapping (so it always matches the dequant step).
///
/// `Mb` depends only on the component's quantization parameters, never on its
/// bit depth `R_I` — the depth enters the *step size* (E-3), not the bit-plane
/// count. A reversible encoder bakes the depth into the exponent it writes, so a
/// decoder that read it back and added `R_I` again would double-count.
///
/// It is per component because a QCC can give one component its own guard bits
/// and exponents; without an override the component's parameters are QCD's.
fn numbps(header: &MainHeader, comp: usize, band: usize) -> Result<u32> {
    let qcd = &header
        .components
        .get(comp)
        .ok_or_else(|| Error::Inconsistent(format!("no coding parameters for component {comp}")))?
        .quant;
    let (exp, _) = qcd.subband_step(band).ok_or_else(|| {
        Error::Inconsistent(format!(
            "component {comp} carries {} step sizes but subband {band} needs one",
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
fn decode_subband<T, F>(
    sb: &Subband<'_>,
    numbps: u32,
    params: BlockParams,
    convert: F,
) -> Result<Band<T>>
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
        // Maxshift lifts every region-of-interest coefficient above every
        // background one, so the block starts `roi_shift` planes higher (ISO
        // H.2's Kmax = Mb + s); see `top_coded_plane` for the interplay with
        // the zero bit-planes.
        //
        // The double-scale reconstruction shifts `1 << top`, so reject
        // high-dynamic-range subbands that would overflow `i32` rather than
        // panic — the same rejection OpenJPEG makes with `bpno_plus_one >= 31`.
        // This does not bound the maxshift itself: a block with more zero
        // bit-planes than `Mb` keeps `top` small under any `SPrgn`, and
        // `undo_maxshift` carries the oracle's `roishift >= 31` arm for it.
        let top = top_coded_plane(numbps, block.zero_bit_planes, params.roi_shift);
        if top > MAX_BIT_PLANES {
            return Err(Error::Unsupported(format!(
                "code-block needs {top} bit-planes, over the {MAX_BIT_PLANES}-plane limit"
            )));
        }
        let mut state = BlockState::new(block.width as u32, block.height as u32);
        // Each codeword segment is decoded from its own MQ stream; a segment's
        // bytes may still arrive in several layer-sized chunks, so they are
        // concatenated first. Without termination there is exactly one segment.
        let segments: Vec<(Vec<u8>, u32)> = block
            .segments
            .iter()
            .map(|segment| (segment.bytes(), segment.passes))
            .collect();
        decode_block(
            &segments,
            &mut state,
            orient,
            numbps,
            block.num_passes,
            block.zero_bit_planes,
            params,
        )?;
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
                segments: vec![crate::tier2::CodedSegment {
                    passes: 1,
                    chunks: vec![&[0x80][..]],
                }],
            }],
        }
    }

    /// A subband whose coded bit-plane count `Mb − zero_bit_planes` exceeds the
    /// `i32` double-scale limit is rejected as unsupported, not decoded into an
    /// overflow.
    #[test]
    fn excessive_bit_planes_rejected() {
        let sb = one_block_subband(1, 0);
        let err = decode_subband::<i32, _>(&sb, MAX_BIT_PLANES + 1, default_params(), |q| q)
            .expect_err("must reject");
        assert!(matches!(err, Error::Unsupported(_)), "got {err:?}");
    }

    /// A maxshift pushes the same subband over the limit even when its own
    /// bit-plane count is in range: the region-of-interest coefficients sit
    /// `roi_shift` planes above the background ones.
    #[test]
    fn a_maxshift_that_overflows_the_bit_planes_is_rejected() {
        let sb = one_block_subband(1, 0);
        let params = BlockParams {
            style: 0,
            roi_shift: 1,
        };
        let err = decode_subband::<i32, _>(&sb, MAX_BIT_PLANES, params, |q| q)
            .expect_err("MAX_BIT_PLANES + 1 planes");
        assert!(matches!(err, Error::Unsupported(_)), "got {err:?}");

        // One less, and the same block decodes.
        decode_subband::<i32, _>(&sb, MAX_BIT_PLANES - 1, params, |q| q).expect("in range");
    }

    /// More signalled zero bit-planes than the subband has magnitude planes
    /// decodes the block to all zeros without an error — the exact outcome of
    /// OpenJPEG's wrapped-negative `bpno_plus_one` running zero passes.
    #[test]
    fn excess_zero_bit_planes_decodes_to_zeros() {
        let sb = one_block_subband(1, 5);
        let band = decode_subband::<i32, _>(&sb, 4, default_params(), |q| q)
            .expect("saturates to zero coded planes, as the oracle does");
        assert!(band.data.iter().all(|&c| c == 0));
    }

    /// The largest in-range bit-plane count decodes without error or overflow.
    #[test]
    fn max_bit_planes_accepted() {
        let sb = one_block_subband(1, 0);
        let band = decode_subband::<i32, _>(&sb, MAX_BIT_PLANES, default_params(), |q| q)
            .expect("in range");
        assert_eq!(band.data.len(), 1);
    }

    fn default_params() -> BlockParams {
        BlockParams::default()
    }
}
