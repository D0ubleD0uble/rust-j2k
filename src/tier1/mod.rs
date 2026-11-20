//! Stage 3 — Tier-1 block decoding.
//!
//! Each code-block's coded bytes decode to the quantized wavelet coefficients
//! of one subband block, which this module assembles back into subbands. The
//! block decoding itself sits behind [`BlockCoder`], the one seam between
//! assembly and a block coder. The Part 1 coder, EBCOT (ISO/IEC 15444-1
//! Annexes C and D), lives in [`ebcot`]; the HTJ2K coder (ISO/IEC 15444-15)
//! lives in [`ht`].

pub(crate) mod ebcot;
mod ht;

use crate::codestream::MainHeader;
use crate::codestream::markers::Transform;
use crate::pipeline::{Band, Bands, DetailBands, SubbandCoeffs};
use crate::quant;
use crate::tier2::{
    BandExtent, BandKind, CodeBlock, CodedData, Coder, ComponentCoded, Resolution, Subband,
};
use crate::{Error, Result};

/// A block coder with the parameters a tile-component gives it.
///
/// Each code-block's coder is the one tier-2 says it was coded with
/// ([`CodeBlock::coder`]): the tile-component's, or, under a MIXED
/// code-block style, its own (see [`Coders`]). Assembly hands the block to
/// [`decode`](Self::decode), which fills
/// the block's coefficients at the scale [`ebcot::passes::decode_block`]
/// documents, and asks [`cost`](Self::cost) what that decode spends. Nothing
/// else of a coder is visible to assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockCoder {
    /// The Part 1 coder, with the code-block style flags and the component's
    /// region-of-interest maxshift.
    Ebcot { style: u8, roi_shift: u8 },
    /// The HTJ2K coder, with the component's region-of-interest maxshift,
    /// whether the vertically causal context flag is set, and the bound
    /// `CAP` sets on its cleanup magnitudes.
    Ht {
        roi_shift: u8,
        causal: bool,
        bound: CleanupBound,
    },
}

/// The bound a `CAP` marker sets on the HT cleanup magnitudes `mu` of a
/// tile-component (ISO/IEC 15444-15 A.3.7, 8.7.3): `mu < 2^B`, except in an
/// irreversible subband with `B <= 31`, where it is
/// `mu < 2^min(31, B + n_b - 1)` for the subband's decomposition level
/// `n_b`. A codestream without a Part 15 `CAP` sets none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CleanupBound {
    /// `B`, from `Ccap15` bits 0-4.
    magb: Option<u32>,
    irreversible: bool,
}

impl CleanupBound {
    /// The bound in a subband of decomposition level `level`, as its bit
    /// count: every `mu` is below `2^bits`. `None` when there is no bound.
    fn bits(self, level: u32) -> Option<u32> {
        let b = self.magb?;
        Some(if self.irreversible && b <= 31 {
            // `n_b` is 0 only for the LL band of a component with no
            // decomposition level.
            (b + level).saturating_sub(1).min(31)
        } else {
            b
        })
    }
}

#[cfg(feature = "__fuzz")]
/// Decode one HT code-block, for the fuzz hook (`crate::fuzz::ht_block`),
/// which holds no [`BlockCoder`].
pub(crate) fn decode_ht_block(
    block: &CodeBlock<'_>,
    numbps: u32,
    roi_shift: u8,
    causal: bool,
    bound_bits: Option<u32>,
) -> Result<Vec<i32>> {
    ht::decode(
        block,
        numbps,
        ht::Params {
            roi_shift,
            causal,
            bound_bits,
        },
    )
}

/// The two block coders of a tile-component, set up from its code-block
/// style. A block uses the one [`CodeBlock::coder`] names: under a MIXED
/// style each block names its own, and otherwise every block names the one
/// the style selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Coders {
    part1: BlockCoder,
    ht: BlockCoder,
    /// Whether each block's coder was told from its length fields (a MIXED
    /// style), so an HT block's failure may be a Part 1 block misread.
    mixed: bool,
}

impl Coders {
    /// The coders of component `comp`. The Part 1 coder takes the style
    /// without its HT bits; under MIXED the other bits apply to the Part 1
    /// blocks (ISO/IEC 15444-15 Table A.4), and of them the HT blocks use
    /// only the vertically causal context.
    fn of(header: &MainHeader, comp: usize) -> Self {
        let params = &header.components[comp];
        let style = params.coding.code_block_style;
        let roi_shift = params.roi_shift;
        use crate::codestream::markers::code_block_style::{HT, HT_MIXED, VCAUSAL};
        Coders {
            mixed: Coder::of_style(style, header.ccap15).is_none(),
            part1: BlockCoder::Ebcot {
                style: style & !(HT | HT_MIXED),
                roi_shift,
            },
            ht: BlockCoder::Ht {
                roi_shift,
                causal: style & VCAUSAL != 0,
                bound: CleanupBound {
                    magb: header.ccap15.map(|c| c.magnitude_bound()),
                    irreversible: params.coding.transform == Transform::Irreversible97,
                },
            },
        }
    }

