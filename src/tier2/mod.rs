//! Stage 2 — Tier-2 packet decoding (ISO/IEC 15444-1 Annex B).
//!
//! Tile-part data is a sequence of *packets*, one per (resolution, layer,
//! component, precinct) in the header's progression order. Each packet header
//! says, via [tag trees](tagtree), which code-blocks are included, how many
//! bit-planes are all-zero, how many coding passes each contributes, and the
//! byte length of each contribution. This stage parses that structure and
//! hands Tier-1 the coded byte segments per code-block — it does **not** run
//! the arithmetic decoder.
//!
//! The decoded subset is one tile, one component, one quality layer, LRCP, and
//! maximal precincts (one precinct per resolution). That collapses the packet
//! stream to exactly one packet per resolution level, coarsest first, so the
//! whole tile-part is `header₀ body₀ header₁ body₁ …` with no precinct or layer
//! nesting. `decode_packets` also computes the resolution / subband /
//! code-block geometry from the [`MainHeader`](crate::codestream::MainHeader),
//! so it is the single source of truth the assembly stage reuses.

pub mod bio;
pub mod tagtree;

use crate::codestream::{Codestream, MainHeader};
use crate::{Error, Result};
use bio::BitReader;
use tagtree::TagTree;

/// The four subband orientations. Kept Tier-2-local so this stage stays
/// independent of Tier-1; the assembly stage maps it to the Tier-1
/// `Orientation` that selects the zero-coding context table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    /// Low-pass both axes (the coarsest resolution only).
    Ll,
    /// High-pass horizontally, low-pass vertically.
    Hl,
    /// Low-pass horizontally, high-pass vertically.
    Lh,
    /// High-pass both axes.
    Hh,
}

/// One code-block's coded contribution within its subband. `num_passes` is the
/// coding-pass count from the packet header (0 if the block is never included
/// in the single layer) and `zero_bit_planes` the all-zero most-significant
/// bit-plane count from the zero-bitplane tag tree. `segment` is the raw
/// MQ-coded byte slice Tier-1 decodes (empty when the block is not included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock<'a> {
    /// Top-left of the block within its subband, in band-relative samples.
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub num_passes: u32,
    pub zero_bit_planes: u32,
    pub segment: &'a [u8],
}

/// One subband: its orientation, tile-component origin, sample geometry, and the
/// code-block grid carrying the coded segments. Blocks are row-major, so block
/// `(i, j)` is `blocks[j * block_cols + i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subband<'a> {
    pub kind: BandKind,
    /// Tile-component coordinate of the band's top-left sample.
    pub origin: (u32, u32),
    pub width: usize,
    pub height: usize,
    pub block_cols: usize,
    pub block_rows: usize,
    pub blocks: Vec<CodeBlock<'a>>,
}

/// One resolution level's subbands, in packet order: `[Ll]` at the coarsest
/// level, `[Hl, Lh, Hh]` at every finer level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<'a> {
    pub subbands: Vec<Subband<'a>>,
}

/// The coded byte segments for every code-block in every subband, grouped by
/// resolution (coarsest first, index 0 the `NLLL` band) so Tier-1 can decode
/// each block independently and the assembly stage can place it back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodedData<'a> {
    pub resolutions: Vec<Resolution<'a>>,
}

/// Parse all packets in the codestream's single tile-part into per-code-block
/// coded segments, following the LRCP order over the one tile / component /
/// layer with maximal precincts: one packet per resolution, coarsest first.
pub fn decode_packets<'a>(cs: &Codestream<'a>) -> Result<CodedData<'a>> {
    let tile = cs
        .tile_parts
        .first()
        .ok_or_else(|| Error::Codestream("codestream carries no tile-part".into()))?;
    let data = tile.data;

    let geoms = resolution_geoms(&cs.header)?;
    let mut cursor = 0usize;
    let mut resolutions = Vec::with_capacity(geoms.len());
    for bands in &geoms {
        let (subbands, next) = parse_packet(data, cursor, bands)?;
        cursor = next;
        resolutions.push(Resolution { subbands });
    }

    // Single-layer LRCP with maximal precincts packs the tile-part with no
    // padding: the packets must tile it exactly up to the closing EOC. Any
    // remainder means a misread field (this doubles as the parse self-check).
    if cursor != data.len() {
        return Err(Error::Codestream(format!(
            "tile-part has {} byte(s) left after the last packet",
            data.len() - cursor
        )));
    }

    Ok(CodedData { resolutions })
}

