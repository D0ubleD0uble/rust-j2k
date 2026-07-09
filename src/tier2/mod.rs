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
//! The decoded subset is one tile with maximal precincts, so the position axis
//! has one value per resolution and the packet stream walks the remaining three
//! axes — layer, resolution, component — in whichever order `COD` names. The
//! tile-part is then `header₀ body₀ header₁ body₁ …` with no padding, and the
//! packets must tile it exactly; a leftover byte means a misread field.
//!
//! A code-block's contributions accumulate across the layers that include it,
//! so a precinct's tag trees and per-block state outlive any one packet. See
//! [`for_each_packet`] for the orders and [`BlockState`] for what persists.
//!
//! `decode_packets` also computes the resolution / subband / code-block geometry
//! from the [`MainHeader`](crate::codestream::MainHeader), so it is the single
//! source of truth the assembly stage reuses.

pub mod bio;
pub mod tagtree;

use crate::codestream::markers::{Progression, marker};
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
    /// Coding passes accumulated over every layer that contributed.
    pub num_passes: u32,
    pub zero_bit_planes: u32,
    /// The block's codeword segments, in order. Tier-1 restarts the MQ decoder
    /// at each one and carries its bit-plane state across them.
    pub segments: Vec<CodedSegment<'a>>,
}

/// One codeword segment: the coding passes it carries and the bytes coding
/// them, which a multi-layer codestream may deliver in several chunks.
///
/// Without per-pass termination a block has exactly one segment, and the MQ
/// codeword runs continuously across every layer that contributed to it. Under
/// `restart` (`termall`) each coding pass is terminated separately, so each is
/// its own segment and Tier-1 must re-initialise the MQ decoder for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedSegment<'a> {
    pub passes: u32,
    pub chunks: Vec<&'a [u8]>,
}

impl CodedSegment<'_> {
    /// The segment's bytes, concatenated across the layers that carried them.
    pub fn bytes(&self) -> Vec<u8> {
        self.chunks.concat()
    }
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

/// One component's coded data, grouped by resolution (coarsest first, index 0
/// the `NLLL` band) so Tier-1 can decode each block independently and the
/// assembly stage can place it back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentCoded<'a> {
    pub resolutions: Vec<Resolution<'a>>,
}

/// The coded byte segments for every code-block of every component, in SIZ
/// component order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodedData<'a> {
    pub components: Vec<ComponentCoded<'a>>,
}