    /// The coder of `block`.
    fn of_block(self, block: &CodeBlock<'_>) -> BlockCoder {
        match block.coder {
            Coder::Part1 => self.part1,
            Coder::Ht => self.ht,
        }
    }

    /// Decode `block` with its coder ([`BlockCoder::decode`]). An HT block of
    /// a MIXED tile-component that fails as malformed says how it came to be
    /// read as HT, since a Part 1 block whose `Lblock` grew more than it
    /// needed passes the same test.
    fn decode<'s, 'a>(
        self,
        block: &CodeBlock<'a>,
        kind: BandKind,
        level: u32,
        numbps: u32,
        scratch: &'s mut Scratch<'a>,
    ) -> Result<&'s [i32]> {
        let decoded = self
            .of_block(block)
            .decode(block, kind, level, numbps, scratch);
        match decoded {
            Err(Error::Codestream(message)) if self.mixed && block.coder == Coder::Ht => {
                Err(Error::codestream(format!(
                    "{message} (a code-block of a MIXED tile-component, read as an HT \
                     block since its first nonzero length field starts with 0 at Lblock \
                     above 3, ISO/IEC 15444-15 A.4; a Part 1 block whose Lblock grew \
                     more than it needed reads this way too)"
                )))
            }
            other => other,
        }
    }
}

/// The buffers of both block coders, kept across the code-blocks of a
/// component so that each block reuses them rather than allocating its own.
/// Each coder resets the ones it uses at the start of every block.
#[derive(Debug, Default)]
struct Scratch<'a> {
    ebcot: ebcot::Scratch<'a>,
    ht: ht::Scratch,
}

impl BlockCoder {
    /// Decode `block`, a code-block of a `kind` band of decomposition level
    /// `level` whose magnitude bit-plane count is `numbps`, into its
    /// coefficients, row-major over the block, at twice their weight. The
    /// coefficients are `scratch`'s until its next decode.
    fn decode<'s, 'a>(
        self,
        block: &CodeBlock<'a>,
        kind: BandKind,
        level: u32,
        numbps: u32,
        scratch: &'s mut Scratch<'a>,
    ) -> Result<&'s [i32]> {
        match self {
            BlockCoder::Ebcot { style, roi_shift } => {
                ebcot::decode(block, kind, numbps, style, roi_shift, &mut scratch.ebcot)
            }
            BlockCoder::Ht {
                roi_shift,
                causal,
                bound,
            } => ht::decode_into(
                block,
                numbps,
                ht::Params {
                    roi_shift,
                    causal,
                    bound_bits: bound.bits(level),
                },
                &mut scratch.ht,
            ),
        }
    }

    /// The coding passes [`decode`](Self::decode) runs on `block`; each one
    /// scans every sample of the block.
    fn cost(self, block: &CodeBlock<'_>, numbps: u32) -> u64 {
        match self {
            BlockCoder::Ebcot { roi_shift, .. } => ebcot::passes_run(numbps, block, roi_shift),
            BlockCoder::Ht { roi_shift, .. } => ht::passes_run(block, numbps, roi_shift),
        }
    }
}