/// Geometry of one subband before its segments are parsed: orientation, origin,
/// sample dimensions, and the code-block grid (each block's band-relative
/// position and size).
struct BandGeom {
    kind: BandKind,
    origin: (u32, u32),
    width: usize,
    height: usize,
    block_cols: usize,
    block_rows: usize,
    /// `(x, y, width, height)` per block, row-major.
    blocks: Vec<(usize, usize, usize, usize)>,
}

/// `ceil(a / b)` for any integers with `b > 0` (Rust's `/` truncates toward
/// zero, so the subband formula's negative numerators need this floor-based
/// form).
fn ceil_div(a: i64, b: i64) -> i64 {
    debug_assert!(b > 0, "ceil_div needs a positive divisor");
    let q = a.div_euclid(b);
    if a.rem_euclid(b) != 0 { q + 1 } else { q }
}

/// Compute the resolution → subband → code-block geometry for the single
/// tile-component, coarsest resolution first (ISO B.5–B.7, Eq. B-15). Maximal
/// precincts mean one precinct per resolution, so the code-block grid tiles each
/// whole subband.
fn resolution_geoms(header: &MainHeader) -> Result<Vec<Vec<BandGeom>>> {
    let siz = &header.siz;
    let cod = &header.cod;

    let nl = cod.decomposition_levels as i64;
    if nl > 32 {
        return Err(Error::Unsupported(format!(
            "{nl} decomposition levels exceeds the 32-level maximum"
        )));
    }

    // Tile-component bounds: the single tile clipped to the image, divided by
    // the component sub-sampling (Annex B.3, Eq. B-7/B-12). The decoded subset
    // uses zero offsets and unit sub-sampling, but the general form costs
    // nothing — provided the tile origin is clamped up to the image offset.
    let comp = siz
        .components
        .first()
        .ok_or_else(|| Error::Codestream("SIZ declares no components".into()))?;
    let xr = (comp.x_sampling.max(1)) as i64;
    let yr = (comp.y_sampling.max(1)) as i64;
    let tx0 = (siz.tile_x_offset as i64).max(siz.x_offset as i64);
    let ty0 = (siz.tile_y_offset as i64).max(siz.y_offset as i64);
    let tx1 = (siz.tile_x_offset as i64 + siz.tile_width as i64).min(siz.x_size as i64);
    let ty1 = (siz.tile_y_offset as i64 + siz.tile_height as i64).min(siz.y_size as i64);
    if tx1 <= tx0 || ty1 <= ty0 {
        return Err(Error::Codestream("tile has empty area".into()));
    }
    let (tcx0, tcx1) = (ceil_div(tx0, xr), ceil_div(tx1, xr));
    let (tcy0, tcy1) = (ceil_div(ty0, yr), ceil_div(ty1, yr));

    // Code-block exponents (COD stores `log2(size) - 2`). The standard bounds
    // each at 2^10 and their sum at 2^12 (ISO Table A-18); reject anything
    // larger so the grid shifts below stay well-defined and a malformed COD is
    // a typed error, not a silently clamped mis-decode.
    let xcb = cod.code_block_width as u32 + 2;
    let ycb = cod.code_block_height as u32 + 2;
    if xcb > 10 || ycb > 10 || xcb + ycb > 12 {
        return Err(Error::Marker(format!(
            "code-block size 2^{xcb}×2^{ycb} exceeds the 2^10 / xcb+ycb≤12 limit"
        )));
    }

    // With maximal precincts (PPx = PPy = 15), the precinct never shrinks the
    // block at level 0 and caps it one below the precinct at finer levels
    // (ISO B.6); for the subset's 2^6 blocks neither cap bites.

    let mut levels = Vec::with_capacity((nl + 1) as usize);
    for r in 0..=nl {
        let bands = if r == 0 {
            // The coarsest resolution carries only the NLLL band.
            let pow = 1i64 << nl;
            vec![band_geom(
                BandKind::Ll,
                ceil_div(tcx0, pow),
                ceil_div(tcx1, pow),
                ceil_div(tcy0, pow),
                ceil_div(tcy1, pow),
                xcb.min(15),
                ycb.min(15),
            )]
        } else {
            // Finer levels add HL, LH, HH at decomposition level `nb = NL-r+1`.
            let nb = nl - r + 1;
            let pow = 1i64 << nb;
            let half = 1i64 << (nb - 1);
            [
                (BandKind::Hl, 1, 0),
                (BandKind::Lh, 0, 1),
                (BandKind::Hh, 1, 1),
            ]
            .into_iter()
            .map(|(kind, xob, yob)| {
                band_geom(
                    kind,
                    ceil_div(tcx0 - xob * half, pow),
                    ceil_div(tcx1 - xob * half, pow),
                    ceil_div(tcy0 - yob * half, pow),
                    ceil_div(tcy1 - yob * half, pow),
                    xcb.min(14),
                    ycb.min(14),
                )
            })
            .collect()
        };
        levels.push(bands);
    }
    Ok(levels)
}