/// Parse all packets in the codestream's single tile-part into per-code-block
/// coded segments.
///
/// LRCP orders packets layer, then resolution, then component, then precinct
/// (ISO B.12.1.1). With one layer and maximal precincts that reduces to a
/// resolution-major sweep with the components nested inside it: `r0c0, r0c1, …,
/// r1c0, r1c1, …`. Each component carries its own tile-component geometry, so a
/// sub-sampled component's subbands are smaller at the same resolution.
pub fn decode_packets<'a>(cs: &Codestream<'a>) -> Result<CodedData<'a>> {
    let tile = cs
        .tile_parts
        .first()
        .ok_or_else(|| Error::Codestream("codestream carries no tile-part".into()))?;
    let data = tile.data;

    let component_count = cs.header.siz.components.len();
    let geoms = (0..component_count)
        .map(|c| resolution_geoms(&cs.header, c))
        .collect::<Result<Vec<_>>>()?;
    // COD is a main-header default, so every component decomposes to the same
    // number of resolutions (per-component overrides are COC, not yet decoded).
    // The packet loop below indexes `geoms[c][r]`, so state that rather than
    // trust it: once COC lands, a component with its own decomposition depth
    // must skip the resolutions it does not have, not index past its list.
    let resolution_count = geoms.first().map_or(0, Vec::len);
    if geoms.iter().any(|g| g.len() != resolution_count) {
        return Err(Error::Inconsistent(
            "components disagree on their resolution count".into(),
        ));
    }

    // One state per (component, resolution, band), carried across every layer of
    // the precinct. The tag trees decode incrementally, so they must outlive the
    // packet that starts them.
    let mut states: Vec<Vec<Vec<BandState<'a>>>> = geoms
        .iter()
        .map(|component| {
            component
                .iter()
                .map(|bands| bands.iter().map(BandState::new).collect())
                .collect()
        })
        .collect();

    let delimiters = Delimiters {
        sop: cs.header.cod.use_sop,
        eph: cs.header.cod.use_eph,
    };
    let mut cursor = 0usize;
    let mut packet_index = 0u32;
    for_each_packet(
        cs.header.cod.progression,
        cs.header.cod.layers as u32,
        resolution_count,
        component_count,
        |layer, resolution, component| {
            cursor = parse_packet(
                data,
                cursor,
                layer,
                packet_index,
                delimiters,
                cs.header.cod.code_block_style,
                &geoms[component][resolution],
                &mut states[component][resolution],
            )?;
            packet_index += 1;
            Ok(())
        },
    )?;

    let components: Vec<ComponentCoded<'a>> = states
        .into_iter()
        .zip(&geoms)
        .map(|(component_states, component_geoms)| {
            Ok(ComponentCoded {
                resolutions: component_states
                    .into_iter()
                    .zip(component_geoms)
                    .map(|(band_states, bands)| {
                        Ok(Resolution {
                            subbands: build_subbands(bands, band_states)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // The packets tile the tile-part exactly, with no padding, up to the closing
    // EOC. Any remainder means a misread field — a dropped layer shows up here
    // as leftover bytes (this doubles as the parse self-check).
    if cursor != data.len() {
        return Err(Error::Codestream(format!(
            "tile-part has {} byte(s) left after the last packet",
            data.len() - cursor
        )));
    }

    Ok(CodedData { components })
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

/// Visit every packet of the tile in the order `progression` prescribes
/// (ISO/IEC 15444-1 B.12.1), calling `f(layer, resolution, component)` for each.
///
/// The standard nests four axes — layer, resolution, component, position — and
/// each order is a permutation of them. The decoded subset has maximal
/// precincts, so the *position* axis has exactly one value per resolution and
/// drops out, leaving three:
///
/// ```text
/// order   standard nesting   with one precinct
/// LRCP    l → r → c → p      l → r → c
/// RLCP    r → l → c → p      r → l → c
/// RPCL    r → p → c → l      r → c → l
/// PCRL    p → c → r → l      c → r → l
/// CPRL    c → p → r → l      c → r → l
/// ```
///
/// PCRL and CPRL therefore enumerate the *same* sequence here. That is not an
/// approximation — with one precinct the orders genuinely coincide — but it does
/// mean no test in this crate can tell them apart until the precinct partition
/// lands (issue #61). The same caveat applies to any codestream with one layer
/// and one component, where all five orders coincide; see `docs/correctness.md`
/// §A passing entry is not proof the feature works.
fn for_each_packet<F>(
    progression: Progression,
    layers: u32,
    resolutions: usize,
    components: usize,
    mut f: F,
) -> Result<()>
where
    F: FnMut(u32, usize, usize) -> Result<()>,
{
    match progression {
        Progression::Lrcp => {
            for layer in 0..layers {
                for resolution in 0..resolutions {
                    for component in 0..components {
                        f(layer, resolution, component)?;
                    }
                }
            }
        }
        Progression::Rlcp => {
            for resolution in 0..resolutions {
                for layer in 0..layers {
                    for component in 0..components {
                        f(layer, resolution, component)?;
                    }
                }
            }
        }
        Progression::Rpcl => {
            for resolution in 0..resolutions {
                for component in 0..components {
                    for layer in 0..layers {
                        f(layer, resolution, component)?;
                    }
                }
            }
        }
        // With one precinct these two are the same walk: PCRL's position loop
        // and CPRL's both degenerate, leaving component outside resolution.
        Progression::Pcrl | Progression::Cprl => {
            for component in 0..components {
                for resolution in 0..resolutions {
                    for layer in 0..layers {
                        f(layer, resolution, component)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// `ceil(a / b)` for any integers with `b > 0` (Rust's `/` truncates toward
/// zero, so the subband formula's negative numerators need this floor-based
/// form).
fn ceil_div(a: i64, b: i64) -> i64 {
    debug_assert!(b > 0, "ceil_div needs a positive divisor");
    let q = a.div_euclid(b);
    if a.rem_euclid(b) != 0 { q + 1 } else { q }
}

/// Compute the resolution → subband → code-block geometry for tile-component
/// `comp`, coarsest resolution first (ISO B.5–B.7, Eq. B-15). Maximal precincts
/// mean one precinct per resolution, so the code-block grid tiles each whole
/// subband.
///
/// The tile-component bounds divide by *this component's* sub-sampling, so two
/// components of the same image can yield different subband sizes.
fn resolution_geoms(header: &MainHeader, comp: usize) -> Result<Vec<Vec<BandGeom>>> {
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
    let component = siz
        .components
        .get(comp)
        .ok_or_else(|| Error::Codestream(format!("SIZ declares no component {comp}")))?;
    let xr = (component.x_sampling.max(1)) as i64;
    let yr = (component.y_sampling.max(1)) as i64;
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

/// How many coding passes one codeword segment may hold, given the code-block
/// style (`opj_t2_init_seg`).
///
/// `termall` terminates the MQ coder after every pass, so each pass is its own
/// segment. Otherwise a segment takes up to 109 passes — `3 * Mb - 2` with `Mb`
/// capped at 37 — which a code-block can never exceed, so the default never
/// splits.
fn segment_max_passes(style: u8) -> u32 {
    if style & crate::codestream::markers::code_block_style::TERMALL != 0 {
        1
    } else {
        109
    }
}

/// Which packet delimiters `COD` signals (`Scod` bits 1 and 2).
#[derive(Debug, Clone, Copy)]
struct Delimiters {
    /// SOP *may* precede each packet.
    sop: bool,
    /// EPH *shall* follow each packet header.
    eph: bool,
}

/// The two-byte marker at `pos`, or `None` if it would run past the end.
fn peek_marker(data: &[u8], pos: usize) -> Option<u16> {
    let hi = *data.get(pos)?;
    let lo = *data.get(pos + 1)?;
    Some(u16::from_be_bytes([hi, lo]))
}

/// Consume the SOP marker segment at `pos` and return the offset of the packet
/// header that follows (ISO A.8.1).
///
/// `Lsop` is fixed at 4 and `Nsop` counts packets from zero **within the tile**,
/// continuing across that tile's tile-parts: OpenJPEG's encoder writes
/// `tile->packno`, which lives on the tile and is never reset per tile-part. The
/// corpus agrees — every single-tile entry that carries SOP numbers its packets
/// sequentially from zero, every multi-tile one restarts.
///
/// The decoded subset is one tile in one tile-part, so the packet index is the
/// count. When tiles and tile-parts land (issues #59, #60) the counter must
/// reset per *tile*, not per tile-part.
///
/// OpenJPEG leaves `Nsop` unchecked — `/* TODO : check the Nsop value */` — so
/// this is stricter than the oracle. Validating the sequence number is the whole
/// point of a resynchronisation marker.
fn read_sop(data: &[u8], pos: usize, packet_index: u32) -> Result<usize> {
    let end = pos + 6;
    if end > data.len() {
        return Err(Error::Codestream("truncated SOP marker segment".into()));
    }
    let lsop = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
    if lsop != 4 {
        return Err(Error::Codestream(format!(
            "SOP declares Lsop {lsop}, expected 4"
        )));
    }
    let nsop = u16::from_be_bytes([data[pos + 4], data[pos + 5]]);
    // Nsop is 16 bits and wraps; a tile with more than 65536 packets restarts it.
    let expected = (packet_index % 65536) as u16;
    if nsop != expected {
        return Err(Error::Codestream(format!(
            "SOP sequence number {nsop} for packet {packet_index}, expected {expected}"
        )));
    }
    Ok(end)
}

/// One code-block's decode state, carried across the quality layers of its
/// precinct (ISO/IEC 15444-1 B.10).
///
/// A block is included in the packets of one layer onward, never in isolation:
/// once the inclusion tag tree resolves it, later layers signal a contribution
/// with a single bit. `lblock` and `zero_bit_planes` are read once and then
/// persist, and `num_passes` and `segments` accumulate. Rebuilding any of this
/// per packet — which a single-layer decoder can get away with — desynchronises
/// the header bit-reader on the second layer.
struct BlockState<'a> {
    included: bool,
    /// Length-indicator width, grown by a unary run of 1s and never reset.
    lblock: u32,
    zero_bit_planes: u32,
    /// Coding passes summed over every contributing layer.
    num_passes: u32,
    /// Codeword segments in order; the last one may still take passes.
    segments: Vec<SegmentState<'a>>,
}

/// A codeword segment while it is being filled from successive layers.
struct SegmentState<'a> {
    passes: u32,
    /// How many passes this segment may hold before the next one starts:
    /// 1 under `termall`, otherwise 109 — the most a code-block can carry
    /// (`3 * Mb - 2` with `Mb <= 37`), so the default never splits.
    /// OpenJPEG's `opj_t2_init_seg` sets exactly these.
    max_passes: u32,
    chunks: Vec<&'a [u8]>,
}

impl BlockState<'_> {
    fn new() -> Self {
        BlockState {
            included: false,
            lblock: 3,
            zero_bit_planes: 0,
            num_passes: 0,
            segments: Vec::new(),
        }
    }
}

/// One subband's decode state across the layers of its precinct: the two tag
/// trees, which decode incrementally at a rising threshold, plus a state per
/// code-block.
struct BandState<'a> {
    inclusion: TagTree,
    zero_bits: TagTree,
    blocks: Vec<BlockState<'a>>,
}

impl BandState<'_> {
    fn new(band: &BandGeom) -> Self {
        BandState {
            inclusion: TagTree::new(band.block_cols as u32, band.block_rows as u32),
            zero_bits: TagTree::new(band.block_cols as u32, band.block_rows as u32),
            blocks: band.blocks.iter().map(|_| BlockState::new()).collect(),
        }
    }
}

/// What one codeword segment takes from *this* packet: the length of the byte
/// range that follows the header, and where to file it.
struct Contribution {
    band: usize,
    block: usize,
    segment: usize,
    seg_len: usize,
}

/// Parse one packet — the single precinct of one resolution of one component in
/// one layer — starting at byte `start` of the tile-part `data`.
///
/// Folds the packet's contributions into `states` and returns the byte offset
/// where the next packet begins. The subbands are built once every layer has
/// been read; see [`build_subbands`].
#[allow(clippy::too_many_arguments)]
fn parse_packet<'a>(
    data: &'a [u8],
    start: usize,
    layer: u32,
    packet_index: u32,
    delimiters: Delimiters,
    style: u8,
    bands: &[BandGeom],
    states: &mut [BandState<'a>],
) -> Result<usize> {
    // A packet occupies at least one byte, so a start at or past the end means
    // the codestream promised more packets than it carries.
    if start >= data.len() {
        return Err(Error::Codestream(
            "tile-part ends before the last packet".into(),
        ));
    }

    // SOP precedes the packet header when COD allows it — but only *may*
    // (A.8.1), so its absence is not an error. OpenJPEG warns and carries on.
    //
    // The peek is unambiguous: a packet header can never begin `FF 91`. Its
    // first byte may well be `0xFF`, but the header's bit stuffing then forces
    // the next byte's most significant bit to zero, so that byte is at most
    // `0x7F` and never `0x91`. See [`bio`].
    let start = if delimiters.sop && peek_marker(data, start) == Some(marker::SOP) {
        read_sop(data, start, packet_index)?
    } else {
        start
    };

    let mut bio = BitReader::new(&data[start..]);

    // The first bit flags an empty packet (no contributions) against a present
    // one. An empty packet still costs its layer: the tag trees are untouched,
    // not reset.
    let present = bio.read_bit() == 1;
    let mut contributions: Vec<Contribution> = Vec::new();
    if present {
        for (band_index, (band, state)) in bands.iter().zip(states.iter_mut()).enumerate() {
            parse_band_header(
                band_index,
                band,
                state,
                layer,
                style,
                &mut bio,
                &mut contributions,
            )?;
        }
    }

    // The header is a whole number of bytes. EPH, when COD signals it, sits
    // between the header and the body — after an empty packet's header too.
    bio.align();
    let mut body = start + bio.bytes_consumed();
    if delimiters.eph {
        if peek_marker(data, body) != Some(marker::EPH) {
            return Err(Error::Codestream(
                "COD signals EPH but the packet header is not followed by one".into(),
            ));
        }
        body += 2;
    }

    for contribution in &contributions {
        let end = body
            .checked_add(contribution.seg_len)
            .filter(|&e| e <= data.len())
            .ok_or_else(|| {
                Error::Codestream("packet body segment overruns the tile-part".into())
            })?;
        states[contribution.band].blocks[contribution.block].segments[contribution.segment]
            .chunks
            .push(&data[body..end]);
        body = end;
    }

    Ok(body)
}

/// Read one subband's code-block entries from this layer's packet header
/// (ISO B.10): per block its inclusion, and for a contributing block the
/// zero-bitplane count (first inclusion only), coding-pass count, and the
/// length of its byte contribution.
#[allow(clippy::too_many_arguments)]
fn parse_band_header(
    band_index: usize,
    band: &BandGeom,
    state: &mut BandState<'_>,
    layer: u32,
    style: u8,
    bio: &mut BitReader,
    contributions: &mut Vec<Contribution>,
) -> Result<()> {
    let cols = band.block_cols as u32;

    for block_index in 0..band.blocks.len() {
        let bx = block_index as u32 % cols;
        let by = block_index as u32 / cols;

        let was_included = state.blocks[block_index].included;
        let contributes = if was_included {
            // Already included: one bit says whether this layer adds passes.
            bio.read_bit() == 1
        } else {
            // Not yet included: the inclusion tree resolves to the first layer
            // that carries this block. Reading at `layer + 1` asks "is that
            // layer at most this one?" and leaves the tree part-decoded when it
            // is not, so the next layer resumes where this one stopped.
            match state.inclusion.read(bx, by, layer + 1, bio) {
                Some(_) => {
                    state.blocks[block_index].included = true;
                    true
                }
                None => false,
            }
        };
        if !contributes {
            continue;
        }

        // The zero-bitplane tree is resolved in full the first time a block is
        // included, so a single read at the ceiling settles it. Later layers
        // never read it again.
        if !was_included {
            state.blocks[block_index].zero_bit_planes = state
                .zero_bits
                .read(bx, by, ZBP_LIMIT, bio)
                .ok_or_else(|| Error::Codestream("zero-bitplane count exceeds the limit".into()))?;
        }

        // `read_num_passes` always returns >= 1, so the `ilog2` below never hits
        // the zero case.
        let num_passes = read_num_passes(bio);

        // Lblock grows by a unary run of 1s and carries into later layers; the
        // length field is then `Lblock + floor(log2(num_passes))` bits wide
        // (ISO B.10.7.5), over *this* contribution's passes, not the running
        // total. OpenJPEG spells the same thing `numlenbits +
        // floorlog2(seg->numnewpasses)`.
        //
        // One length field, because one codeword segment. A contribution splits
        // into several segments only under a code-block style that terminates
        // (`termall`, `bypass`), which `decode_cod` rejects; there OpenJPEG's
        // `do { ... } while (n > 0)` reads a length per segment.
        let block = &mut state.blocks[block_index];
        while bio.read_bit() == 1 {
            block.lblock += 1;
            if block.lblock > LBLOCK_MAX {
                return Err(Error::Codestream("Lblock indicator runs too long".into()));
            }
        }
        // The passes are handed to codeword segments in turn, each with its own
        // length field. Without termination one segment swallows them all;
        // under `restart` each pass terminates, so each gets a segment and a
        // length of its own. OpenJPEG spells this `do { ... } while (n > 0)`.
        block.num_passes += num_passes;
        let mut remaining = num_passes;
        loop {
            if block
                .segments
                .last()
                .is_none_or(|s| s.passes >= s.max_passes)
            {
                block.segments.push(SegmentState {
                    passes: 0,
                    max_passes: segment_max_passes(style),
                    chunks: Vec::new(),
                });
            }
            let segment_index = block.segments.len() - 1;
            let segment = &mut block.segments[segment_index];
            let new_passes = remaining.min(segment.max_passes - segment.passes);

            // The length field's width uses *this* segment's new passes.
            let length_bits = block.lblock + new_passes.ilog2();
            let seg_len = bio.read(length_bits) as usize;

            // A layer may add coding passes and no bytes. The MQ codeword is
            // continuous within a segment, so the encoder's rate split can put
            // passes in one layer and the bytes carrying them in the next;
            // OpenJPEG reads such a length without complaint. What must not
            // happen is a block ending up with passes and no bytes at all,
            // which `build_subbands` checks once every layer has been read.
            let segment = &mut block.segments[segment_index];
            segment.passes += new_passes;
            if seg_len > 0 {
                contributions.push(Contribution {
                    band: band_index,
                    block: block_index,
                    segment: segment_index,
                    seg_len,
                });
            }

            remaining -= new_passes;
            if remaining == 0 {
                break;
            }
        }
    }
    Ok(())
}

/// Turn the accumulated per-layer state of one resolution into its subbands.
///
/// A block that took coding passes but no bytes from any layer would hand Tier-1
/// an empty MQ stream. That is checked here rather than per packet, because a
/// single layer is allowed to contribute passes without bytes.
fn build_subbands<'a>(bands: &[BandGeom], states: Vec<BandState<'a>>) -> Result<Vec<Subband<'a>>> {
    for state in &states {
        for block in &state.blocks {
            if block.num_passes > 0 && block.segments.iter().all(|s| s.chunks.is_empty()) {
                return Err(Error::Codestream(
                    "included code-block has coding passes but no coded bytes".into(),
                ));
            }
        }
    }

    Ok(bands
        .iter()
        .zip(states)
        .map(|(band, state)| Subband {
            kind: band.kind,
            origin: band.origin,
            width: band.width,
            height: band.height,
            block_cols: band.block_cols,
            block_rows: band.block_rows,
            blocks: band
                .blocks
                .iter()
                .zip(state.blocks)
                .map(|(&(x, y, width, height), block)| CodeBlock {
                    x,
                    y,
                    width,
                    height,
                    num_passes: block.num_passes,
                    zero_bit_planes: block.zero_bit_planes,
                    segments: block
                        .segments
                        .into_iter()
                        .map(|s| CodedSegment {
                            passes: s.passes,
                            chunks: s.chunks,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect())
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
