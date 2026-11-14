//! Stage 3 — Tier-1 / EBCOT block coding (ISO/IEC 15444-1 Annex C-D).
//!
//! The core and the hardest part. Each code-block's coded bytes are an MQ
//! arithmetic-coded bit-plane stream. From the most significant non-zero plane
//! down, every plane is decoded by up to three passes — significance
//! propagation, magnitude refinement, cleanup — each forming contexts from the
//! 3×3 neighborhood and reading binary decisions from the [`mq`] decoder.
//! The result is the quantized wavelet coefficients of one subband block.

pub mod mq;
pub mod passes;

use crate::codestream::MainHeader;
use crate::codestream::markers::Transform;
use crate::pipeline::{Band, Bands, DetailBands, SubbandCoeffs};
use crate::quant;
use crate::tier1::passes::{
    BlockParams, BlockState, MAX_BIT_PLANES, Orientation, decode_block, top_coded_plane,
};
use crate::tier2::{BandKind, CodeBlock, CodedData, ComponentCoded, Resolution, Subband};
use crate::{Error, Result};

/// Decode every code-block's MQ/EBCOT stream into quantized subband
/// coefficients, assembling code-blocks back into their subbands.
///
/// The arithmetic is integer regardless of filter — Tier-1 recovers the
/// quantized indices — so the COD transform only fixes how those indices are
/// carried onward: the 5/3 reversible path keeps them as `i32` (the inverse is
/// exact), while the 9/7 irreversible path widens them to `f32` and scales
/// each by its subband's step as the block is placed ([`quant::half_step`]).
/// [`Bands`] mirrors one component of
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
                    |_| Ok(|q| q / 2),
                )?)),
                // On the irreversible arm the half bit must survive into the
                // float: OpenJPEG multiplies by `0.5f * band->stepsize` as it
                // copies the block into the band, so the halving happens in
                // float, not as an integer truncation. `q` can be odd, and
                // dropping its low bit here loses up to half a quantization
                // step on every coefficient. Dequantization is that multiply,
                // folded into placement: each band gets its own factor.
                Transform::Irreversible97 => Ok(SubbandCoeffs::Irreversible(assemble(
                    header,
                    index,
                    component,
                    reduction,
                    |band| {
                        let half_step = quant::half_step(header, index, band)?;
                        Ok(move |q| q as f32 * half_step)
                    },
                )?)),
            }
        })
        .collect()
}

/// The fewest sample-passes (see [`work`]) a decode may spend in the block
/// decoder. A sample-pass costs about 3 ns where a pass finds little to
/// code, but 7 to 8 ns on the data slowest to decode (every byte 0x4e), so
/// this is about 0.5 s.
pub(crate) const DECODE_WORK_MIN: u64 = 1 << 26;

/// The sample-passes a decode may spend per byte of its tiles' data, when
/// that comes to more than [`DECODE_WORK_MIN`].
///
/// Encoder output spends up to about 14000 per byte: `opj_compress -n 1`
/// (no wavelet level) on a flat 16-bit image, where every 64x64 code-block
/// runs about 46 passes for about 14 bytes. With the wavelet, a flat image
/// with a few bright specks spends up to about 5600. Most files spend under
/// 120. So this is about 2.35 times the most seen. A packet header can ask
/// for 88 passes over a 64x64 code-block in about 4 bytes, about 100000
/// sample-passes per byte, so this cuts a file's worst case to a third,
/// about 0.25 ms a byte on the slowest data, and refuses it before any of
/// that work is done.
pub(crate) const DECODE_WORK_PER_BYTE: u64 = 1 << 15;

/// The sample-passes the block decoder may spend on tiles that hold `bytes`
/// bytes of data in all, packed packet headers included.
pub(crate) fn decode_work(bytes: u64) -> u64 {
    DECODE_WORK_MIN.max(bytes.saturating_mul(DECODE_WORK_PER_BYTE))
}

