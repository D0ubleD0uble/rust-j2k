//! Progression-order packet iteration for Tier-2 (ISO/IEC 15444-1 B.12.1).
//!
//! Enumerates a tile's packets in the order its `COD` — or a `POC` marker's
//! sequence of progression volumes (A.6.6) — prescribes, calling back once per
//! (layer, resolution, component, precinct). The three positional orders sweep
//! the reference grid rather than counting, so this module needs the canvas
//! coordinates the per-component [`ResolutionGeom`] has already divided away.

use crate::codestream::MainHeader;
use crate::codestream::markers::{PocVolume, Progression, Rect, Siz};
use crate::{Error, Result};

use super::geometry::ResolutionGeom;
use super::{Meter, WorkRefusal};

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
    /// desynchronize the tile.
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
            "precinct index ({i}, {j}) out of the {}x{} grid",
            geom.precincts_wide,
            geom.precincts_high,
        );
        Some(j as usize * geom.precincts_wide + i as usize)
    }

    /// The finest precinct span of `comp`, in reference-grid units, over all
    /// of its resolutions, on each axis. The sweep's step is the minimum of
    /// these over the (component, resolution) pairs the order sweeps together.
    /// Stepping by the minimum is what lets one sweep serve every component at
    /// once — [`precinct_at`](Self::precinct_at) then filters out the points
    /// that are not a given component's precinct corner.
    ///
    /// An axis is `None` when every one of its pairs scales past the 32-bit
    /// reference grid. OpenJPEG skips those the same way, so the packet stream
    /// stays aligned with the oracle's.
    ///
    /// Resolutions that hold no precinct count here too, as in OpenJPEG:
    /// with mixed sub-sampling a coarser step can pass over precincts and
    /// change which packets the sweep emits.
    fn spans(&self, comp: usize) -> (Option<u64>, Option<u64>) {
        let (dx, dy) = self.sampling(comp);
        let mut spans = (None, None);
        for (res, geom) in self.geoms[comp].iter().enumerate() {
            let level = self.level(comp, res);
            spans.0 = min_some(spans.0, shl32(dx, geom.ppx + level));
            spans.1 = min_some(spans.1, shl32(dy, geom.ppy + level));
        }
        spans
    }
}