/// Decode every code-block into quantized subband coefficients, assembling
/// code-blocks back into their subbands.
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
/// Returns one [`SubbandCoeffs`] per component, in SIZ order, for each
/// component `wanted` marks, and `None` for the rest, whose code-blocks are
/// not decoded. Components are independent here: no inter-component
/// transform is applied.
///
/// `reduction` is the resolution reduction: that many of each component's
/// finest resolutions are dropped from the pyramid, their code-blocks skipped
/// rather than decoded. The caller has already checked it leaves every
/// component at least its coarsest resolution.
pub fn decode_code_blocks(
    header: &MainHeader,
    coded: &CodedData<'_>,
    reduction: u8,
    wanted: &[bool],
) -> Result<Vec<Option<SubbandCoeffs>>> {
    coded
        .components
        .iter()
        .enumerate()
        .map(|(index, component)| {
            if !wanted.get(index).copied().unwrap_or(false) {
                return Ok(None);
            }
            // The wavelet is per component: a COC can put one component on 5/3
            // and another on 9/7.
            let transform = header
                .components
                .get(index)
                .ok_or_else(|| {
                    Error::inconsistent(format!("no coding parameters for component {index}"))
                })?
                .coding
                .transform;
            match transform {
                // Drop the double-scale half bit. On the reversible arm this is
                // OpenJPEG's integer `datap[i] /= 2`, exact by construction.
                Transform::Reversible53 => Ok(Some(SubbandCoeffs::Reversible(assemble(
                    header,
                    index,
                    component,
                    reduction,
                    |_| Ok(|q| q / 2),
                )?))),
                // On the irreversible arm the half bit must survive into the
                // float: OpenJPEG multiplies by `0.5f * band->stepsize` as it
                // copies the block into the band, so the halving happens in
                // float, not as an integer truncation. `q` can be odd, and
                // dropping its low bit here loses up to half a quantization
                // step on every coefficient. Dequantization is that multiply,
                // folded into placement: each band gets its own factor.
                Transform::Irreversible97 => Ok(Some(SubbandCoeffs::Irreversible(assemble(
                    header,
                    index,
                    component,
                    reduction,
                    |band| {
                        let half_step = quant::half_step(header, index, band)?;
                        Ok(move |q| q as f32 * half_step)
                    },
                )?))),
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
/// For a Part 1 block the passes run are the block's passes, but no more than
/// the `3 × top − 2` its coded bit-planes hold; a block of no coded plane, or
/// one the decode refuses for too many, runs none. An HT block is counted by
/// the passes the HT decoder would run (its cleanup pass, and its refinement
/// passes when the block has a non-empty refinement segment and a plane left
/// to refine), also when the decode then refuses it for too many zero
/// bit-planes, a cleanup plane shifted 30 or more, more than 3 passes, or a malformed `Lcup`
/// or `Scup`: its count is the work the bound is charged, not the work done. The
/// code-blocks a resolution reduction drops, and those of the components
/// `wanted` does not mark, are not decoded and not counted.
pub(crate) fn work(
    header: &MainHeader,
    coded: &CodedData<'_>,
    reduction: u8,
    wanted: &[bool],
) -> u64 {
    let mut total = 0u64;
    for (comp, component) in coded.components.iter().enumerate() {
        if header.components.get(comp).is_none() || !wanted.get(comp).copied().unwrap_or(false) {
            continue;
        }
        let coders = Coders::of(header, comp);
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
                    let passes = coders.of_block(block).cost(block, numbps);
                    let samples = (block.width as u64).saturating_mul(block.height as u64);
                    total = total.saturating_add(passes.saturating_mul(samples));
                }
            }
        }
    }
    total
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
fn assemble<'a, T, F, G>(
    header: &MainHeader,
    comp: usize,
    coded: &ComponentCoded<'a>,
    reduction: u8,
    convert_for: G,
) -> Result<Bands<T>>
where
    T: Copy + Default,
    F: Fn(i32) -> T,
    G: Fn(usize) -> Result<F>,
{
    let coders = Coders::of(header, comp);
    // A resolution reduction drops that many of the finest levels: their
    // packets were parsed (the codestream's framing demands it) but their
    // code-blocks never reach the bit-plane decoder. The caller has checked
    // `reduction` against this component's decomposition count, so `keep`
    // cannot underflow past the coarsest resolution.
    let keep = coded
        .resolutions
        .len()
        .saturating_sub(1 + usize::from(reduction));
    // The decomposition levels: the LL band is at level `NL`, and resolution
    // `r` above it holds the bands of level `NL + 1 - r`.
    let levels = u32::try_from(coded.resolutions.len().saturating_sub(1)).unwrap_or(u32::MAX);
    let mut resolutions = coded.resolutions.iter();
    let coarsest = resolutions
        .next()
        .ok_or_else(|| Error::inconsistent("tier-2 produced no resolutions to decode"))?;
    let mut scratch = Scratch::default();
    let mut decode = |res: usize, resolution: &Resolution<'a>, kind: BandKind| {
        let band = band_index(res, kind);
        let level = match res {
            0 => levels,
            _ => (levels + 1).saturating_sub(res as u32),
        };
        let listed;
        let sb = match subband_of(resolution, kind) {
            Ok(sb) => sb,
            // A resolution that holds no precinct lists none of its bands.
            Err(_) if resolution.subbands.is_empty() => {
                listed = empty_subband(extent_of(coded, res, kind)?, kind);
                &listed
            }
            Err(error) => return Err(error),
        };
        decode_subband(
            sb,
            level,
            numbps(header, comp, band)?,
            coders,
            &mut scratch,
            convert_for(band)?,
        )
    };
    let ll = decode(0, coarsest, BandKind::Ll)?;

    let mut levels = Vec::with_capacity(keep);
    for (level, resolution) in resolutions.take(keep).enumerate() {
        let res = level + 1;
        if resolution.subbands.iter().all(|s| s.width * s.height == 0) {
            // No sample to decode and none to place. The bit-plane counts are
            // still looked up, so a missing step size fails as it does for a
            // band that holds blocks.
            //
            // A resolution that lists no band gets its three from one pass
            // over its rectangle.
            let [hl, lh, hh] = match coded.detail_extents(res) {
                Some(extents) => extents.map(Some),
                None => [BandKind::Hl, BandKind::Lh, BandKind::Hh].map(|k| coded.band(res, k)),
            };
            levels.push(DetailBands {
                hl: empty_band(header, comp, res, BandKind::Hl, hl)?,
                lh: empty_band(header, comp, res, BandKind::Lh, lh)?,
                hh: empty_band(header, comp, res, BandKind::Hh, hh)?,
            });
            continue;
        }
        levels.push(DetailBands {
            hl: decode(res, resolution, BandKind::Hl)?,
            lh: decode(res, resolution, BandKind::Lh)?,
            hh: decode(res, resolution, BandKind::Hh)?,
        });
    }

    Ok(Bands { ll, levels })
}

