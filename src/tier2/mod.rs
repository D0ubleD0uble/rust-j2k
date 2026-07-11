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
//! This stage runs once per **tile**: a tile's packets are its own, in its own
//! progression, and nothing about them crosses a tile boundary. The packet
//! stream walks four axes — layer, resolution, component, and *position* (the
//! precinct) — in whichever order that tile's `COD` names. The tile's data is
//! then `header₀ body₀ header₁ body₁ …` with no padding, and the packets must
//! tile it exactly; a leftover byte means a misread field.
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

use std::collections::HashSet;

use crate::codestream::markers::{Progression, Rect, Siz, marker};
use crate::codestream::{MainHeader, Tile};
use crate::{Error, Result};
use bio::BitReader;
use tagtree::TagTree;

/// Ceiling on the number of precincts one tile may hold, across every component
/// and resolution. Each precinct costs a `PrecinctGeom` (its own `Vec`) plus a
/// tag-tree pair per subband, none of it bounded by the code-block or sample
/// budgets — a band empty in one axis carries no blocks while its resolution
/// still has a full column of precincts. 2^18 leaves any real encode far behind
/// (a 4096-square image at the 64×64 precincts JPIP favours needs ~5.5k) while
/// capping the allocation in the tens of megabytes. Enforced *while* the geometry
/// is built, before the precincts are allocated — see [`resolution_geoms`].
const MAX_PRECINCTS: usize = 1 << 18;

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