/// The smaller of two optional spans, either of which may be missing.
fn min_some(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// A sweep's step on both axes, or `None` when either axis has none: then no
/// pair contributes, and the sweep emits nothing.
fn both(spans: (Option<u64>, Option<u64>)) -> Option<(u64, u64)> {
    Some((spans.0?, spans.1?))
}

/// `value << shift`, or `None` when the result leaves the 32-bit reference grid.
///
/// Used only to derive the sweep *step*: OpenJPEG minimizes `pi->dx` under a
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
pub(crate) struct Volume {
    progression: Progression,
    /// Layers `0..layers` (a `POC` volume's layer range always starts at 0).
    layers: u32,
    resolutions: std::ops::Range<usize>,
    components: std::ops::Range<usize>,
}

/// What a walk works out once for all of its volumes, rather than once per
/// volume.
#[derive(Default)]
struct Plan {
    /// For each resolution, the components that hold a precinct there, in
    /// order. Only they can have a packet at that resolution, so a loop over
    /// components runs over these and takes no step for the rest: a tile of
    /// one sample at an odd origin has a precinct at its finest resolution
    /// alone, and 32 more resolutions per component that hold none.
    held: Vec<Vec<usize>>,
    /// Each component's precinct spans (see [`PacketWalk::spans`]), when a
    /// volume sweeps positions; else empty.
    spans: Vec<(Option<u64>, Option<u64>)>,
    /// The step of a sweep over every component and resolution, which RPCL
    /// and PCRL take.
    step: Option<(u64, u64)>,
}

impl Plan {
    /// The plan for walking `volumes` over `walk`'s tile: one pass over its
    /// (component, resolution) pairs, as the geometry has made.
    fn new(walk: &PacketWalk<'_>, max_res: usize, volumes: &[Volume]) -> Self {
        let mut held = vec![Vec::new(); max_res];
        for (comp, resolutions) in walk.geoms.iter().enumerate() {
            for (res, geom) in resolutions.iter().enumerate() {
                if geom.precinct_count() > 0 {
                    held[res].push(comp);
                }
            }
        }
        let sweeps = volumes.iter().any(|v| {
            matches!(
                v.progression,
                Progression::Rpcl | Progression::Pcrl | Progression::Cprl
            )
        });
        let spans: Vec<_> = if sweeps {
            (0..walk.geoms.len()).map(|c| walk.spans(c)).collect()
        } else {
            Vec::new()
        };
        let all = spans.iter().fold((None, None), |acc, s| {
            (min_some(acc.0, s.0), min_some(acc.1, s.1))
        });
        Plan {
            held,
            spans,
            step: both(all),
        }
    }

    /// Where the components of `range` that hold a precinct at resolution
    /// `res` sit in `held[res]`.
    fn held_range(&self, res: usize, range: &std::ops::Range<usize>) -> (usize, usize) {
        let comps = &self.held[res];
        let start = comps.partition_point(|&c| c < range.start);
        let end = comps.partition_point(|&c| c < range.end);
        (start, end.max(start))
    }
}

/// The (component, resolution) pairs that hold a precinct, sorted.
type PairList = Vec<(usize, usize)>;

/// Where a volume's pairs sit: the index of a [`PairList`] and the run of it.
type ListRun = (usize, std::ops::Range<usize>);

/// The pairs each PCRL or CPRL volume of `volumes` sweeps over `walk`'s
/// tile, of at most `max_res` resolutions per component, as
/// [`Order::lists`] and [`Order::pairs`] hold them.
///
/// The pairs are sorted by component, so a volume over every resolution
/// takes a run of the whole tile's list, whatever its components: a `POC`
/// sequence of such volumes keeps one list, not one per volume. With 32
/// volumes over 16384 components, a list each is 8 MB a tile, which the
/// heap grows for and gives back on every tile.
fn pair_lists(
    plan: &Plan,
    components: usize,
    max_res: usize,
    volumes: &[Volume],
) -> (Vec<PairList>, Vec<ListRun>) {
    let swept = |v: &Volume| matches!(v.progression, Progression::Pcrl | Progression::Cprl);
    let whole = |v: &Volume| v.resolutions.start == 0 && v.resolutions.end >= max_res;
    // The held pairs of `components` over `resolutions`, sorted. They are
    // read from `plan.held`, which lies in one place, rather than from
    // each component's geometry, which does not.
    let build = |components: &std::ops::Range<usize>, resolutions: &std::ops::Range<usize>| {
        let mut list: Vec<(usize, usize)> = resolutions
            .clone()
            .flat_map(|res| {
                let (start, end) = plan.held_range(res, components);
                plan.held[res][start..end].iter().map(move |&c| (c, res))
            })
            .collect();
        list.sort_unstable();
        list
    };
    let mut lists = Vec::new();
    if volumes.iter().any(|v| swept(v) && whole(v)) {
        lists.push(build(&(0..components), &(0..max_res)));
    }
    // The volumes that built a list of their own, with the list.
    let mut owners: Vec<(usize, usize)> = Vec::new();
    let pairs = volumes
        .iter()
        .enumerate()
        .map(|(index, v)| {
            if !swept(v) {
                return (0, 0..0);
            }
            if whole(v) {
                let list = &lists[0];
                let start = list.partition_point(|&(c, _)| c < v.components.start);
                let end = list.partition_point(|&(c, _)| c < v.components.end);
                return (0, start..end.max(start));
            }
            // A volume over fewer resolutions shares the list of an earlier
            // one over the same ranges. There are at most 32 volumes.
            let same = owners.iter().find(|&&(o, _)| {
                volumes[o].resolutions == v.resolutions && volumes[o].components == v.components
            });
            let list = match same {
                Some(&(_, list)) => list,
                None => {
                    lists.push(build(&v.components, &v.resolutions));
                    owners.push((index, lists.len() - 1));
                    lists.len() - 1
                }
            };
            (list, 0..lists[list].len())
        })
        .collect();
    (lists, pairs)
}

/// Refuse a `POC` sequence that leaves out packets of the tile, with
/// [`Error::Codestream`].
///
/// A tile has a packet for every layer of `COD`, every resolution each
/// component has (its own `COD` or `COC` level count), and every precinct of
/// that resolution, including one with no code-blocks (B.6, B.9). A
/// resolution empty on the tile has no precincts, so no packets, and needs
/// no volume. `header.poc` holds the volumes the walk uses (the tile's own
/// when its first tile-part has a POC, else the main header's and then any
/// later tile-part's), and they must define every packet (A.6.6, B.12.3); a
/// packet none of them reaches is missing from the codestream, as if dropped from
/// the end of the tile (B.11), and decoding it as empty would invent its
/// samples. OpenJPEG decodes such a tile with those packets left empty.
///
/// A volume's layers start at 0, so a (component, resolution) has every
/// layer's packets only when one volume reaching the last layer covers it.
/// The check is on those axes, before the walk, and costs `components ×
/// (volumes + resolutions)` steps, against the geometry's own `components ×
/// resolutions` built before it. A layer order reads every precinct of a
/// resolution it reaches. A positional order reads the precincts its sweep
/// meets, which with mixed sub-sampling is not always all of them; that is
/// the same with or without a `POC`, as in OpenJPEG's encoder and decoder,
/// and not refused here.
///
/// A decode with `strict` off takes the packets left out as missing instead,
/// like a cut tile-part's, through [`poc_gaps`].
pub(crate) fn check_poc_coverage(
    header: &MainHeader,
    walk: &PacketWalk<'_>,
    tile: u32,
) -> Result<()> {
    let layers = header.cod.layers;
    poc_gaps(header, walk, |comp, res, reached| {
        Err(Error::codestream(format!(
            "the `POC` volumes of tile {tile} leave out its packets of component \
             {comp}, resolution {res}, from layer {reached} of {layers}"
        )))
    })
}

/// Call `gap(component, resolution, layer)` for each (component, resolution)
/// whose packets the `POC` volumes leave out, from `layer` on, in component
/// and then resolution order, as [`check_poc_coverage`] defines it; stop at
/// the first error `gap` returns.
pub(crate) fn poc_gaps(
    header: &MainHeader,
    walk: &PacketWalk<'_>,
    mut gap: impl FnMut(usize, usize, u16) -> Result<()>,
) -> Result<()> {
    if header.poc.is_empty() {
        return Ok(());
    }
    let layers = header.cod.layers;
    for (comp, resolutions) in walk.geoms.iter().enumerate() {
        let covers = |v: &&PocVolume| (v.comp_start..v.comp_end).contains(&(comp as u16));
        // Bit `r` is set when a volume over every layer covers resolution `r`
        // of this component. A component has at most 33 resolutions.
        let mut full = 0u64;
        for v in header.poc.iter().filter(covers) {
            if v.layer_end >= layers {
                let (start, end) = (u32::from(v.res_start).min(64), u32::from(v.res_end).min(64));
                full |= mask_below(end) & !mask_below(start);
            }
        }
        for (res, geom) in resolutions.iter().enumerate() {
            if (full >> res) & 1 == 0 && geom.precinct_count() > 0 {
                let reached = header
                    .poc
                    .iter()
                    .filter(covers)
                    .filter(|v| (usize::from(v.res_start)..usize::from(v.res_end)).contains(&res))
                    .map(|v| v.layer_end)
                    .max()
                    .unwrap_or(0);
                gap(comp, res, reached)?;
            }
        }
    }
    Ok(())
}

/// The bits below bit `n`, for `n` up to 64.
fn mask_below(n: u32) -> u64 {
    1u64.checked_shl(n).map_or(u64::MAX, |bit| bit - 1)
}

/// Visit every packet of the tile in `header`'s progression order (ISO/IEC
/// 15444-1 B.12.1), calling `f(layer, resolution, component, precinct)` for each.
///
/// Without a `POC` marker the order is `COD`'s single progression over every
/// axis. A `POC` marker replaces it with a *sequence* of progression volumes
/// (A.6.6), each an order over its own layer/resolution/component sub-range; the
/// packet stream is their concatenation, and a packet reached by more than one
/// volume is emitted once, by the first — OpenJPEG dedups through a shared
/// `include` array, mirrored here by [`Emitted`].
///
/// The order is stepped by a [`Cursor`] over an [`Order`]; see
/// [`Order::next`] for the five orders and where the loops are charged.
///
/// The loops spend `meter` as they go, the
/// points that hold no packet included, so a walk under a bound stops however
/// sparse its packets are.
///
/// `poc_bitmap` and `poc_counts` are how a `POC` sequence's packets are held
/// (see [`Emitted`]): in a bitmap when they are no more than `poc_bitmap`,
/// and else as a count per precinct if `poc_counts`, or the walk is
/// [`crate::Error::Limit`].
pub(crate) fn for_each_packet<F>(
    header: &MainHeader,
    walk: &PacketWalk<'_>,
    meter: &Meter,
    poc_bitmap: u64,
    poc_counts: bool,
    mut f: F,
) -> Result<()>
where
    F: FnMut(u32, usize, usize, usize) -> Result<()>,
{
    let order = Order::new(header, walk);
    // With one volume every tuple is visited once, so the dedup is pure overhead
    // and skipped; a `POC` sequence needs it, because volumes may overlap.
    if order.volumes.len() == 1 {
        return order.walk_runs(walk, meter, f);
    }
    let mut emitted = Emitted::new(
        &order.volumes,
        walk,
        order.max_res,
        meter,
        poc_bitmap,
        poc_counts,
    )?;
    let skip_visits = emitted.skip_visits();
    order.walk_runs(walk, meter, |layer, res, comp, precinct| {
        // A packet skipped from the bitmap is not charged. Only a packet
        // read is marked, and a read takes a visit and at least a byte of
        // packet header; at most 31 later volumes can meet it again. A
        // bitmap skip takes about 4 ns, so the skips cost at most about
        // 0.12 µs per byte, under the 0.19 µs a byte the fastest
        // conformance codestream past 100 KB takes to decode (best of
        // five release builds). Charging them would refuse files
        // OpenJPEG reads quickly, such as 31 identical volumes over 1 MB
        // of empty packets. A skip by the counts is charged (see
        // `COUNTS_SKIP_VISITS`).
        if emitted.insert(walk, layer, res, comp, precinct)? {
            f(layer, res, comp, precinct)
        } else if skip_visits > 0 {
            meter.spend(skip_visits, WorkRefusal::PacketWalk)
        } else {
            Ok(())
        }
    })
}

/// The visits a packet skipped by [`Emitted`]'s per-precinct counts is
/// charged. The packets read were once kept in a hash set, where a lookup
/// among millions missed the cache: about 80 to 180 ns, measured on the
/// `setskip` shape of 1.4e7 packets read. Twelve visits priced a visit at
/// 15 ns, toward the cheap end of the 1 to 55 ns one takes (see
/// `DECODE_VISITS_PER_BYTE`). A count is cheaper to look up than a set
/// entry, so the charge, kept so that the same files are refused, errs
/// higher still.
pub(crate) const COUNTS_SKIP_VISITS: usize = 12;

/// The packets a `POC` sequence has emitted so far (see [`for_each_packet`]),
/// as OpenJPEG's `include` array holds them.
///
/// Packet `(layer, res, comp, precinct)` is the bit `start + layer *
/// precincts + precinct`, where `start` is where its (component,
/// resolution) begins and `precincts` is that resolution's precinct count.
/// Each (component, resolution) holds the layers below the highest layer end
/// of the volumes that cover it, so the bits are exactly the packets the
/// volumes reach.
///
/// A layer order reads all of a resolution's precincts, so if every volume
/// is one and they reach more packets than the visits the walk has left, it
/// cannot finish, and it fails here with [`Error::Limit`].
///
/// Otherwise they are a bitmap when they are no more than the decode's
/// visits left, and no more than its default bound when the caller raised
/// it. Each packet read spends a visit, so a walk that reads them all
/// never has more. The bitmap is then at most a bit per visit of the
/// default bound: 2 MB under the decode's floor of 2^24 visits, and 4 bytes
/// per byte of tile data above it. `read_header`'s walk takes the same cap from the
/// decode's bound for the same tile data, so it holds a bitmap wherever
/// the decode can.
///
/// When there are more, a positional order can pass over precincts, as
/// OpenJPEG's does (see [`PacketWalk::spans`]), so the walk might still
/// finish. The packets emitted are then held as a count per precinct, and
/// each packet skipped is charged [`COUNTS_SKIP_VISITS`]. Each volume emits a
/// precinct's layers from 0 up, so the layers emitted of a precinct are
/// always its first ones, and a packet is one emitted before exactly when
/// its layer is below its precinct's count. The counts take 4 bytes per
/// precinct of the tile, at most 1 MB under the geometry's cap, however
/// many packets the walk reaches: packets past the end of the data, which
/// a walk with `strict` off counts without reading, take no more. (A set of
/// the packets emitted grew by one for each such packet, up to the bound.)
/// `read_header`'s walk keeps no counts: it fails with [`Error::Limit`]
/// there.
pub(super) struct Emitted {
    /// Where each (component, resolution)'s marks start, at `component *
    /// resolutions + resolution`: its first bit in the bitmap, or its first
    /// precinct's count.
    starts: Vec<u64>,
    resolutions: usize,
    marks: Marks,
}

/// How [`Emitted`] holds the packets emitted.
enum Marks {
    /// A bit per packet the volumes reach.
    Dense(Vec<u64>),
    /// The layers emitted of each precinct of the tile.
    Counts(Vec<u32>),
}

impl Emitted {
    /// Nothing emitted yet, for `volumes` over a tile of at most
    /// `resolutions` resolutions per component, with `meter` for the
    /// walk; a bitmap when the volumes reach at most `dense_bits` packets,
    /// else counts if `poc_counts`.
    pub(super) fn new(
        volumes: &[Volume],
        walk: &PacketWalk<'_>,
        resolutions: usize,
        meter: &Meter,
        dense_bits: u64,
        poc_counts: bool,
    ) -> Result<Self> {
        let (mut starts, reached) = Self::layout(volumes, walk, resolutions);
        let words = usize::try_from(reached.div_ceil(64))
            .ok()
            .filter(|_| reached <= dense_bits);
        let marks = match words {
            // A walk that cannot finish is refused up front. With `strict`
            // off it cannot finish either: the packets past the end of the
            // data are counted, a visit each.
            _ if layer_orders(volumes) && reached > meter.left() => {
                return Err(meter.refuse(WorkRefusal::PocReach { reached }));
            }
            Some(words) => Marks::Dense(vec![0; words]),
            // Only the JP2 `Psot = 0` probe walks without counts, and it
            // drops this error, so no caller sees it.
            None if !poc_counts => {
                return Err(Error::limit_fixed(
                    crate::LimitKind::PocVolumes,
                    format!(
                        "tile packet walk exceeds its work bound: its `POC` volumes hold \
                     {reached} packets, past the packet bitmap"
                    ),
                ));
            }
            None => {
                // The geometry holds at most the precincts the sample limit
                // allows a tile across its components (`Cap::tile_precincts`),
                // so the counts take at most 4 bytes for each 2048 samples
                // that limit allows, and 1 MB at its floor.
                let mut precincts = 0u64;
                for (index, start) in starts.iter_mut().enumerate() {
                    let (comp, res) = (index / resolutions, index % resolutions);
                    *start = precincts;
                    if let Some(geom) = walk.geoms[comp].get(res) {
                        precincts += geom.precinct_count() as u64;
                    }
                }
                let precincts = usize::try_from(precincts).map_err(|_| {
                    Error::inconsistent(format!("{precincts} precincts in one tile"))
                })?;
                Marks::Counts(vec![0; precincts])
            }
        };
        Ok(Emitted {
            starts,
            resolutions,
            marks,
        })
    }

    /// Where each (component, resolution)'s bits start, and how many
    /// packets `volumes` reach in all.
    fn layout(volumes: &[Volume], walk: &PacketWalk<'_>, resolutions: usize) -> (Vec<u64>, u64) {
        // First the highest layer end over the volumes that cover each
        // (component, resolution): at most 32 steps per pair, for each of
        // which the geometry has already built far more.
        let mut starts = vec![0u64; walk.geoms.len() * resolutions];
        for volume in volumes {
            for comp in volume.components.clone() {
                let held = walk.geoms[comp].len();
                for res in volume.resolutions.clone().filter(|&r| r < held) {
                    let layers = &mut starts[comp * resolutions + res];
                    *layers = (*layers).max(u64::from(volume.layers));
                }
            }
        }
        // Then each pair's start: the layers times the precincts of the
        // pairs before it. At most 65535 layers times the geometry's 2^18
        // precincts, so the sum cannot overflow.
        let mut reached = 0u64;
        for (index, start) in starts.iter_mut().enumerate() {
            let (comp, res) = (index / resolutions, index % resolutions);
            let layers = std::mem::replace(start, reached);
            if layers > 0 {
                reached += layers * walk.geoms[comp][res].precinct_count() as u64;
            }
        }
        (starts, reached)
    }

    /// The visits a packet skipped as already emitted is charged: none from
    /// the bitmap, [`COUNTS_SKIP_VISITS`] from the counts.
    fn skip_visits(&self) -> usize {
        match self.marks {
            Marks::Dense(_) => 0,
            Marks::Counts(_) => COUNTS_SKIP_VISITS,
        }
    }

    /// Mark packet `(layer, res, comp, precinct)` emitted; whether it was not
    /// already. Kept out of line: inlined into the walk's loop, it slows a
    /// walk of many skips by about 5%.
    #[inline(never)]
    pub(super) fn insert(
        &mut self,
        walk: &PacketWalk<'_>,
        layer: u32,
        res: usize,
        comp: usize,
        precinct: usize,
    ) -> Result<bool> {
        let start = self.starts[comp * self.resolutions + res];
        let precincts = walk.geoms[comp][res].precinct_count() as u64;
        debug_assert!(
            (precinct as u64) < precincts,
            "precinct {precinct} of {precincts}"
        );
        match &mut self.marks {
            Marks::Dense(words) => {
                let bit = start + u64::from(layer) * precincts + precinct as u64;
                let word = usize::try_from(bit / 64)
                    .ok()
                    .and_then(|w| words.get_mut(w))
                    .ok_or_else(|| {
                        Error::inconsistent(format!("packet {bit} past the `POC` bitmap"))
                    })?;
                let mask = 1u64 << (bit % 64);
                let new = *word & mask == 0;
                *word |= mask;
                Ok(new)
            }
            Marks::Counts(counts) => {
                let count = usize::try_from(start + precinct as u64)
                    .ok()
                    .and_then(|at| counts.get_mut(at))
                    .ok_or_else(|| {
                        Error::inconsistent(format!("precinct {precinct} past the `POC` counts"))
                    })?;
                // The layers emitted of a precinct are its first ones (see
                // [`Emitted`]), so the next one new is the count.
                if layer < *count {
                    Ok(false)
                } else if layer == *count {
                    *count += 1;
                    Ok(true)
                } else {
                    Err(Error::inconsistent(format!(
                        "layer {layer} of a precinct emitted before layer {count}"
                    )))
                }
            }
        }
    }
}

/// The packets `volumes` reach over `walk`'s tile, of at most
/// `resolutions` resolutions per component: the bits [`Emitted`] holds.
pub(crate) fn reached(volumes: &[Volume], walk: &PacketWalk<'_>, resolutions: usize) -> u64 {
    Emitted::layout(volumes, walk, resolutions).1
}

/// Whether every one of `volumes` is a layer order, which reads all of a
/// resolution's precincts (see [`Emitted`]).
pub(crate) fn layer_orders(volumes: &[Volume]) -> bool {
    volumes
        .iter()
        .all(|v| matches!(v.progression, Progression::Lrcp | Progression::Rlcp))
}

/// One packet of a tile: the layer, resolution, component and precinct that
/// name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PacketId {
    pub(crate) layer: u32,
    pub(crate) res: usize,
    pub(crate) comp: usize,
    pub(crate) precinct: usize,
}