/// Build one subband's geometry from its sample bounds `[bx0, bx1) × [by0, by1)`
/// and the effective code-block exponents, tiling it with the code-block grid
/// anchored at the canvas origin (ISO B.7).
fn band_geom(
    kind: BandKind,
    bx0: i64,
    bx1: i64,
    by0: i64,
    by1: i64,
    xcb: u32,
    ycb: u32,
) -> BandGeom {
    let width = (bx1 - bx0).max(0) as usize;
    let height = (by1 - by0).max(0) as usize;
    let cbw = 1i64 << xcb;
    let cbh = 1i64 << ycb;

    let (block_cols, first_col) = grid_span(bx0, bx1, cbw);
    let (block_rows, first_row) = grid_span(by0, by1, cbh);

    let mut blocks = Vec::with_capacity(block_cols * block_rows);
    for j in 0..block_rows {
        let gy0 = (first_row + j as i64) * cbh;
        let cy0 = gy0.max(by0);
        let cy1 = (gy0 + cbh).min(by1);
        for i in 0..block_cols {
            let gx0 = (first_col + i as i64) * cbw;
            let cx0 = gx0.max(bx0);
            let cx1 = (gx0 + cbw).min(bx1);
            blocks.push((
                (cx0 - bx0) as usize,
                (cy0 - by0) as usize,
                (cx1 - cx0) as usize,
                (cy1 - cy0) as usize,
            ));
        }
    }

    BandGeom {
        kind,
        origin: (bx0.max(0) as u32, by0.max(0) as u32),
        width,
        height,
        block_cols,
        block_rows,
        blocks,
    }
}

/// Number of code-block grid cells spanning `[lo, hi)` and the index of the
/// first cell, with the grid anchored at multiples of `cell` from the origin.
fn grid_span(lo: i64, hi: i64, cell: i64) -> (usize, i64) {
    if hi <= lo {
        return (0, 0);
    }
    let first = lo.div_euclid(cell);
    let last = ceil_div(hi, cell);
    ((last - first) as usize, first)
}

/// Ceiling on the zero-bitplane count: a single read above this resolves any
/// real value (the count is bounded by the magnitude bit-planes — at most the
/// sample depth plus guard bits, well under 64 for the ≤32-bit subset), while
/// rejecting a malformed run of zero bits rather than looping on it.
const ZBP_LIMIT: u32 = 64;

/// Upper bound on the Lblock length-indicator before a malformed packet is
/// rejected. The length field is `Lblock + floor(log2(num_passes))` bits and
/// `num_passes ≤ 164` (so `floor(log2) ≤ 7`); capping Lblock at 24 keeps that
/// read at most 31 bits, inside the `u32` [`BitReader::read`] accepts.
const LBLOCK_MAX: u32 = 24;

/// Per-block metadata read from a packet header, before body bytes are sliced.
struct BlockMeta {
    num_passes: u32,
    zero_bit_planes: u32,
    seg_len: usize,
}