/// The origin and size of the `kind` band of resolution `res`.
fn extent_of(coded: &ComponentCoded<'_>, res: usize, kind: BandKind) -> Result<BandExtent> {
    coded
        .band(res, kind)
        .ok_or_else(|| Error::inconsistent(format!("resolution is missing its {kind:?} subband")))
}

/// A subband of `extent` with no code-block.
fn empty_subband(extent: BandExtent, kind: BandKind) -> Subband<'static> {
    Subband {
        kind,
        origin: extent.origin,
        width: extent.width,
        height: extent.height,
        block_cols: 0,
        block_rows: 0,
        blocks: Vec::new(),
    }
}

/// The band of orientation `kind` of resolution `res`, which holds no sample:
/// what [`decode_subband`] returns for it, without the buffer or the
/// dequantization step.
fn empty_band<T>(
    header: &MainHeader,
    comp: usize,
    res: usize,
    kind: BandKind,
    extent: Option<BandExtent>,
) -> Result<Band<T>> {
    let extent = extent.ok_or_else(|| {
        Error::inconsistent(format!("resolution is missing its {kind:?} subband"))
    })?;
    numbps(header, comp, band_index(res, kind))?;
    Ok(Band {
        origin: extent.origin,
        width: extent.width,
        height: extent.height,
        data: Vec::new(),
    })
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
        .ok_or_else(|| Error::inconsistent(format!("no coding parameters for component {comp}")))?
        .quant;
    let (exp, _) = qcd.subband_step(band).ok_or_else(|| {
        Error::inconsistent(format!(
            "component {comp} carries {} step sizes but subband {band} needs one",
            qcd.steps.len()
        ))
    })?;
    // Mb = guard + ε − 1, floored at 0 (a band with no magnitude planes decodes
    // to all zeros).
    Ok((u32::from(qcd.guard_bits) + u32::from(exp)).saturating_sub(1))
}

/// The subband of orientation `kind` listed by one resolution. Tier-2 lists
/// the full set (one `Ll`, or `Hl/Lh/Hh`) for a resolution that holds a
/// precinct, so a miss there is an internal inconsistency rather than
/// malformed input. A resolution with no precinct lists none; see
/// [`ComponentCoded::band`].
fn subband_of<'a, 'b>(res: &'a Resolution<'b>, kind: BandKind) -> Result<&'a Subband<'b>> {
    res.subbands
        .iter()
        .find(|s| s.kind == kind)
        .ok_or_else(|| Error::inconsistent(format!("resolution is missing its {kind:?} subband")))
}