/// What [`Order::next`] reaches next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// A point of the order's loops, which costs this many visits: one for
    /// the point and one for each (component, resolution) it checks.
    Point(usize),
    /// A packet the order emits at the point it last reached.
    Packet(PacketId),
}

/// The packets [`Order::next_run`] reaches next: every packet left of one
/// (component, resolution) of a point, in the order's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Run {
    /// Layers `layers` of one precinct, in a positional order.
    Layers {
        res: usize,
        comp: usize,
        precinct: usize,
        layers: std::ops::Range<u32>,
    },
    /// Precincts `precincts` of one (layer, resolution, component), in a
    /// layer order.
    Precincts {
        layer: u32,
        res: usize,
        comp: usize,
        precincts: std::ops::Range<usize>,
    },
}

impl Run {
    /// The first packet of the run.
    fn first(&self) -> PacketId {
        match *self {
            Run::Layers {
                res,
                comp,
                precinct,
                ref layers,
            } => PacketId {
                layer: layers.start,
                res,
                comp,
                precinct,
            },
            Run::Precincts {
                layer,
                res,
                comp,
                ref precincts,
            } => PacketId {
                layer,
                res,
                comp,
                precinct: precincts.start,
            },
        }
    }
}

/// A tile's progression: its volumes, and what a walk works out once for
/// all of them. A [`Cursor`] holds where a walk of it is. The default is
/// an order of no volumes, which emits nothing.
#[derive(Default)]
pub(crate) struct Order {
    pub(crate) volumes: Vec<Volume>,
    plan: Plan,
    /// Lists of (component, resolution) pairs that hold a precinct,
    /// component first, then resolution, both ascending. When a PCRL or
    /// CPRL volume spans every resolution, the first is the whole tile's,
    /// which each such volume reads a run of; a volume over fewer
    /// resolutions has its own, shared by the volumes over the same ranges.
    lists: Vec<PairList>,
    /// Per volume, the list in `lists` and the run of it that its pairs
    /// are, for a PCRL or CPRL volume; else an empty run.
    pairs: Vec<ListRun>,
    /// The most resolutions a component of the tile has.
    pub(crate) max_res: usize,
}