/// Parse one packet (the single precinct of one resolution) starting at byte
/// `start` of the tile-part `data`. Returns the resolution's subbands with their
/// segments and the byte offset where the next packet begins.
fn parse_packet<'a>(
    data: &'a [u8],
    start: usize,
    bands: &[BandGeom],
) -> Result<(Vec<Subband<'a>>, usize)> {
    let mut bio = BitReader::new(&data[start..]);

    // The first bit flags an empty packet (no contributions) vs. a present one.
    let present = bio.read_bit() == 1;
    let mut metas: Vec<Vec<BlockMeta>> = Vec::with_capacity(bands.len());
    for band in bands {
        if !present {
            metas.push(band.blocks.iter().map(|_| BlockMeta::absent()).collect());
            continue;
        }
        metas.push(parse_band_header(band, &mut bio)?);
    }

    // The header is a whole number of bytes; the body follows immediately.
    bio.align();
    let mut body = start + bio.bytes_consumed();

    let mut subbands = Vec::with_capacity(bands.len());
    for (band, band_meta) in bands.iter().zip(&metas) {
        let mut blocks = Vec::with_capacity(band.blocks.len());
        for (&(x, y, width, height), meta) in band.blocks.iter().zip(band_meta) {
            // An included block (passes > 0) must carry coded bytes; a zero
            // length there is malformed and would hand Tier-1 an empty MQ
            // stream, so reject it here where the context is known.
            if meta.num_passes > 0 && meta.seg_len == 0 {
                return Err(Error::Codestream(
                    "included code-block has coding passes but zero length".into(),
                ));
            }
            let segment = if meta.seg_len == 0 {
                &[][..]
            } else {
                let end = body
                    .checked_add(meta.seg_len)
                    .filter(|&e| e <= data.len())
                    .ok_or_else(|| {
                        Error::Codestream("packet body segment overruns the tile-part".into())
                    })?;
                let slice = &data[body..end];
                body = end;
                slice
            };
            blocks.push(CodeBlock {
                x,
                y,
                width,
                height,
                num_passes: meta.num_passes,
                zero_bit_planes: meta.zero_bit_planes,
                segment,
            });
        }
        subbands.push(Subband {
            kind: band.kind,
            origin: band.origin,
            width: band.width,
            height: band.height,
            block_cols: band.block_cols,
            block_rows: band.block_rows,
            blocks,
        });
    }

    Ok((subbands, body))
}

impl BlockMeta {
    /// A block that contributes nothing to this packet.
    fn absent() -> Self {
        BlockMeta {
            num_passes: 0,
            zero_bit_planes: 0,
            seg_len: 0,
        }
    }
}

/// Read one subband's code-block entries from the packet header: per block, its
/// inclusion (inclusion tag tree at layer 0), and for an included block the
/// zero-bitplane count, coding-pass count, and contribution length.
fn parse_band_header(band: &BandGeom, bio: &mut BitReader) -> Result<Vec<BlockMeta>> {
    let cols = band.block_cols as u32;
    let rows = band.block_rows as u32;
    let mut inclusion = TagTree::new(cols, rows);
    let mut zero_bits = TagTree::new(cols, rows);

    let mut metas = Vec::with_capacity(band.blocks.len());
    for idx in 0..band.blocks.len() {
        let bx = idx as u32 % cols;
        let by = idx as u32 / cols;

        // Single layer: a block is included now iff its inclusion value is 0
        // (it never appears in a later layer), so read at threshold 1.
        if inclusion.read(bx, by, 1, bio).is_none() {
            metas.push(BlockMeta::absent());
            continue;
        }

        // Zero bit-planes: unlike inclusion (read once per layer at a rising
        // threshold), the zero-bitplane tree is resolved in full the first time
        // a block is included, so a single read at the ceiling resolves it.
        let zero_bit_planes = zero_bits
            .read(bx, by, ZBP_LIMIT, bio)
            .ok_or_else(|| Error::Codestream("zero-bitplane count exceeds the limit".into()))?;

        // `read_num_passes` always returns ≥ 1, so the `ilog2` below never hits
        // the zero case.
        let num_passes = read_num_passes(bio);

        // Lblock grows by a unary run of 1s; the length field is then
        // `Lblock + floor(log2(num_passes))` bits wide (ISO B.10.7.5).
        let mut lblock = 3u32;
        while bio.read_bit() == 1 {
            lblock += 1;
            if lblock > LBLOCK_MAX {
                return Err(Error::Codestream("Lblock indicator runs too long".into()));
            }
        }
        let length_bits = lblock + num_passes.ilog2();
        let seg_len = bio.read(length_bits) as usize;

        metas.push(BlockMeta {
            num_passes,
            zero_bit_planes,
            seg_len,
        });
    }
    Ok(metas)
}

/// Decode the number of coding passes (ISO Table B.4 / OpenJPEG
/// `opj_t2_getnumpasses`): a prefix code spanning 1 to 164 passes.
fn read_num_passes(bio: &mut BitReader) -> u32 {
    if bio.read_bit() == 0 {
        return 1;
    }
    if bio.read_bit() == 0 {
        return 2;
    }
    let n = bio.read(2);
    if n != 3 {
        return 3 + n;
    }
    let n = bio.read(5);
    if n != 31 {
        return 6 + n;
    }
    37 + bio.read(7)
}

#[cfg(test)]
mod tests;