/// Parse all packets of one tile into per-code-block coded segments.
///
/// `header` is the tile's resolved header (`Codestream::tile_header`), so a
/// tile-part COD/QCD override is already in force and nothing below needs to know
/// a main header exists. The geometry is the tile's own too: every
/// tile-component bound is the *tile*'s rect on that component's grid, not the
/// image's.
///
/// There is one packet per (layer, resolution, component, precinct) the tile
/// carries, enumerated in `COD`'s progression order — see [`for_each_packet`].
/// Each component carries its own tile-component geometry, so a sub-sampled
/// component's subbands are smaller at the same resolution, and its precinct
/// lattice is coarser on the reference grid.
pub fn decode_packets<'a>(header: &MainHeader, tile: &'a Tile<'a>) -> Result<CodedData<'a>> {
    let data: &'a [u8] = &tile.data;

    // Precincts are bounded *as the geometry is built*, not after: a precinct's
    // grid dimensions are cheap to compute but each precinct then allocates a
    // `PrecinctGeom` with its own `Vec`, and the block count does not bound them
    // — a band empty in one axis has no blocks while its resolution still has a
    // full column of precincts. A 1 × 2^26 image under a 2^1 precinct is the
    // shape that exploits it: ~2^25 precincts, each a heap allocation, built
    // before any after-the-fact check could fire. So the cap is threaded through
    // `resolution_geoms` and enforced before the allocation, across every
    // component of the tile (`resolution_geoms` mutates the running total).
    let mut precinct_budget = MAX_PRECINCTS;
    let component_count = header.siz.components.len();
    let geoms = (0..component_count)
        .map(|c| resolution_geoms(header, tile.index, c, &mut precinct_budget))
        .collect::<Result<Vec<_>>>()?;

    // The sample budget bounds the decoded buffers, but the per-block
    // bookkeeping below — `BlockState` and the eventual `CodeBlock`s, roughly
    // 200 bytes a block — is driven by the code-block *count*, which legal 4×4
    // blocks push toward samples/16: a sub-kilobyte header could demand ~1 GiB
    // of metadata. 2^19 clears every plausible real encode (64×64 default blocks
    // at the full sample budget need ~2^15) while capping hostile geometry near
    // 100 MiB. The geometry tuples already built above are transient and an
    // order of magnitude cheaper per block.
    const MAX_CODE_BLOCKS: usize = 1 << 19;
    let total_blocks: usize = geoms
        .iter()
        .flatten()
        .flat_map(|level| &level.bands)
        .map(|band| band.blocks.len())
        .sum();
    if total_blocks > MAX_CODE_BLOCKS {
        return Err(Error::Unsupported(format!(
            "{total_blocks} code-blocks exceeds the decode guard of {MAX_CODE_BLOCKS}"
        )));
    }

    // One state per (component, resolution, band), carried across every layer.
    // The tag trees decode incrementally and there is a pair per *precinct*, so
    // they must outlive the packet that starts them.
    let mut states: Vec<Vec<Vec<BandState<'a>>>> = geoms
        .iter()
        .map(|component| {
            component
                .iter()
                .map(|level| level.bands.iter().map(BandState::new).collect())
                .collect()
        })
        .collect();

    let walk = PacketWalk {
        siz: &header.siz,
        tile: header
            .siz
            .tile_rect(tile.index)
            .ok_or_else(|| Error::Inconsistent(format!("no tile {} in SIZ", tile.index)))?,
        geoms: &geoms,
    };
    let delimiters = Delimiters {
        sop: header.cod.use_sop,
        eph: header.cod.use_eph,
    };
    let mut cursor = 0usize;
    let mut packet_index = 0u32;
    for_each_packet(header, &walk, |layer, resolution, component, precinct| {
        let geom = &geoms[component][resolution];
        cursor = parse_packet(
            data,
            cursor,
            layer,
            packet_index,
            delimiters,
            header.components[component].coding.code_block_style,
            &geom.bands,
            precinct,
            &mut states[component][resolution],
        )?;
        packet_index += 1;
        Ok(())
    })?;

    let components: Vec<ComponentCoded<'a>> = states
        .into_iter()
        .zip(&geoms)
        .map(|(component_states, component_geoms)| {
            Ok(ComponentCoded {
                resolutions: component_states
                    .into_iter()
                    .zip(component_geoms)
                    .map(|(band_states, level)| {
                        Ok(Resolution {
                            subbands: build_subbands(&level.bands, band_states)?,
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
/// sample dimensions, the code-block grid (each block's band-relative position
/// and size), and how the precincts divide that grid up.
struct BandGeom {
    kind: BandKind,
    origin: (u32, u32),
    width: usize,
    height: usize,
    block_cols: usize,
    block_rows: usize,
    /// `(x, y, width, height)` per block, row-major over the **whole band**.
    blocks: Vec<(usize, usize, usize, usize)>,
    /// One entry per precinct of the enclosing resolution, in raster order — so
    /// `precincts.len()` is the same on every band of a resolution, even where
    /// the band is empty and each entry owns nothing.
    precincts: Vec<PrecinctGeom>,
}

/// One precinct's slice of one subband: the sub-grid of code-blocks it owns
/// (ISO/IEC 15444-1 B.7).
///
/// The precinct partition and the code-block partition are both anchored at the
/// canvas origin, and a code-block is never larger than its precinct, so the
/// blocks of a precinct are a contiguous rectangle of the band's block grid and
/// every block belongs to exactly one precinct. `cols × rows` is therefore both
/// the block count and the dimensions of the two tag trees the packet header
/// runs over this precinct.
struct PrecinctGeom {
    cols: usize,
    rows: usize,
    /// Indices into [`BandGeom::blocks`], row-major within the precinct.
    blocks: Vec<usize>,
}

/// One resolution level of a tile-component: its precinct grid and its subbands.
///
/// A packet exists per (layer, resolution, component, **precinct**), so the
/// precinct grid is what decides how many packets this resolution contributes.
/// A resolution whose rectangle is empty — `trx0 == trx1` or `try0 == try1` —
/// has **zero precincts** (ISO B.6) and so contributes *no* packet. Reading one
/// anyway would consume the next resolution's bytes and desynchronise the rest
/// of the tile.
///
/// Empty resolutions are not a curiosity: a tile-component only one sample wide
/// at an odd origin has `ceil(u0/2) == ceil(u1/2)`, so it vanishes one level up.
/// A single tile at the canvas origin can never produce one (its `trx0` is 0 and
/// its `trx1` at least 1), which is why this only matters once tiles exist.
/// OpenJPEG skips them the same way: `opj_pi_next_*` bounds its precinct loop by
/// `res->pw * res->ph`, which is zero here.
struct ResolutionGeom {
    /// Precinct exponents `(PPx, PPy)` on **this resolution's** grid.
    ppx: u32,
    ppy: u32,
    /// The precinct grid: OpenJPEG's `res->pw` and `res->ph`.
    precincts_wide: usize,
    precincts_high: usize,
    bands: Vec<BandGeom>,
}

impl ResolutionGeom {
    /// How many packets this resolution carries per (layer, component): one per
    /// precinct, and zero when the resolution is empty.
    fn precinct_count(&self) -> usize {
        self.precincts_wide * self.precincts_high
    }
}

/// Everything the packet walk needs beyond the geometry itself: the tile's
/// rectangle on the **reference grid** and the components' sub-sampling.
///
/// The two positional orders below enumerate precincts by sweeping the reference
/// grid, not by counting, so they need the canvas coordinates a resolution's
/// precinct lattice is anchored to — which the per-component geometry has
/// already divided away.
struct PacketWalk<'a> {
    siz: &'a Siz,
    tile: Rect,
    geoms: &'a [Vec<ResolutionGeom>],
}

impl PacketWalk<'_> {
    /// A component's sub-sampling `(XRsiz, YRsiz)`.
    fn sampling(&self, comp: usize) -> (u64, u64) {
        let c = &self.siz.components[comp];
        (u64::from(c.x_sampling), u64::from(c.y_sampling))
    }

    /// How many decomposition levels still separate resolution `res` of `comp`
    /// from the tile-component grid — OpenJPEG's `levelno`.
    fn level(&self, comp: usize, res: usize) -> u32 {
        (self.geoms[comp].len() - 1 - res) as u32
    }

    /// The precinct of `(comp, res)` whose top-left corner sits at the
    /// reference-grid point `(x, y)`, or `None` if no precinct starts there
    /// (ISO B.12.1.3–B.12.1.5; OpenJPEG's `opj_pi_next_rpcl`).
    ///
    /// The lattice test is what keeps the sweep injective: the step below is the
    /// *finest* precinct over all components, so a component with a coarser
    /// partition must be skipped at the points between its own precinct corners.
    /// The `x == tx0` escape is the tile's leading partial precinct, whose corner
    /// is off-lattice because the tile begins mid-precinct.
    ///
    /// All the scaling here is `u64` and unguarded: a resolution's precinct span
    /// is `XRsiz · 2^(PPx + level)`, at most `255 · 2^(15 + 32) < 2^55`, so it
    /// always fits. This is deliberately **not** the 32-bit [`shl32`] the step
    /// uses — OpenJPEG guards the step at 32 bits but scales the emission span in
    /// 64, and a coarse resolution of a deep pyramid (span past `2^32`) is a
    /// packet it emits. Gating emission on `shl32` would drop that packet and
    /// desynchronise the tile.
    fn precinct_at(&self, comp: usize, res: usize, x: u64, y: u64) -> Option<usize> {
        let geom = &self.geoms[comp][res];
        if geom.precinct_count() == 0 {
            return None;
        }
        let level = self.level(comp, res);
        let (dx, dy) = self.sampling(comp);

        // The tile-component-to-resolution scale, `XRsiz · 2^level`.
        let (sx, sy) = (dx << level, dy << level);
        let (tx0, ty0) = (u64::from(self.tile.x0), u64::from(self.tile.y0));
        let (tx1, ty1) = (u64::from(self.tile.x1), u64::from(self.tile.y1));
        let (trx0, trx1) = (tx0.div_ceil(sx), tx1.div_ceil(sx));
        let (try0, try1) = (ty0.div_ceil(sy), ty1.div_ceil(sy));
        if trx0 == trx1 || try0 == try1 {
            return None;
        }

        // The precinct's span in reference-grid units: its resolution-grid
        // exponent scaled back up through the pyramid and the sub-sampling.
        let (rpx, rpy) = (geom.ppx + level, geom.ppy + level);
        let (px, py) = (dx << rpx, dy << rpy);

        // A precinct corner sits at `v` when `v` is on the precinct lattice — or
        // when `v` is the tile's own leading edge and the lattice missed it,
        // which is the partial precinct a tile that begins mid-precinct carries.
        let corner = |v: u64, t0: u64, tr0: u64, span: u64, exp: u32| {
            v.is_multiple_of(span) || (v == t0 && !(tr0 << level).is_multiple_of(1u64 << exp))
        };
        if !corner(y, ty0, try0, py, rpy) || !corner(x, tx0, trx0, px, rpx) {
            return None;
        }

        let i = (x.div_ceil(sx) >> geom.ppx) - (trx0 >> geom.ppx);
        let j = (y.div_ceil(sy) >> geom.ppy) - (try0 >> geom.ppy);
        debug_assert!(
            i < geom.precincts_wide as u64 && j < geom.precincts_high as u64,
            "precinct index ({i}, {j}) out of the {}×{} grid",
            geom.precincts_wide,
            geom.precincts_high,
        );
        Some(j as usize * geom.precincts_wide + i as usize)
    }

    /// The sweep's step: the finest precinct span, in reference-grid units, over
    /// the `(component, resolution)` pairs the order sweeps together. Stepping by
    /// the minimum is what lets one sweep serve every component at once —
    /// [`precinct_at`](Self::precinct_at) then filters out the points that are
    /// not a given component's precinct corner.
    ///
    /// `None` when no pair contributes, which happens when every one of them
    /// scales past the 32-bit reference grid. OpenJPEG skips those the same way,
    /// so the packet stream stays aligned with the oracle's.
    fn step(&self, pairs: impl Iterator<Item = (usize, usize)>) -> Option<(u64, u64)> {
        let (mut step_x, mut step_y): (Option<u64>, Option<u64>) = (None, None);
        for (comp, res) in pairs {
            let geom = &self.geoms[comp][res];
            let level = self.level(comp, res);
            let (dx, dy) = self.sampling(comp);
            if let Some(v) = shl32(dx, geom.ppx + level) {
                step_x = Some(step_x.map_or(v, |cur: u64| cur.min(v)));
            }
            if let Some(v) = shl32(dy, geom.ppy + level) {
                step_y = Some(step_y.map_or(v, |cur: u64| cur.min(v)));
            }
        }
        Some((step_x?, step_y?))
    }
}

/// `value << shift`, or `None` when the result leaves the 32-bit reference grid.
///
/// Used only to derive the sweep *step*: OpenJPEG minimises `pi->dx` under a
/// 32-bit guard (`opj_pi_next_rpcl`'s `first` block), skipping a
/// `(component, resolution)` pair whose span overflows 32 bits. A pair skipped
/// here only widens the step past that pair's own precincts, which the corner
/// test in [`PacketWalk::precinct_at`] filters back out — so the packet set is
/// unchanged. Emission itself is **not** gated this way: `precinct_at` scales in
/// full `u64`, because OpenJPEG emits a coarse-resolution packet whose span sits
/// past `2^32`.
fn shl32(value: u64, shift: u32) -> Option<u64> {
    let scaled = value.checked_shl(shift)?;
    (scaled <= u64::from(u32::MAX)).then_some(scaled)
}

/// One progression volume the packet walk enumerates: an order over a sub-range
/// of the layer, resolution, and component axes. Without a `POC` marker there is
/// exactly one, spanning every axis in `COD`'s order.
struct Volume {
    progression: Progression,
    /// Layers `0..layers` (a `POC` volume's layer range always starts at 0).
    layers: u32,
    resolutions: std::ops::Range<usize>,
    components: std::ops::Range<usize>,
}

/// Visit every packet of the tile in `header`'s progression order (ISO/IEC
/// 15444-1 B.12.1), calling `f(layer, resolution, component, precinct)` for each.
///
/// Without a `POC` marker the order is `COD`'s single progression over every
/// axis. A `POC` marker replaces it with a *sequence* of progression volumes
/// (A.6.6), each an order over its own layer/resolution/component sub-range; the
/// packet stream is their concatenation, and a packet reached by more than one
/// volume is emitted once, by the first — OpenJPEG dedups through a shared
/// `include` array, mirrored here by `seen`.
///
/// Each volume is enumerated by [`run_order`]. The five orders are permutations
/// of the four axes — layer, resolution, component, position — and the three
/// positional ones sweep the reference grid; see that function.
fn for_each_packet<F>(header: &MainHeader, walk: &PacketWalk<'_>, mut f: F) -> Result<()>
where
    F: FnMut(u32, usize, usize, usize) -> Result<()>,
{
    let components = walk.geoms.len();
    let max_res = walk.geoms.iter().map(Vec::len).max().unwrap_or(0);
    let layers = u32::from(header.cod.layers);

    let volumes: Vec<Volume> = if header.poc.is_empty() {
        vec![Volume {
            progression: header.cod.progression,
            layers,
            resolutions: 0..max_res,
            components: 0..components,
        }]
    } else {
        // Clamp each volume to the axes that actually exist — the layer and
        // component ends are only bounded here, where the counts are known
        // (A.6.6): `layno1` to the layer count, `compno1` to the component count.
        header
            .poc
            .iter()
            .map(|v| Volume {
                progression: v.progression,
                layers: u32::from(v.layer_end).min(layers),
                resolutions: usize::from(v.res_start)..usize::from(v.res_end).min(max_res),
                components: usize::from(v.comp_start)..usize::from(v.comp_end).min(components),
            })
            .collect()
    };

    // With one volume every tuple is visited once, so the dedup is pure overhead
    // and skipped; a `POC` sequence needs it, because volumes may overlap.
    if volumes.len() == 1 {
        return run_order(&volumes[0], walk, &mut f);
    }
    let mut seen: HashSet<(u32, usize, usize, usize)> = HashSet::new();
    for volume in &volumes {
        run_order(volume, walk, &mut |layer, res, comp, precinct| {
            if seen.insert((layer, res, comp, precinct)) {
                f(layer, res, comp, precinct)
            } else {
                Ok(())
            }
        })?;
    }
    Ok(())
}

/// Enumerate one progression volume, calling `emit` for each of its packets in
/// order.
///
/// The five orders nest the four axes differently:
///
/// ```text
/// LRCP    l → r → c → p        RPCL    r → p → c → l
/// RLCP    r → l → c → p        PCRL    p → c → r → l
///                              CPRL    c → p → r → l
/// ```
///
/// LRCP and RLCP put position innermost and enumerate it by counting a
/// resolution's precincts in raster order. RPCL, PCRL and CPRL put it outside
/// another axis, where counting will not do — a packet's place depends on where
/// its precinct sits on the canvas, and components with different sub-sampling or
/// precinct sizes interleave — so they sweep the reference grid in steps of the
/// finest precinct and ask each (component, resolution) whether a precinct of its
/// own starts at that point. The sweep step is minimised over *every* component
/// and resolution, not just the volume's range (OpenJPEG's `opj_pi_next_rpcl`
/// does the same in its `first` block), so a `POC` sub-range cannot coarsen it
/// and skip a corner; only which packets are *emitted* is restricted to the
/// range.
fn run_order<F>(volume: &Volume, walk: &PacketWalk<'_>, emit: &mut F) -> Result<()>
where
    F: FnMut(u32, usize, usize, usize) -> Result<()>,
{
    let layers = volume.layers;
    let res_range = volume.resolutions.clone();
    let comp_range = volume.components.clone();
    // A component's resolution axis is as long as its own COD/COC says, so the
    // shallower ones simply do not appear at the tail of the deepest one's.
    let has = |comp: usize, res: usize| res < walk.geoms[comp].len();
    let precincts = |comp: usize, res: usize| walk.geoms[comp][res].precinct_count();

    let (tx0, ty0) = (u64::from(walk.tile.x0), u64::from(walk.tile.y0));
    let (tx1, ty1) = (u64::from(walk.tile.x1), u64::from(walk.tile.y1));

    // The positional sweep's step, over every component and resolution — the
    // range restricts emission, not the step.
    let all_pairs =
        || (0..walk.geoms.len()).flat_map(|c| (0..walk.geoms[c].len()).map(move |r| (c, r)));

    match volume.progression {
        Progression::Lrcp => {
            for layer in 0..layers {
                for res in res_range.clone() {
                    for comp in comp_range.clone() {
                        if !has(comp, res) {
                            continue;
                        }
                        for precinct in 0..precincts(comp, res) {
                            emit(layer, res, comp, precinct)?;
                        }
                    }
                }
            }
        }
        Progression::Rlcp => {
            for res in res_range.clone() {
                for layer in 0..layers {
                    for comp in comp_range.clone() {
                        if !has(comp, res) {
                            continue;
                        }
                        for precinct in 0..precincts(comp, res) {
                            emit(layer, res, comp, precinct)?;
                        }
                    }
                }
            }
        }
        // Resolution outermost, then the positional sweep, then component, then
        // layer.
        Progression::Rpcl => {
            let Some((step_x, step_y)) = walk.step(all_pairs()) else {
                return Ok(());
            };
            for res in res_range.clone() {
                let mut y = ty0;
                while y < ty1 {
                    let mut x = tx0;
                    while x < tx1 {
                        for comp in comp_range.clone() {
                            if !has(comp, res) {
                                continue;
                            }
                            if let Some(precinct) = walk.precinct_at(comp, res, x, y) {
                                for layer in 0..layers {
                                    emit(layer, res, comp, precinct)?;
                                }
                            }
                        }
                        x += step_x - (x % step_x);
                    }
                    y += step_y - (y % step_y);
                }
            }
        }
        // The sweep outermost, so its step is the finest precinct over every
        // component and resolution at once.
        Progression::Pcrl => {
            let Some((step_x, step_y)) = walk.step(all_pairs()) else {
                return Ok(());
            };
            let mut y = ty0;
            while y < ty1 {
                let mut x = tx0;
                while x < tx1 {
                    for comp in comp_range.clone() {
                        for res in res_range.clone() {
                            if !has(comp, res) {
                                continue;
                            }
                            if let Some(precinct) = walk.precinct_at(comp, res, x, y) {
                                for layer in 0..layers {
                                    emit(layer, res, comp, precinct)?;
                                }
                            }
                        }
                    }
                    x += step_x - (x % step_x);
                }
                y += step_y - (y % step_y);
            }
        }
        // Component outermost, so each gets its own sweep — and its own step, the
        // finest precinct over that component's resolutions alone. This is where
        // CPRL parts company with PCRL: PCRL interleaves the components inside one
        // shared sweep, CPRL finishes a component before starting the next.
        Progression::Cprl => {
            for comp in comp_range.clone() {
                let pairs = (0..walk.geoms[comp].len()).map(|r| (comp, r));
                let Some((step_x, step_y)) = walk.step(pairs) else {
                    continue;
                };
                let mut y = ty0;
                while y < ty1 {
                    let mut x = tx0;
                    while x < tx1 {
                        for res in res_range.clone() {
                            if !has(comp, res) {
                                continue;
                            }
                            if let Some(precinct) = walk.precinct_at(comp, res, x, y) {
                                for layer in 0..layers {
                                    emit(layer, res, comp, precinct)?;
                                }
                            }
                        }
                        x += step_x - (x % step_x);
                    }
                    y += step_y - (y % step_y);
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

/// Compute the resolution → precinct → subband → code-block geometry for one
/// tile-component, coarsest resolution first (ISO B.5–B.7, Eq. B-14/B-15/B-16).
///
/// The bounds come from `tile`'s rect on *this component's* grid (B.3, Eq.
/// B-7/B-12): they divide by the component's sub-sampling, so two components of
/// one tile can yield different subband sizes, and they sit at the tile's own
/// offset, so two tiles of one component yield subbands at different origins.
/// Those origins are as load-bearing as the sizes — the inverse DWT reads its
/// interleave parity from them, and the assembly stage places the tile by them.
///
/// `precinct_budget` is the remaining precinct allowance across the whole tile;
/// this decrements it per resolution and errors before the allocation once it is
/// spent, so the [`MAX_PRECINCTS`] cap holds regardless of how a hostile header
/// splits the count between components.
fn resolution_geoms(
    header: &MainHeader,
    tile: u32,
    comp: usize,
    precinct_budget: &mut usize,
) -> Result<Vec<ResolutionGeom>> {
    let siz = &header.siz;
    let cod = &header
        .components
        .get(comp)
        .ok_or_else(|| Error::Inconsistent(format!("no coding parameters for component {comp}")))?
        .coding;

    let nl = cod.decomposition_levels as i64;
    if nl > 32 {
        return Err(Error::Unsupported(format!(
            "{nl} decomposition levels exceeds the 32-level maximum"
        )));
    }

    let rect = siz.tile_component_rect(tile, comp).ok_or_else(|| {
        Error::Inconsistent(format!("no tile {tile} or no component {comp} in SIZ"))
    })?;
    if rect.is_empty() {
        return Err(Error::Codestream(format!(
            "tile {tile} has an empty area on component {comp}"
        )));
    }
    let (tcx0, tcx1) = (i64::from(rect.x0), i64::from(rect.x1));
    let (tcy0, tcy1) = (i64::from(rect.y0), i64::from(rect.y1));

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

    let mut levels = Vec::with_capacity((nl + 1) as usize);
    for r in 0..=nl {
        // The resolution's own rectangle (ISO B.5, Eq. B-14). It is what the
        // precinct grid is cut from — not the bands', which can be empty while
        // the resolution is not.
        let level = nl - r;
        let pow = 1i64 << level;
        let (trx0, trx1) = (ceil_div(tcx0, pow), ceil_div(tcx1, pow));
        let (try0, try1) = (ceil_div(tcy0, pow), ceil_div(tcy1, pow));

        // Precinct exponents are quoted on the resolution grid. The parser
        // guarantees a non-zero exponent above resolution 0, so the `- 1` below
        // is always in range.
        let (ppx, ppy) = cod.precinct(r as usize);
        debug_assert!(r == 0 || (ppx >= 1 && ppy >= 1));

        // The precinct grid over the resolution (ISO B.6, Eq. B-16), anchored at
        // the canvas origin like every other partition. `prc_span` returns the
        // count and the lattice point the first precinct starts on.
        let (precincts_wide, tl_prc_x) = prc_span(trx0, trx1, ppx);
        let (precincts_high, tl_prc_y) = prc_span(try0, try1, ppy);

        // Spend the tile's precinct budget before this resolution's precincts are
        // allocated below. `precincts_wide/high` are cheap products of the rect
        // and the exponents; the `PrecinctGeom`s that follow are not, so the cap
        // has to bite here rather than after the fact. `checked_mul` guards the
        // product itself, since a 2^0 precinct at resolution 0 makes each factor
        // as large as the resolution.
        let here = precincts_wide.checked_mul(precincts_high);
        match here.filter(|&n| n <= *precinct_budget) {
            Some(n) => *precinct_budget -= n,
            None => {
                return Err(Error::Unsupported(format!(
                    "tile precinct count exceeds the decode guard of {MAX_PRECINCTS}"
                )));
            }
        }

        // The *code-block group*: where that precinct lands on the subband grid.
        // At resolution 0 the band is the resolution, so the two coincide. Above
        // it the three bands sit on a grid one level coarser than the
        // resolution's, so the precinct halves — which is why a 2^0 precinct is
        // legal only at resolution 0, and why the code-block is capped one
        // exponent lower here (ISO B.6, Eq. B-17/B-18).
        let cbg = if r == 0 {
            Cbg {
                x0: tl_prc_x,
                y0: tl_prc_y,
                x_exp: ppx,
                y_exp: ppy,
                wide: precincts_wide,
                high: precincts_high,
            }
        } else {
            Cbg {
                x0: ceil_div(tl_prc_x, 2),
                y0: ceil_div(tl_prc_y, 2),
                x_exp: ppx - 1,
                y_exp: ppy - 1,
                wide: precincts_wide,
                high: precincts_high,
            }
        };
        // A code-block never outgrows the precinct that has to contain it
        // (ISO B.7): OpenJPEG's `cblkwidthexpn = min(tccp->cblkw, cbgwidthexpn)`.
        // With a maximal partition this caps at 2^15 / 2^14 and never bites; with
        // a 2^7 precinct at resolution 3 it shrinks a 2^6 block to 2^6, and a
        // 2^1 precinct shrinks it to a single sample.
        let (xcb, ycb) = (xcb.min(cbg.x_exp), ycb.min(cbg.y_exp));

        let bands = if r == 0 {
            // The coarsest resolution carries only the NLLL band, and it *is*
            // the resolution rectangle.
            vec![band_geom(
                BandKind::Ll,
                trx0,
                trx1,
                try0,
                try1,
                xcb,
                ycb,
                &cbg,
            )]
        } else {
            // Finer levels add HL, LH, HH at decomposition level `nb = NL-r+1`.
            let nb = level + 1;
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
                    xcb,
                    ycb,
                    &cbg,
                )
            })
            .collect()
        };

        levels.push(ResolutionGeom {
            ppx,
            ppy,
            precincts_wide,
            precincts_high,
            bands,
        });
    }
    Ok(levels)
}

/// The precinct partition as it lands on the *subband* grid: the top-left of the
/// first code-block group, the group's exponents, and the grid's extent. One per
/// resolution, shared by its bands (ISO B.6).
struct Cbg {
    x0: i64,
    y0: i64,
    x_exp: u32,
    y_exp: u32,
    wide: usize,
    high: usize,
}

/// How many precincts of span `2^exp` cover `[lo, hi)`, and the lattice point the
/// first one starts on (ISO B.6, Eq. B-16). An empty span has zero precincts —
/// and so carries no packet — which is why this returns a count rather than
/// rounding up to one.
fn prc_span(lo: i64, hi: i64, exp: u32) -> (usize, i64) {
    if hi <= lo {
        return (0, 0);
    }
    let span = 1i64 << exp;
    let start = lo.div_euclid(span) * span;
    let end = ceil_div(hi, span) * span;
    (((end - start) >> exp) as usize, start)
}

/// Build one subband's geometry from its sample bounds `[bx0, bx1) × [by0, by1)`
/// and the effective code-block exponents, tiling it with the code-block grid
/// anchored at the canvas origin (ISO B.7), then grouping those blocks by the
/// precinct they fall in.
#[allow(clippy::too_many_arguments)]
fn band_geom(
    kind: BandKind,
    bx0: i64,
    bx1: i64,
    by0: i64,
    by1: i64,
    xcb: u32,
    ycb: u32,
    cbg: &Cbg,
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

    // Cut the band's block grid up by precinct. A precinct's rectangle is its
    // code-block group clipped to the band (OpenJPEG's `prc->x0 = max(cbgxstart,
    // band->x0)`), and the block grid divides that rectangle evenly because the
    // block never outgrew the group above. So every block lands in exactly one
    // precinct, and a precinct the band does not reach simply owns none.
    let group_w = 1i64 << cbg.x_exp;
    let group_h = 1i64 << cbg.y_exp;
    let mut precincts = Vec::with_capacity(cbg.wide * cbg.high);
    for j in 0..cbg.high {
        let gy0 = cbg.y0 + j as i64 * group_h;
        let (py0, py1) = (gy0.max(by0), (gy0 + group_h).min(by1));
        for i in 0..cbg.wide {
            let gx0 = cbg.x0 + i as i64 * group_w;
            let (px0, px1) = (gx0.max(bx0), (gx0 + group_w).min(bx1));

            let (cols, col0) = grid_span(px0, px1, cbw);
            let (rows, row0) = grid_span(py0, py1, cbh);
            let mut owned = Vec::with_capacity(cols * rows);
            for row in 0..rows {
                let band_row = (row0 + row as i64 - first_row) as usize;
                for col in 0..cols {
                    let band_col = (col0 + col as i64 - first_col) as usize;
                    owned.push(band_row * block_cols + band_col);
                }
            }
            precincts.push(PrecinctGeom {
                cols,
                rows,
                blocks: owned,
            });
        }
    }
    debug_assert_eq!(
        precincts.iter().map(|p| p.blocks.len()).sum::<usize>(),
        block_cols * block_rows,
        "the precincts must partition the band's code-block grid"
    );

    BandGeom {
        kind,
        origin: (bx0.max(0) as u32, by0.max(0) as u32),
        width,
        height,
        block_cols,
        block_rows,
        blocks,
        precincts,
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
/// real value, while rejecting a malformed run of zero bits rather than
/// looping on it. The count is bounded by the *raised* plane count
/// `Kmax = Mb + SPrgn` (ISO H.2), not by `Mb` alone — a background-only block
/// under a maxshift legitimately signals more zero bit-planes than the band
/// has magnitude planes. `Mb` is at most 37 (7 guard bits + exponent 31 − 1)
/// and the maxshift byte at most 255, so 292 is the largest value a
/// conformant stream can signal and this threshold resolves every one.
const ZBP_LIMIT: u32 = 293;

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
/// The counter therefore lives in [`decode_packets`], which runs once per tile
/// over that tile's tile-parts joined end to end: it starts at zero for each
/// tile and does not restart at a tile-part boundary.
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

/// One subband's decode state across the layers: a tag-tree pair **per
/// precinct**, plus a state per code-block of the whole band.
///
/// The trees are per precinct because a packet is per precinct — each one
/// decodes incrementally at a rising threshold over its own precinct's block
/// grid, and knows nothing of the blocks next door. The block states stay
/// band-indexed so that [`build_subbands`] can hand the band back whole; a block
/// is touched by exactly one precinct's packets, so the two indexings never
/// collide.
struct BandState<'a> {
    precincts: Vec<PrecinctState>,
    blocks: Vec<BlockState<'a>>,
}

/// The two tag trees one precinct runs over one subband: which code-blocks the
/// packet includes, and how many of their leading bit-planes are all zero.
struct PrecinctState {
    inclusion: TagTree,
    zero_bits: TagTree,
}

impl BandState<'_> {
    fn new(band: &BandGeom) -> Self {
        BandState {
            precincts: band
                .precincts
                .iter()
                .map(|precinct| PrecinctState {
                    inclusion: TagTree::new(precinct.cols as u32, precinct.rows as u32),
                    zero_bits: TagTree::new(precinct.cols as u32, precinct.rows as u32),
                })
                .collect(),
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

/// Parse one packet — one precinct of one resolution of one component in one
/// layer — starting at byte `start` of the tile-part `data`.
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
    precinct: usize,
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
                &band.precincts[precinct],
                &mut state.precincts[precinct],
                &mut state.blocks,
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

/// Read one precinct's code-block entries, on one subband, from this layer's
/// packet header (ISO B.10): per block its inclusion, and for a contributing
/// block the zero-bitplane count (first inclusion only), coding-pass count, and
/// the length of its byte contribution.
///
/// The blocks are visited in raster order **within the precinct**, and the tag
/// trees are addressed in the precinct's own coordinates — the packet header
/// knows nothing of the band's wider grid, so a band-relative index here would
/// address the wrong leaf on every precinct but the first.
#[allow(clippy::too_many_arguments)]
fn parse_band_header(
    band_index: usize,
    precinct: &PrecinctGeom,
    trees: &mut PrecinctState,
    blocks: &mut [BlockState<'_>],
    layer: u32,
    style: u8,
    bio: &mut BitReader,
    contributions: &mut Vec<Contribution>,
) -> Result<()> {
    let cols = precinct.cols as u32;

    for (entry, &block_index) in precinct.blocks.iter().enumerate() {
        let bx = entry as u32 % cols;
        let by = entry as u32 / cols;

        let was_included = blocks[block_index].included;
        let contributes = if was_included {
            // Already included: one bit says whether this layer adds passes.
            bio.read_bit() == 1
        } else {
            // Not yet included: the inclusion tree resolves to the first layer
            // that carries this block. Reading at `layer + 1` asks "is that
            // layer at most this one?" and leaves the tree part-decoded when it
            // is not, so the next layer resumes where this one stopped.
            match trees.inclusion.read(bx, by, layer + 1, bio) {
                Some(_) => {
                    blocks[block_index].included = true;
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
            blocks[block_index].zero_bit_planes = trees
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
        let block = &mut blocks[block_index];
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