/// Where a walk of an [`Order`] is: the volume, the point of its loops, and
/// the packet within the point. It is a few numbers, so a walk can stop
/// after any step and go on from a copy.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Cursor {
    volume: usize,
    /// Whether the point below is the volume's, found by `first_point`.
    started: bool,
    /// The point's outer loop indices: layer and resolution for LRCP,
    /// resolution and layer for RLCP, the resolution for RPCL, and for
    /// CPRL where the component's run of the volume's pairs starts.
    a: usize,
    b: usize,
    /// The point on the reference grid, for the positional orders.
    x: u64,
    y: u64,
    /// The positional orders: the sweep's step, and the range of the list
    /// [`Order::item`] reads that each of its points checks.
    step: (u64, u64),
    sweep: (usize, usize),
    /// Whether the point's charge has been given.
    charged: bool,
    /// The (component, resolution) of the point being emitted from, and
    /// the end of the point's: indices into the list [`Order::item`]
    /// reads.
    item: usize,
    end: usize,
    /// The precinct (layer orders) or layer (positional orders) next.
    inner: usize,
    /// Positional orders: whether `found` is the precinct of `item`.
    entered: bool,
    found: Option<usize>,
}

impl Order {
    /// The bytes the order holds on the heap, capacity included.
    pub(crate) fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        let lists = |outer: usize, inner: usize, item: usize| outer + inner * item;
        let held = lists(
            self.plan.held.capacity() * size_of::<Vec<usize>>(),
            self.plan.held.iter().map(Vec::capacity).sum(),
            size_of::<usize>(),
        );
        let pair_lists = lists(
            self.lists.capacity() * size_of::<PairList>(),
            self.lists.iter().map(Vec::capacity).sum(),
            size_of::<(usize, usize)>(),
        );
        self.volumes.capacity() * size_of::<Volume>()
            + self.plan.spans.capacity() * size_of::<(Option<u64>, Option<u64>)>()
            + held
            + pair_lists
            + self.pairs.capacity() * size_of::<ListRun>()
    }

    /// The order of `header`'s packets over `walk`'s tile.
    pub(crate) fn new(header: &MainHeader, walk: &PacketWalk<'_>) -> Self {
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
        let plan = Plan::new(walk, max_res, &volumes);
        let (lists, pairs) = pair_lists(&plan, components, max_res, &volumes);
        Order {
            volumes,
            plan,
            lists,
            pairs,
            max_res,
        }
    }

    /// The (component, resolution) pairs of volume `volume` that hold a
    /// precinct (see [`Order::lists`]), for a PCRL or CPRL volume; else none.
    #[inline(always)]
    fn pairs(&self, volume: usize) -> &[(usize, usize)] {
        let (list, run) = &self.pairs[volume];
        self.lists.get(*list).map_or(&[], |l| &l[run.clone()])
    }

    /// The step after `cursor`, which it then holds, or `None` at the end of
    /// the order.
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
    /// own starts at that point. The sweep step is minimized over *every* component
    /// and resolution, not just the volume's range (OpenJPEG's `opj_pi_next_rpcl`
    /// does the same in its `first` block), so a `POC` sub-range cannot coarsen it
    /// and skip a corner; only which packets are *emitted* is restricted to the
    /// range.
    ///
    /// Each loop runs over the (component, resolution) pairs of the range that
    /// hold a precinct ([`Plan::held_range`]): the others have no packet, and OpenJPEG's
    /// loops find none there either. A point of the loops is a
    /// [`Step::Point`], charged a step for each pair it checks and one for
    /// itself, before the packets it emits: a (layer, resolution) for LRCP
    /// and RLCP, a (resolution, position) for RPCL, a position for PCRL and
    /// a (component, position) for CPRL.
    pub(crate) fn next(&self, walk: &PacketWalk<'_>, cursor: &mut Cursor) -> Option<Step> {
        loop {
            let volume = self.volumes.get(cursor.volume)?;
            if !cursor.started {
                cursor.started = true;
                cursor.charged = false;
                if !self.first_point(walk, cursor) {
                    cursor.volume += 1;
                    cursor.started = false;
                    continue;
                }
            }
            if !cursor.charged {
                cursor.charged = true;
                (cursor.item, cursor.end) = self.items(cursor);
                cursor.inner = 0;
                cursor.entered = false;
                return Some(Step::Point(1 + cursor.end - cursor.item));
            }
            if let Some(run) = self.next_in_point(walk, volume, cursor, false) {
                return Some(Step::Packet(run.first()));
            }
            cursor.charged = false;
            if !self.next_point(walk, cursor) {
                cursor.volume += 1;
                cursor.started = false;
            }
        }
    }

    /// The packets after `cursor`, which it then holds past, or `None` at
    /// the end of the order: the steps [`Order::next`] takes, with each
    /// point spent on `meter` as it is reached, and the packets of one
    /// (component, resolution) of a point taken at once.
    ///
    /// This is the one-shot decode's walk, so a sweep's points that hold no
    /// packet are stepped over in a loop of their own, as a walk by
    /// [`Order::next`] reaches them one by one.
    #[inline(always)]
    pub(crate) fn next_run(
        &self,
        walk: &PacketWalk<'_>,
        cursor: &mut Cursor,
        meter: &Meter,
    ) -> Result<Option<Run>> {
        loop {
            let Some(volume) = self.volumes.get(cursor.volume) else {
                return Ok(None);
            };
            if !cursor.started {
                cursor.started = true;
                cursor.charged = false;
                if !self.first_point(walk, cursor) {
                    cursor.volume += 1;
                    cursor.started = false;
                    continue;
                }
            }
            if !cursor.charged {
                cursor.charged = true;
                (cursor.item, cursor.end) = self.items(cursor);
                cursor.inner = 0;
                cursor.entered = false;
                meter.spend(1 + cursor.end - cursor.item, WorkRefusal::PacketWalk)?;
            }
            if let Some(run) = self.next_in_point(walk, volume, cursor, true) {
                return Ok(Some(run));
            }
            let positional = !matches!(volume.progression, Progression::Lrcp | Progression::Rlcp);
            if positional && self.sweep_points(walk, volume, cursor, meter)? {
                continue;
            }
            cursor.charged = false;
            let next = if positional {
                self.sweep_done(walk, cursor)
            } else {
                self.next_point(walk, cursor)
            };
            if !next {
                cursor.volume += 1;
                cursor.started = false;
            }
        }
    }

    /// Walk the order from its start, calling `f(layer, resolution,
    /// component, precinct)` for each packet: the steps of
    /// [`Order::next_run`], with the rest of each point taken by
    /// [`Order::finish_point`].
    fn walk_runs(
        &self,
        walk: &PacketWalk<'_>,
        meter: &Meter,
        mut f: impl FnMut(u32, usize, usize, usize) -> Result<()>,
    ) -> Result<()> {
        let mut cursor = Cursor::default();
        while let Some(run) = self.next_run(walk, &mut cursor, meter)? {
            match run {
                Run::Layers {
                    res,
                    comp,
                    precinct,
                    layers,
                } => {
                    for layer in layers {
                        f(layer, res, comp, precinct)?;
                    }
                }
                Run::Precincts {
                    layer,
                    res,
                    comp,
                    precincts,
                } => {
                    for precinct in precincts {
                        f(layer, res, comp, precinct)?;
                    }
                }
            }
            self.finish_point(walk, &mut cursor, &mut f)?;
        }
        Ok(())
    }

    /// Emit with `f` every packet left of the point at `cursor`, after a
    /// run [`Order::next_run`] gave, in the order `next_run` would give
    /// them, and leave `cursor` past them, as `next_run` would.
    ///
    /// This is the one-shot walk's loop within a point. Taking the rest of
    /// a point here, rather than a run at a time through `next_run`, keeps
    /// a point of many (component, resolution) pairs as fast as a nested
    /// loop over them: a run at a time adds about 15% to a walk of 32
    /// `POC` volumes over 16384 components.
    #[inline(always)]
    fn finish_point(
        &self,
        walk: &PacketWalk<'_>,
        cursor: &mut Cursor,
        f: &mut impl FnMut(u32, usize, usize, usize) -> Result<()>,
    ) -> Result<()> {
        let Some(volume) = self.volumes.get(cursor.volume) else {
            return Ok(());
        };
        let end = cursor.end;
        match volume.progression {
            Progression::Lrcp | Progression::Rlcp => {
                let (layer, res) = match volume.progression {
                    Progression::Lrcp => (cursor.a, cursor.b),
                    _ => (cursor.b, cursor.a),
                };
                let comps = &self.plan.held[res][..end];
                let mut inner = cursor.inner;
                for &comp in &comps[cursor.item.min(end)..] {
                    for precinct in inner..walk.geoms[comp][res].precinct_count() {
                        f(layer as u32, res, comp, precinct)?;
                    }
                    inner = 0;
                }
            }
            _ => {
                let layers = volume.layers;
                let (x, y) = (cursor.x, cursor.y);
                let mut start = cursor.item;
                if cursor.entered {
                    if let Some(precinct) = cursor.found {
                        let (comp, res) = self.item(volume, cursor, cursor.item);
                        for layer in cursor.inner as u32..layers {
                            f(layer, res, comp, precinct)?;
                        }
                    }
                    start += 1;
                }
                // The loops are written out, not shared through a closure:
                // a closure here is not inlined, and calling it for each
                // pair costs about a third of the loop.
                if layers > 0 {
                    match volume.progression {
                        Progression::Rpcl => {
                            let res = cursor.a;
                            for &comp in &self.plan.held[res][start.min(end)..end] {
                                if let Some(precinct) = walk.precinct_at(comp, res, x, y) {
                                    for layer in 0..layers {
                                        f(layer, res, comp, precinct)?;
                                    }
                                }
                            }
                        }
                        _ => {
                            for &(comp, res) in &self.pairs(cursor.volume)[start.min(end)..end] {
                                if let Some(precinct) = walk.precinct_at(comp, res, x, y) {
                                    for layer in 0..layers {
                                        f(layer, res, comp, precinct)?;
                                    }
                                }
                            }
                        }
                    }
                }
                cursor.entered = false;
            }
        }
        cursor.item = end;
        cursor.inner = 0;
        Ok(())
    }

    /// Step `cursor`'s sweep on from its point to the next that holds a
    /// packet, spending each point reached on `meter`; `false` if the sweep
    /// ends first. The cursor is then charged at that point, at the first
    /// of its (component, resolution) pairs that has a precinct there.
    #[inline(always)]
    fn sweep_points(
        &self,
        walk: &PacketWalk<'_>,
        volume: &Volume,
        cursor: &mut Cursor,
        meter: &Meter,
    ) -> Result<bool> {
        let (start, end) = cursor.sweep;
        let visits = 1 + end - start;
        let hit = match volume.progression {
            Progression::Rpcl => {
                let res = cursor.a;
                let comps = &self.plan.held[res][start..end];
                sweep_until(walk, cursor, meter, visits, |x, y| {
                    comps.iter().enumerate().find_map(|(i, &comp)| {
                        walk.precinct_at(comp, res, x, y).map(|p| (start + i, p))
                    })
                })?
            }
            _ => {
                let pairs = &self.pairs(cursor.volume)[start..end];
                sweep_until(walk, cursor, meter, visits, |x, y| {
                    pairs.iter().enumerate().find_map(|(i, &(comp, res))| {
                        walk.precinct_at(comp, res, x, y).map(|p| (start + i, p))
                    })
                })?
            }
        };
        let Some((index, precinct)) = hit else {
            return Ok(false);
        };
        cursor.item = index;
        cursor.end = end;
        cursor.inner = 0;
        cursor.entered = true;
        cursor.found = Some(precinct);
        Ok(true)
    }

    /// The (component, resolution) pairs the point at `cursor` checks, as
    /// the range of the list [`Order::item`] reads.
    fn items(&self, cursor: &Cursor) -> (usize, usize) {
        let volume = &self.volumes[cursor.volume];
        match volume.progression {
            Progression::Lrcp => self.plan.held_range(cursor.b, &volume.components),
            Progression::Rlcp => self.plan.held_range(cursor.a, &volume.components),
            Progression::Rpcl | Progression::Pcrl | Progression::Cprl => cursor.sweep,
        }
    }

    /// Item `index` of the point at `cursor`, as (component, resolution),
    /// for an `index` in the range [`Order::items`] gives.
    #[inline(always)]
    fn item(&self, volume: &Volume, cursor: &Cursor, index: usize) -> (usize, usize) {
        match volume.progression {
            Progression::Lrcp => (self.plan.held[cursor.b][index], cursor.b),
            Progression::Rlcp | Progression::Rpcl => (self.plan.held[cursor.a][index], cursor.a),
            Progression::Pcrl | Progression::Cprl => self.pairs(cursor.volume)[index],
        }
    }

    /// The next packet of the point at `cursor`, or if `whole` every packet
    /// left of the next (component, resolution) of the point that has one,
    /// if any is left.
    #[inline(always)]
    fn next_in_point(
        &self,
        walk: &PacketWalk<'_>,
        volume: &Volume,
        cursor: &mut Cursor,
        whole: bool,
    ) -> Option<Run> {
        if !matches!(volume.progression, Progression::Lrcp | Progression::Rlcp) {
            return self.next_at_position(walk, volume, cursor, whole);
        }
        let res = match volume.progression {
            Progression::Lrcp => cursor.b,
            _ => cursor.a,
        };
        let layer = match volume.progression {
            Progression::Lrcp => cursor.a,
            _ => cursor.b,
        } as u32;
        while cursor.item < cursor.end {
            let comp = self.plan.held[res][cursor.item];
            let precincts = walk.geoms[comp][res].precinct_count();
            if cursor.inner < precincts {
                let start = cursor.inner;
                cursor.inner = if whole { precincts } else { start + 1 };
                return Some(Run::Precincts {
                    layer,
                    res,
                    comp,
                    precincts: start..cursor.inner,
                });
            }
            cursor.inner = 0;
            cursor.item += 1;
        }
        None
    }

    /// [`Order::next_in_point`] for a positional order: the layers of the
    /// precinct entered, then of the next (component, resolution) of the
    /// point with a precinct there.
    #[inline(always)]
    fn next_at_position(
        &self,
        walk: &PacketWalk<'_>,
        volume: &Volume,
        cursor: &mut Cursor,
        whole: bool,
    ) -> Option<Run> {
        let layers = volume.layers as usize;
        if cursor.entered {
            if let Some(precinct) = cursor.found.filter(|_| cursor.inner < layers) {
                let (comp, res) = self.item(volume, cursor, cursor.item);
                let start = cursor.inner;
                cursor.inner = if whole { layers } else { start + 1 };
                return Some(Run::Layers {
                    res,
                    comp,
                    precinct,
                    layers: start as u32..cursor.inner as u32,
                });
            }
            cursor.entered = false;
            cursor.item += 1;
        }
        if layers == 0 {
            cursor.item = cursor.end;
            return None;
        }
        let (comp, res, precinct) = self.find(walk, volume, cursor, cursor.item)?;
        cursor.entered = true;
        cursor.found = Some(precinct);
        cursor.inner = if whole { layers } else { 1 };
        Some(Run::Layers {
            res,
            comp,
            precinct,
            layers: 0..cursor.inner as u32,
        })
    }

    /// The first of the point's (component, resolution) pairs from `index`
    /// on that has a precinct at the point, with that precinct; `cursor`
    /// is left on it, or past the pairs if none has.
    #[inline(always)]
    fn find(
        &self,
        walk: &PacketWalk<'_>,
        volume: &Volume,
        cursor: &mut Cursor,
        index: usize,
    ) -> Option<(usize, usize, usize)> {
        let (x, y, end) = (cursor.x, cursor.y, cursor.end);
        let found = match volume.progression {
            Progression::Rpcl => {
                let res = cursor.a;
                let comps = &self.plan.held[res][..end];
                (index..end).find_map(|i| {
                    let comp = comps[i];
                    walk.precinct_at(comp, res, x, y).map(|p| (i, comp, res, p))
                })
            }
            _ => {
                let pairs = &self.pairs(cursor.volume)[..end];
                (index..end).find_map(|i| {
                    let (comp, res) = pairs[i];
                    walk.precinct_at(comp, res, x, y).map(|p| (i, comp, res, p))
                })
            }
        };
        match found {
            Some((i, comp, res, precinct)) => {
                cursor.item = i;
                Some((comp, res, precinct))
            }
            None => {
                cursor.item = end;
                None
            }
        }
    }

    /// Put `cursor` on the first point of its volume; `false` if it has none.
    fn first_point(&self, walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
        let volume = &self.volumes[cursor.volume];
        let layers = volume.layers as usize;
        let res = &volume.resolutions;
        match volume.progression {
            Progression::Lrcp => {
                (cursor.a, cursor.b) = (0, res.start);
                layers > 0 && !res.is_empty()
            }
            Progression::Rlcp => {
                (cursor.a, cursor.b) = (res.start, 0);
                layers > 0 && !res.is_empty()
            }
            Progression::Rpcl => {
                if self.plan.step.is_none() {
                    return false;
                }
                cursor.a = res.start;
                self.rpcl_resolution(walk, cursor)
            }
            Progression::Pcrl => {
                let Some(step) = self.plan.step else {
                    return false;
                };
                cursor.step = step;
                cursor.sweep = (0, self.pairs(cursor.volume).len());
                cursor.sweep.1 > 0 && sweep_first(walk, cursor)
            }
            Progression::Cprl => {
                cursor.a = 0;
                self.cprl_group(walk, cursor)
            }
        }
    }

    /// Put `cursor` on the point after the one it holds in its volume;
    /// `false` past the volume's last.
    fn next_point(&self, walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
        let volume = &self.volumes[cursor.volume];
        let layers = volume.layers as usize;
        let res = &volume.resolutions;
        match volume.progression {
            Progression::Lrcp => {
                cursor.b += 1;
                if cursor.b < res.end {
                    return true;
                }
                cursor.b = res.start;
                cursor.a += 1;
                cursor.a < layers
            }
            Progression::Rlcp => {
                cursor.b += 1;
                if cursor.b < layers {
                    return true;
                }
                cursor.b = 0;
                cursor.a += 1;
                cursor.a < res.end
            }
            Progression::Rpcl | Progression::Pcrl | Progression::Cprl => {
                sweep_next(walk, cursor) || self.sweep_done(walk, cursor)
            }
        }
    }

    /// Put `cursor`, whose sweep has passed its last point, on the first
    /// point of the next sweep of its positional volume; `false` if none
    /// is left.
    fn sweep_done(&self, walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
        match self.volumes[cursor.volume].progression {
            Progression::Rpcl => {
                cursor.a += 1;
                self.rpcl_resolution(walk, cursor)
            }
            Progression::Cprl => {
                cursor.a = cursor.sweep.1;
                self.cprl_group(walk, cursor)
            }
            _ => false,
        }
    }

    /// RPCL: put `cursor` on the first point of resolution `cursor.a` or
    /// the first after it that a component of the volume holds a precinct
    /// at; `false` if none is left. A resolution no component of the range
    /// holds a precinct at has no packet, so it is not swept.
    fn rpcl_resolution(&self, walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
        let volume = &self.volumes[cursor.volume];
        let Some(step) = self.plan.step else {
            return false;
        };
        cursor.step = step;
        while cursor.a < volume.resolutions.end {
            cursor.sweep = self.plan.held_range(cursor.a, &volume.components);
            if cursor.sweep.1 > cursor.sweep.0 && sweep_first(walk, cursor) {
                return true;
            }
            cursor.a += 1;
        }
        false
    }

    /// CPRL: put `cursor` on the first point of the component whose run of
    /// the volume's pairs starts at `cursor.a`, or of the first after it
    /// that has a sweep step; `false` if
    /// none is left. Component outermost, so each gets its own sweep — and
    /// its own step, the finest precinct over that component's resolutions
    /// alone. This is where CPRL parts company with PCRL: PCRL interleaves
    /// the components inside one shared sweep, CPRL finishes a component
    /// before starting the next.
    fn cprl_group(&self, walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
        let pairs = self.pairs(cursor.volume);
        while let Some(&(comp, _)) = pairs.get(cursor.a) {
            // A component's run of pairs is at most its 33 resolutions.
            let end = cursor.a
                + pairs[cursor.a..]
                    .iter()
                    .take_while(|&&(c, _)| c == comp)
                    .count();
            cursor.sweep = (cursor.a, end);
            if let Some(step) = both(self.plan.spans[comp]) {
                cursor.step = step;
                if sweep_first(walk, cursor) {
                    return true;
                }
            }
            cursor.a = end;
        }
        false
    }
}