/// The work [`decode_code_blocks`] will do on `coded`: for each code-block
/// it decodes, the coding passes it runs times the block's samples. Each
/// pass scans the whole block, however few bytes it carries, so a packet
/// header of a few bits per code-block can ask for up to 88 passes over
/// 4096 samples each.
///
/// The passes run are the block's passes, but no more than the `3 × top − 2`
/// its coded bit-planes hold; a block of no coded plane, or one the decode
/// refuses for too many, runs none. The code-blocks a resolution reduction
/// drops are not decoded and not counted.
pub(crate) fn work(header: &MainHeader, coded: &CodedData<'_>, reduction: u8) -> u64 {
    let mut total = 0u64;
    for (comp, component) in coded.components.iter().enumerate() {
        let Some(params) = header.components.get(comp) else {
            continue;
        };
        let kept = component
            .resolutions
            .len()
            .saturating_sub(usize::from(reduction))
            .max(1);
        for (res, resolution) in component.resolutions.iter().take(kept).enumerate() {
            for subband in &resolution.subbands {
                let band = band_index(res, subband.kind);
                // A band without a step size fails its decode; it costs nothing.
                let Ok(numbps) = numbps(header, comp, band) else {
                    continue;
                };
                for block in &subband.blocks {
                    let passes = passes_run(numbps, block, params.roi_shift);
                    let samples = (block.width as u64).saturating_mul(block.height as u64);
                    total = total.saturating_add(passes.saturating_mul(samples));
                }
            }
        }
    }
    total
}

/// The coding passes [`decode_block`] runs on `block`: its passes, but no
/// more than its coded bit-planes hold, the first plane one pass and each
/// later one three.
fn passes_run(numbps: u32, block: &CodeBlock<'_>, roi_shift: u8) -> u64 {
    if block.num_passes == 0 {
        return 0;
    }
    let top = top_coded_plane(numbps, block.zero_bit_planes, roi_shift);
    if !(1..=MAX_BIT_PLANES).contains(&top) {
        return 0;
    }
    u64::from(block.num_passes.min(3 * top - 2))
}

/// The QCD subband index of a `kind` band in resolution `res`: LL is 0, then
/// each resolution level coarsest-first as `HL, LH, HH`.
fn band_index(res: usize, kind: BandKind) -> usize {
    match (res, kind) {
        (0, _) => 0,
        (_, BandKind::Hl) => 3 * res - 2,
        (_, BandKind::Lh) => 3 * res - 1,
        (_, _) => 3 * res,
    }
}

/// Decode every subband into a [`Bands`] pyramid. `convert_for(band)` gives
/// the function that turns each quantized index of subband `band` into its
/// stored value: `q / 2` for the reversible path, the index times the band's
/// half step for the irreversible one.
///
/// Subbands run in QCD order (see [`band_index`]), which sets each band's
/// magnitude bit-plane count `Mb` (guard bits + quantization exponent - 1)
/// that Tier-1 needs to place bits at their true weights.
fn assemble<T, F, G>(
    header: &MainHeader,
    comp: usize,
    coded: &ComponentCoded<'_>,
    reduction: u8,
    convert_for: G,
) -> Result<Bands<T>>
where
    T: Copy + Default,
    F: Fn(i32) -> T,
    G: Fn(usize) -> Result<F>,
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
        .ok_or_else(|| Error::Inconsistent("tier-2 produced no resolutions to decode".into()))?;
    let decode = |res: usize, resolution: &Resolution<'_>, kind: BandKind| {
        let band = band_index(res, kind);
        decode_subband(
            subband_of(resolution, kind)?,
            numbps(header, comp, band)?,
            params,
            convert_for(band)?,
        )
    };
    let ll = decode(0, coarsest, BandKind::Ll)?;

    let mut levels = Vec::with_capacity(keep);
    for (level, resolution) in resolutions.take(keep).enumerate() {
        let res = level + 1;
        levels.push(DetailBands {
            hl: decode(res, resolution, BandKind::Hl)?,
            lh: decode(res, resolution, BandKind::Lh)?,
            hh: decode(res, resolution, BandKind::Hh)?,
        });
    }

    Ok(Bands { ll, levels })
}

