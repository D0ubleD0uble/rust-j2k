//! Progression-order packet iteration for Tier-2 (ISO/IEC 15444-1 B.12.1).
//!
//! Enumerates a tile's packets in the order its `COD` — or a `POC` marker's
//! sequence of progression volumes (A.6.6) — prescribes, calling back once per
//! (layer, resolution, component, precinct). The three positional orders sweep
//! the reference grid rather than counting, so this module needs the canvas
//! coordinates the per-component [`ResolutionGeom`] has already divided away.

use std::collections::HashSet;

use crate::Result;
use crate::codestream::MainHeader;
use crate::codestream::markers::{Progression, Rect, Siz};

use super::geometry::ResolutionGeom;

/// Everything the packet walk needs beyond the geometry itself: the tile's
/// rectangle on the **reference grid** and the components' sub-sampling.
///
/// The two positional orders below enumerate precincts by sweeping the reference
/// grid, not by counting, so they need the canvas coordinates a resolution's
/// precinct lattice is anchored to — which the per-component geometry has
/// already divided away.
pub(crate) struct PacketWalk<'a> {
    pub(crate) siz: &'a Siz,
    pub(crate) tile: Rect,
    pub(crate) geoms: &'a [Vec<ResolutionGeom>],
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
pub(crate) fn for_each_packet<F>(header: &MainHeader, walk: &PacketWalk<'_>, mut f: F) -> Result<()>
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