/// Step `cursor`'s sweep of `walk`'s tile on from its point, spending
/// `visits` on `meter` at each point, until `probe` finds a packet there;
/// what it found, or `None` past the sweep's last point.
#[inline(always)]
fn sweep_until(
    walk: &PacketWalk<'_>,
    cursor: &mut Cursor,
    meter: &Meter,
    visits: usize,
    mut probe: impl FnMut(u64, u64) -> Option<(usize, usize)>,
) -> Result<Option<(usize, usize)>> {
    let (step_x, step_y) = cursor.step;
    let (tx0, tx1) = (u64::from(walk.tile.x0), u64::from(walk.tile.x1));
    let ty1 = u64::from(walk.tile.y1);
    let (mut x, mut y) = (cursor.x, cursor.y);
    let hit = loop {
        x += step_x - (x % step_x);
        if x >= tx1 {
            x = tx0;
            y += step_y - (y % step_y);
            if y >= ty1 {
                break Ok(None);
            }
        }
        if let Err(e) = meter.spend(visits, WorkRefusal::PacketWalk) {
            break Err(e);
        }
        if let Some(hit) = probe(x, y) {
            break Ok(Some(hit));
        }
    };
    (cursor.x, cursor.y) = (x, y);
    hit
}

/// Put `cursor` on the first point of a sweep of `walk`'s tile; `false` if
/// the tile has none.
fn sweep_first(walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
    cursor.x = u64::from(walk.tile.x0);
    cursor.y = u64::from(walk.tile.y0);
    cursor.x < u64::from(walk.tile.x1) && cursor.y < u64::from(walk.tile.y1)
}

/// Move `cursor` to the next point of a sweep of `walk`'s tile in steps of
/// its `step`, rows of `x` within columns of `y`; `false` past the last.
fn sweep_next(walk: &PacketWalk<'_>, cursor: &mut Cursor) -> bool {
    let (step_x, step_y) = cursor.step;
    cursor.x += step_x - (cursor.x % step_x);
    if cursor.x < u64::from(walk.tile.x1) {
        return true;
    }
    cursor.x = u64::from(walk.tile.x0);
    cursor.y += step_y - (cursor.y % step_y);
    cursor.y < u64::from(walk.tile.y1)
}