/// The magnitude bit-plane count `Mb` for subband index `band` (ISO E-2):
/// `guard_bits + ε_b − 1`, where the exponent `ε_b` comes from the shared
/// [`Qcd::subband_step`](crate::codestream::markers::Qcd::subband_step) mapping (so it always matches the dequant step).
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
            return Err(Error::Limit(format!(
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
        kind_block_subband(BandKind::Ll, num_passes, zero_bit_planes)
    }

    /// As [`one_block_subband`], for a band of orientation `kind`.
    fn kind_block_subband(
        kind: BandKind,
        num_passes: u32,
        zero_bit_planes: u32,
    ) -> Subband<'static> {
        Subband {
            kind,
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
    /// `i32` double-scale limit is rejected at the guard, not decoded into an
    /// overflow.
    #[test]
    fn excessive_bit_planes_rejected() {
        let sb = one_block_subband(1, 0);
        let err = decode_subband::<i32, _>(&sb, MAX_BIT_PLANES + 1, default_params(), |q| q)
            .expect_err("must reject");
        assert!(matches!(err, Error::Limit(_)), "got {err:?}");
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
        assert!(matches!(err, Error::Limit(_)), "got {err:?}");

        // One less, and the same block decodes.
        decode_subband::<i32, _>(&sb, MAX_BIT_PLANES - 1, params, |q| q).expect("in range");
    }

    /// More signaled zero bit-planes than the subband has magnitude planes
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

    /// A one-sample band whose block decodes to a nonzero index.
    fn coded_subband(kind: BandKind) -> Subband<'static> {
        let mut sb = kind_block_subband(kind, 1, 0);
        sb.blocks[0].segments[0].chunks = vec![&[0x00, 0x00, 0x00][..]];
        sb
    }

    /// Subband indices follow QCD order: LL is 0, then each resolution
    /// coarsest-first as HL, LH, HH.
    #[test]
    fn band_index_follows_qcd_order() {
        for kind in [BandKind::Ll, BandKind::Hl, BandKind::Lh, BandKind::Hh] {
            assert_eq!(band_index(0, kind), 0);
        }
        assert_eq!(band_index(1, BandKind::Hl), 1);
        assert_eq!(band_index(1, BandKind::Lh), 2);
        assert_eq!(band_index(1, BandKind::Hh), 3);
        assert_eq!(band_index(2, BandKind::Hl), 4);
        assert_eq!(band_index(2, BandKind::Lh), 5);
        assert_eq!(band_index(2, BandKind::Hh), 6);
    }

    /// Each 9/7 band is placed scaled by its own step: with the same coded
    /// block in every band and a distinct step per band (same exponent, so
    /// the same bit-plane count), a swapped pairing shows in the values.
    #[test]
    fn each_irreversible_band_is_scaled_by_its_own_step() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/irreversible_lossy.j2k"
        ))
        .unwrap();
        let mut header = crate::codestream::parse_header(&bytes).unwrap().header;
        assert_eq!(
            header.components[0].coding.transform,
            Transform::Irreversible97
        );
        header.components[0].quant.style = crate::codestream::markers::QuantStyle::ScalarExpounded;
        header.components[0].quant.guard_bits = 2;
        header.components[0].quant.steps = vec![(8, 0), (8, 100), (8, 700), (8, 1500)];
        let coded = CodedData {
            components: vec![ComponentCoded {
                resolutions: vec![
                    Resolution {
                        subbands: vec![coded_subband(BandKind::Ll)],
                    },
                    Resolution {
                        subbands: vec![
                            coded_subband(BandKind::Hl),
                            coded_subband(BandKind::Lh),
                            coded_subband(BandKind::Hh),
                        ],
                    },
                ],
            }],
        };
        // The quantized index every band decodes to, before scaling.
        let q = decode_subband::<i32, _>(
            &coded.components[0].resolutions[0].subbands[0],
            numbps(&header, 0, 0).unwrap(),
            default_params(),
            |q| q,
        )
        .unwrap()
        .data[0];
        assert_ne!(q, 0);

        let out = decode_code_blocks(&header, &coded, 0).unwrap();
        let SubbandCoeffs::Irreversible(bands) = &out[0] else {
            panic!("expected the irreversible arm");
        };
        let got = [
            bands.ll.data[0],
            bands.levels[0].hl.data[0],
            bands.levels[0].lh.data[0],
            bands.levels[0].hh.data[0],
        ];
        for (band, value) in got.into_iter().enumerate() {
            let step = quant::half_step(&header, 0, band).unwrap();
            assert_eq!(value.to_bits(), (q as f32 * step).to_bits(), "band {band}");
        }
        // The four steps differ, so a swapped pairing cannot pass.
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(got[i].to_bits(), got[j].to_bits());
            }
        }
    }

    fn default_params() -> BlockParams {
        BlockParams::default()
    }
}

#[cfg(test)]
mod work_bound_tests;