/// Decode one subband's code-blocks into a row-major coefficient [`Band`].
/// `level` is the band's decomposition level and `numbps` its magnitude
/// bit-plane count `Mb`. Absent blocks (no coding passes) are left at the
/// band's zero fill. The blocks are decoded in `scratch`'s buffers.
fn decode_subband<'a, T, F>(
    sb: &Subband<'a>,
    level: u32,
    numbps: u32,
    coders: Coders,
    scratch: &mut Scratch<'a>,
    convert: F,
) -> Result<Band<T>>
where
    T: Copy + Default,
    F: Fn(i32) -> T,
{
    let mut data = vec![T::default(); sb.width * sb.height];
    for block in &sb.blocks {
        // An absent block contributes nothing; its samples stay zero.
        if block.num_passes == 0 {
            continue;
        }
        let coeffs = coders.decode(block, sb.kind, level, numbps, scratch)?;
        place_block(&mut data, sb.width, block, coeffs, &convert);
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
    coeffs: &[i32],
    convert: &F,
) where
    F: Fn(i32) -> T,
{
    for row in 0..block.height {
        let dst = (block.y + row) * band_width + block.x;
        let src = row * block.width;
        for col in 0..block.width {
            data[dst + col] = convert(coeffs[src + col]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ebcot::passes::MAX_BIT_PLANES;
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
                segments: [(1, [&[0x80][..]])].into_iter().collect(),
                cut: false,
                placeholders: 0,
                coder: Coder::Part1,
                missing_from: None,
            }],
        }
    }

    /// A subband whose coded bit-plane count `Mb − zero_bit_planes` exceeds the
    /// `i32` double-scale limit is rejected at the guard, not decoded into an
    /// overflow.
    #[test]
    fn excessive_bit_planes_rejected() {
        let sb = one_block_subband(1, 0);
        let err = decode_subband::<i32, _>(
            &sb,
            1,
            MAX_BIT_PLANES + 1,
            default_params(),
            &mut Scratch::default(),
            |q| q,
        )
        .expect_err("must reject");
        assert!(matches!(err, Error::Limit(_)), "got {err:?}");
    }

    /// A maxshift pushes the same subband over the limit even when its own
    /// bit-plane count is in range: the region-of-interest coefficients sit
    /// `roi_shift` planes above the background ones.
    #[test]
    fn a_maxshift_that_overflows_the_bit_planes_is_rejected() {
        let sb = one_block_subband(1, 0);
        let params = part1_coders(1);
        let err = decode_subband::<i32, _>(
            &sb,
            1,
            MAX_BIT_PLANES,
            params,
            &mut Scratch::default(),
            |q| q,
        )
        .expect_err("MAX_BIT_PLANES + 1 planes");
        assert!(matches!(err, Error::Limit(_)), "got {err:?}");

        // One less, and the same block decodes.
        decode_subband::<i32, _>(
            &sb,
            1,
            MAX_BIT_PLANES - 1,
            params,
            &mut Scratch::default(),
            |q| q,
        )
        .expect("in range");
    }

    /// More signaled zero bit-planes than the subband has magnitude planes
    /// decodes the block to all zeros without an error — the exact outcome of
    /// OpenJPEG's wrapped-negative `bpno_plus_one` running zero passes.
    #[test]
    fn excess_zero_bit_planes_decodes_to_zeros() {
        let sb = one_block_subband(1, 5);
        let band =
            decode_subband::<i32, _>(&sb, 1, 4, default_params(), &mut Scratch::default(), |q| q)
                .expect("saturates to zero coded planes, as the oracle does");
        assert!(band.data.iter().all(|&c| c == 0));
    }

    /// The largest in-range bit-plane count decodes without error or overflow.
    #[test]
    fn max_bit_planes_accepted() {
        let sb = one_block_subband(1, 0);
        let band = decode_subband::<i32, _>(
            &sb,
            1,
            MAX_BIT_PLANES,
            default_params(),
            &mut Scratch::default(),
            |q| q,
        )
        .expect("in range");
        assert_eq!(band.data.len(), 1);
    }

    /// A one-sample band whose block decodes to a nonzero index.
    fn coded_subband(kind: BandKind) -> Subband<'static> {
        let mut sb = kind_block_subband(kind, 1, 0);
        let passes = sb.blocks[0].segments.first().unwrap().passes;
        sb.blocks[0].segments = [(passes, [&[0x00, 0x00, 0x00][..]])].into_iter().collect();
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
                rect: None,
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
            1,
            numbps(&header, 0, 0).unwrap(),
            default_params(),
            &mut Scratch::default(),
            |q| q,
        )
        .unwrap()
        .data[0];
        assert_ne!(q, 0);

        let out = decode_code_blocks(&header, &coded, 0, &[true]).unwrap();
        let Some(SubbandCoeffs::Irreversible(bands)) = &out[0] else {
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

    /// The bound of T.814 8.7.3: `2^B` in a reversible subband or for
    /// `B > 31`, else `2^min(31, B + n_b - 1)`.
    #[test]
    fn the_cleanup_bound_follows_8_7_3() {
        let bound = |magb, irreversible| CleanupBound { magb, irreversible };
        assert_eq!(bound(None, false).bits(3), None);
        assert_eq!(bound(None, true).bits(3), None);
        for level in [0, 1, 5] {
            assert_eq!(bound(Some(11), false).bits(level), Some(11));
            assert_eq!(bound(Some(35), true).bits(level), Some(35));
        }
        assert_eq!(bound(Some(11), true).bits(1), Some(11));
        assert_eq!(bound(Some(11), true).bits(3), Some(13));
        assert_eq!(bound(Some(11), true).bits(0), Some(10));
        assert_eq!(bound(Some(30), true).bits(5), Some(31));
        assert_eq!(bound(Some(31), true).bits(1), Some(31));
    }

    /// Each block of a MIXED tile-component goes to the coder tier-2 chose
    /// for it. Tier-2 settles the choice, so an HT block whose bytes fail as
    /// an HT cleanup segment (`Scup` below 2, B.4) is `Codestream`, not
    /// retried as a Part 1 block.
    #[test]
    fn a_mixed_block_goes_to_its_own_coder() {
        let coders = Coders {
            part1: BlockCoder::Ebcot {
                style: 0,
                roi_shift: 0,
            },
            ht: BlockCoder::Ht {
                roi_shift: 0,
                causal: false,
                bound: CleanupBound {
                    magb: None,
                    irreversible: false,
                },
            },
            mixed: true,
        };
        let mut sb = one_block_subband(1, 0);
        decode_subband::<i32, _>(&sb, 1, 1, coders, &mut Scratch::default(), |q| q)
            .expect("a Part 1 block");
        sb.blocks[0].coder = Coder::Ht;
        let passes = sb.blocks[0].segments.first().unwrap().passes;
        sb.blocks[0].segments = [(passes, [&[0, 0][..]])].into_iter().collect();
        let err = decode_subband::<i32, _>(&sb, 1, 1, coders, &mut Scratch::default(), |q| q)
            .expect_err("an HT block");
        let Error::Codestream(message) = &err else {
            panic!("got {err:?}")
        };
        assert!(
            message
                .message()
                .contains("MIXED tile-component, read as an HT block")
                && message.message().contains("A.4")
                && message
                    .message()
                    .contains("Lblock grew more than it needed"),
            "{message}"
        );

        // Outside MIXED the HT decoder's error is left as it is.
        let plain = Coders {
            mixed: false,
            ..coders
        };
        let err = decode_subband::<i32, _>(&sb, 1, 1, plain, &mut Scratch::default(), |q| q)
            .expect_err("an HT block");
        let Error::Codestream(message) = &err else {
            panic!("got {err:?}")
        };
        assert!(!message.message().contains("MIXED"), "{message}");
    }

    /// A MIXED style marks the coders as MIXED, with no `CAP` or with
    /// `Ccap15` bits 15-14 at `11`, but not under HTONLY (`00`), where every
    /// block is an HT block. Neither does an HT style alone.
    #[test]
    fn coders_of_marks_a_mixed_tile_component() {
        use crate::codestream::markers::Ccap15;
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/irreversible_lossy.j2k"
        ))
        .unwrap();
        let mut header = crate::codestream::parse_header(&bytes).unwrap().header;
        for (style, ccap15, mixed) in [
            (0xC0, None, true),
            (0xC0, Some(Ccap15(0xC000)), true),
            (0xC0, Some(Ccap15(0x0000)), false),
            (0x40, None, false),
            (0x00, None, false),
        ] {
            header.components[0].coding.code_block_style = style;
            header.ccap15 = ccap15;
            assert_eq!(
                Coders::of(&header, 0).mixed,
                mixed,
                "style {style:#04x}, {ccap15:?}"
            );
        }
    }

    /// `len` bytes from a fixed-seed generator, none of them 0xFF so that no
    /// marker ends the MQ data early.
    fn noise(seed: u32, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).max(1);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                ((x >> 24) as u8).min(0xFE)
            })
            .collect()
    }

    /// The zero bit-planes of the stale-state test blocks, in a band of
    /// [`BUSY_PLANES`] planes. An HT block's exponent bounds stay within its
    /// zero bit-planes plus 2, which noise keeps to only when that is large.
    fn busy_zero_planes(coder: Coder) -> u32 {
        match coder {
            Coder::Part1 => 0,
            Coder::Ht => BUSY_PLANES - 2,
        }
    }

    /// The magnitude bit-planes `Mb` of the stale-state test bands.
    const BUSY_PLANES: u32 = 28;

    /// A `width × height` code-block at `(x, y)` of `coder`, all its passes
    /// in one segment of `data`: every pass of its bit-planes for a Part 1
    /// block, the cleanup pass for an HT one.
    fn shaped_block(
        (x, y): (usize, usize),
        (width, height): (usize, usize),
        coder: Coder,
        data: &[u8],
    ) -> CodeBlock<'_> {
        let passes = match coder {
            Coder::Part1 => 3 * BUSY_PLANES - 2,
            Coder::Ht => 1,
        };
        CodeBlock {
            x,
            y,
            width,
            height,
            num_passes: passes,
            zero_bit_planes: busy_zero_planes(coder),
            segments: [(passes, [data])].into_iter().collect(),
            cut: false,
            placeholders: 0,
            coder,
            missing_from: None,
        }
    }

    /// Coded bytes for a `shape` block of `coder` that decode, alone, with
    /// many samples significant, so that the block's neighbor bits and the
    /// padding around it are set: the first such noise. For a Part 1 block
    /// that is over a quarter of them. Noise seldom gets far as HT data
    /// before it breaks a rule of the cleanup pass, so for an HT block it is
    /// as many as the shorter side. An HT cleanup segment ends in its suffix
    /// length `Scup`, here 40.
    fn busy_data(coders: Coders, coder: Coder, (w, h): (usize, usize)) -> Vec<u8> {
        (1..=10_000)
            .map(|seed| match coder {
                Coder::Part1 => noise(seed, 2 * w * h + 64),
                Coder::Ht => {
                    let mut bytes = noise(seed, 4 * w * h + 64);
                    let last = bytes.len() - 1;
                    bytes[last - 1] = (bytes[last - 1] & 0xF0) | 8;
                    bytes[last] = 2;
                    bytes
                }
            })
            .find(|data| {
                let block = shaped_block((0, 0), (w, h), coder, data);
                coders
                    .decode(
                        &block,
                        BandKind::Hl,
                        1,
                        BUSY_PLANES,
                        &mut Scratch::default(),
                    )
                    .is_ok_and(|c| {
                        let significant = c.iter().filter(|&&c| c != 0).count();
                        match coder {
                            Coder::Part1 => 4 * significant > c.len(),
                            Coder::Ht => significant >= w.min(h),
                        }
                    })
            })
            .expect("noise that codes enough")
    }

    /// The shapes the stale-state tests decode in sequence: narrow then
    /// wider, wide then narrow, full then a partial edge block, tall then
    /// short, and a block whose last stripe is short after one whose last
    /// stripe is full. A narrow block comes first, since a stripe length
    /// kept from it would not hold the wider blocks after it.
    const SHAPES: [(usize, usize); 9] = [
        (4, 64),
        (64, 4),
        (32, 32),
        (32, 5),
        (5, 32),
        (64, 64),
        (3, 7),
        (64, 8),
        (64, 6),
    ];

    /// [`SHAPES`] at a size noise gets through as HT data, with the same
    /// changes from block to block, and odd widths and heights that leave
    /// the last quads of a row or column half outside the block.
    const HT_SHAPES: [(usize, usize); 9] = [
        (16, 2),
        (2, 16),
        (8, 8),
        (8, 3),
        (3, 8),
        (16, 16),
        (3, 7),
        (16, 4),
        (16, 3),
    ];

    /// Decode `shapes` in turn with one set of buffers, and check each block
    /// comes out as it does decoded alone.
    fn check_shapes_in_turn(coders: Coders, coder: Coder, shapes: &[(usize, usize)], what: &str) {
        let data: Vec<Vec<u8>> = shapes
            .iter()
            .map(|&shape| busy_data(coders, coder, shape))
            .collect();
        let mut shared = Scratch::default();
        let kinds = [BandKind::Hl, BandKind::Hh, BandKind::Lh];
        for ((&shape, data), &kind) in shapes.iter().zip(&data).zip(kinds.iter().cycle()) {
            let block = shaped_block((0, 0), shape, coder, data);
            let alone = coders
                .decode(&block, kind, 1, BUSY_PLANES, &mut Scratch::default())
                .unwrap()
                .to_vec();
            let reused = coders
                .decode(&block, kind, 1, BUSY_PLANES, &mut shared)
                .unwrap();
            assert_eq!(reused, &alone[..], "the {shape:?} block, {what}");
        }
    }

    /// One set of block buffers serves blocks of any shape in turn: each
    /// block, decoded after blocks of other widths and heights, comes out as
    /// it does decoded alone. A neighbor bit or a significance flag left
    /// over from the block before, in the block or its padding, or a stripe
    /// length kept from a block of another width, changes the coding
    /// contexts and so the coefficients.
    #[test]
    fn reused_buffers_decode_each_part1_block_as_alone() {
        use crate::codestream::markers::code_block_style::{RESET, VCAUSAL};
        for style in [0, VCAUSAL, RESET] {
            let coder = BlockCoder::Ebcot {
                style,
                roi_shift: 0,
            };
            let coders = Coders {
                part1: coder,
                ht: coder,
                mixed: false,
            };
            check_shapes_in_turn(
                coders,
                Coder::Part1,
                &SHAPES,
                &format!("style {style:#04x}"),
            );
        }
    }

    /// As [`reused_buffers_decode_each_part1_block_as_alone`], for the HT
    /// coder: its cleanup buffers carry nothing from one block to the next.
    #[test]
    fn reused_buffers_decode_each_ht_block_as_alone() {
        let coder = BlockCoder::Ht {
            roi_shift: 0,
            causal: false,
            bound: CleanupBound {
                magb: None,
                irreversible: false,
            },
        };
        let coders = Coders {
            part1: coder,
            ht: coder,
            mixed: false,
        };
        check_shapes_in_turn(coders, Coder::Ht, &HT_SHAPES, "HT");
    }

    /// An HT block of `shape` with a cleanup segment and a refinement
    /// segment that carries its SigProp and MagRef passes.
    fn refined_block<'a>(
        shape: (usize, usize),
        cleanup: &'a [u8],
        refinement: &'a [u8],
    ) -> CodeBlock<'a> {
        let mut block = shaped_block((0, 0), shape, Coder::Ht, cleanup);
        block.num_passes = 3;
        block.segments.push(2, &[refinement]);
        block
    }

    /// As [`reused_buffers_decode_each_ht_block_as_alone`], with refinement
    /// passes: each block's SigProp pass makes samples significant and its
    /// MagRef pass changes magnitudes, so a SigProp flag left over from the
    /// block before changes the coefficients.
    #[test]
    fn reused_buffers_decode_each_refined_ht_block_as_alone() {
        let coder = BlockCoder::Ht {
            roi_shift: 0,
            causal: false,
            bound: CleanupBound {
                magb: None,
                irreversible: false,
            },
        };
        let coders = Coders {
            part1: coder,
            ht: coder,
            mixed: false,
        };
        fn decode<'a>(
            coders: Coders,
            block: &CodeBlock<'a>,
            scratch: &mut Scratch<'a>,
        ) -> Result<Vec<i32>> {
            coders
                .decode(block, BandKind::Hl, 1, BUSY_PLANES, scratch)
                .map(<[i32]>::to_vec)
        }
        let data: Vec<(Vec<u8>, Vec<u8>)> = HT_SHAPES
            .iter()
            .map(|&shape| {
                let cleanup = busy_data(coders, Coder::Ht, shape);
                let base = decode(
                    coders,
                    &shaped_block((0, 0), shape, Coder::Ht, &cleanup),
                    &mut Scratch::default(),
                )
                .unwrap();
                let refinement = (1..=10_000)
                    .map(|seed| noise(seed, shape.0 * shape.1 + 16))
                    .find(|refinement| {
                        let block = refined_block(shape, &cleanup, refinement);
                        decode(coders, &block, &mut Scratch::default()).is_ok_and(|c| {
                            let pairs = || base.iter().zip(&c);
                            let sigprop = pairs().filter(|&(&b, &r)| b == 0 && r != 0).count();
                            let magref = pairs().any(|(&b, &r)| b != 0 && b != r);
                            sigprop >= 2 && magref
                        })
                    })
                    .expect("refinement noise that changes the block");
                (cleanup, refinement)
            })
            .collect();
        let mut shared = Scratch::default();
        for (&shape, (cleanup, refinement)) in HT_SHAPES.iter().zip(&data) {
            let block = refined_block(shape, cleanup, refinement);
            let alone = decode(coders, &block, &mut Scratch::default()).unwrap();
            let reused = decode(coders, &block, &mut shared).unwrap();
            assert_eq!(reused, alone, "the refined {shape:?} block");
        }
    }

    /// The same through a subband: a 36x37 band of 32x32 code-blocks holds a
    /// full block, then partial edge blocks of 4 columns, of 5 rows, and of
    /// both, and comes out as its blocks decoded alone and placed.
    #[test]
    fn reused_buffers_decode_a_band_of_edge_blocks_as_its_blocks_alone() {
        let coders = default_params();
        let layout = [
            ((0, 0), (32, 32)),
            ((32, 0), (4, 32)),
            ((0, 32), (32, 5)),
            ((32, 32), (4, 5)),
        ];
        let data: Vec<Vec<u8>> = layout
            .iter()
            .map(|&(_, shape)| busy_data(coders, Coder::Part1, shape))
            .collect();
        let sb = Subband {
            kind: BandKind::Hl,
            origin: (0, 0),
            width: 36,
            height: 37,
            block_cols: 2,
            block_rows: 2,
            blocks: layout
                .iter()
                .zip(&data)
                .map(|(&(at, shape), data)| shaped_block(at, shape, Coder::Part1, data))
                .collect(),
        };
        let band =
            decode_subband::<i32, _>(&sb, 1, BUSY_PLANES, coders, &mut Scratch::default(), |q| q)
                .unwrap();
        let mut expected = vec![0; 36 * 37];
        for block in &sb.blocks {
            let alone = coders
                .decode(block, sb.kind, 1, BUSY_PLANES, &mut Scratch::default())
                .unwrap()
                .to_vec();
            place_block(&mut expected, 36, block, &alone, &|q| q);
        }
        assert_eq!(band.data, expected);
    }

    fn default_params() -> Coders {
        part1_coders(0)
    }

    /// The coders of a Part 1 tile-component of default style with maxshift
    /// `roi_shift`. The test blocks are Part 1 blocks, so the HT coder is
    /// never asked for.
    fn part1_coders(roi_shift: u8) -> Coders {
        let coder = BlockCoder::Ebcot {
            style: 0,
            roi_shift,
        };
        Coders {
            part1: coder,
            ht: coder,
            mixed: false,
        }
    }
}

#[cfg(test)]
mod work_bound_tests;
