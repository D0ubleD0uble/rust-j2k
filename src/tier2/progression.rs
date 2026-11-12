//! Progression-order packet iteration for Tier-2 (ISO/IEC 15444-1 B.12.1).
//!
//! Enumerates a tile's packets in the order its `COD` — or a `POC` marker's
//! sequence of progression volumes (A.6.6) — prescribes, calling back once per
//! (layer, resolution, component, precinct). The three positional orders sweep
//! the reference grid rather than counting, so this module needs the canvas
//! coordinates the per-component [`ResolutionGeom`] has already divided away.

use std::cell::Cell;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

use crate::codestream::MainHeader;
use crate::codestream::markers::{PocVolume, Progression, Rect, Siz};
use crate::{Error, Result};

use super::geometry::ResolutionGeom;
use super::visit;

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
            "precinct index ({i}, {j}) out of the {}×{} grid",
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
struct Volume {
    progression: Progression,
    /// Layers `0..layers` (a `POC` volume's layer range always starts at 0).
    layers: u32,
    resolutions: std::ops::Range<usize>,
    components: std::ops::Range<usize>,
}

/// What a walk works out once for all of its volumes, rather than once per
/// volume.
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

    /// The components of `range` that hold a precinct at resolution `res`.
    fn held(&self, res: usize, range: &std::ops::Range<usize>) -> &[usize] {
        let comps = &self.held[res];
        let start = comps.partition_point(|&c| c < range.start);
        let end = comps.partition_point(|&c| c < range.end);
        &comps[start..end.max(start)]
    }

    /// The (component, resolution) pairs of `volume` that hold a precinct,
    /// component first, then resolution, both ascending.
    fn pairs(&self, volume: &Volume) -> Vec<(usize, usize)> {
        let mut pairs: Vec<(usize, usize)> = volume
            .resolutions
            .clone()
            .flat_map(|res| {
                self.held(res, &volume.components)
                    .iter()
                    .map(move |&c| (c, res))
            })
            .collect();
        pairs.sort_unstable();
        pairs
    }
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
/// The refusal relies on strict decoding; partial decode (#214) must revisit
/// it, as the packets left out are missing data like a cut tile-part.
pub(crate) fn check_poc_coverage(header: &MainHeader, walk: &PacketWalk<'_>) -> Result<()> {
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
                return Err(Error::Codestream(format!(
                    "the tile's POC volumes leave out its packets of component {comp}, \
                     resolution {res}, from layer {reached} of {layers}"
                )));
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
/// Each volume is enumerated by [`run_order`]. The five orders are permutations
/// of the four axes — layer, resolution, component, position — and the three
/// positional ones sweep the reference grid; see that function.
///
/// The loops spend `visits` (see [`super::Budget::visits`]) as they go, the
/// points that hold no packet included, so a walk under a bound stops however
/// sparse its packets are.
///
/// `poc_bitmap` is how a `POC` sequence's packets are held (see
/// [`Emitted`]): with `None`, in a bitmap when they are no more than the
/// visits left and else in a set; with `Some(bits)`, in a bitmap when they
/// are no more than `bits`, and else the walk is [`crate::Error::Limit`].
pub(crate) fn for_each_packet<F>(
    header: &MainHeader,
    walk: &PacketWalk<'_>,
    visits: &Cell<u64>,
    poc_bitmap: Option<u64>,
    f: F,
) -> Result<()>
where
    F: FnMut(u32, usize, usize, usize) -> Result<()>,
{
    let dense_bits = poc_bitmap.unwrap_or(visits.get());
    for_each_packet_holding(header, walk, visits, dense_bits, poc_bitmap.is_none(), f)
}

/// [`for_each_packet`], with a `POC` sequence's packets held in a bitmap
/// only when its volumes reach at most `dense_bits` of them (see
/// [`Emitted`]).
pub(super) fn for_each_packet_holding<F>(
    header: &MainHeader,
    walk: &PacketWalk<'_>,
    visits: &Cell<u64>,
    dense_bits: u64,
    poc_set: bool,
    mut f: F,
) -> Result<()>
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

    let plan = Plan::new(walk, max_res, &volumes);

    // With one volume every tuple is visited once, so the dedup is pure overhead
    // and skipped; a `POC` sequence needs it, because volumes may overlap.
    if volumes.len() == 1 {
        return run_order(&volumes[0], walk, &plan, visits, &mut f);
    }
    let mut emitted = Emitted::new(&volumes, walk, max_res, visits.get(), dense_bits, poc_set)?;
    let skip_visits = emitted.skip_visits();
    for volume in &volumes {
        run_order(
            volume,
            walk,
            &plan,
            visits,
            &mut |layer, res, comp, precinct| {
                // A packet skipped from the bitmap is not charged. Only a packet
                // read is marked, and a read takes a visit and at least a byte of
                // packet header; at most 31 later volumes can meet it again. A
                // bitmap skip takes about 4 ns, so the skips cost at most about
                // 0.12 µs per byte, under the 0.25 to 0.47 µs a byte of real
                // data takes to decode that the bound is set by. Charging them would refuse
                // files OpenJPEG reads quickly, such as 31 identical volumes over
                // 1 MB of empty packets. A skip from the set costs far more and
                // is charged (see `SET_SKIP_VISITS`).
                if emitted.insert(walk, layer, res, comp, precinct)? {
                    f(layer, res, comp, precinct)
                } else if skip_visits > 0 {
                    visit(visits, skip_visits)
                } else {
                    Ok(())
                }
            },
        )?;
    }
    Ok(())
}

/// The visits a packet skipped from [`Emitted`]'s set is charged. A lookup
/// in a set of millions of packets misses the cache: about 80 to 180 ns,
/// measured on the `setskip` shape of 1.4e7 packets read, against about
/// 10 ns for a visit there (the bound is set by 14 ns), so 12 visits.
const SET_SKIP_VISITS: usize = 12;

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
/// visits left. Each packet read spends a visit, so a walk that reads them
/// all never has more. The bitmap is then at most a bit per visit: 2 MB
/// under the decode's floor of 2^24 visits, and 4 bytes per byte of tile
/// data above it. `read_header`'s walk takes the same cap from the
/// decode's bound for the same tile data, so it holds a bitmap wherever
/// the decode can.
///
/// When there are more, a positional order can pass over precincts, as
/// OpenJPEG's does (see [`PacketWalk::spans`]), so the walk might still
/// finish; the packets read are then kept in a set, at most one per visit
/// and one per byte of packet headers, about 24 bytes each, and each packet
/// skipped from it is charged [`SET_SKIP_VISITS`]. `read_header`'s walk
/// keeps no set: it fails with [`Error::Limit`] there.
struct Emitted {
    /// The first bit of each (component, resolution), at
    /// `component * resolutions + resolution`.
    starts: Vec<u64>,
    resolutions: usize,
    marks: Marks,
}

/// How [`Emitted`] holds its bits.
enum Marks {
    Dense(Vec<u64>),
    Sparse(HashSet<u64, BuildHasherDefault<PacketHasher>>),
}

impl Emitted {
    /// Nothing emitted yet, for `volumes` over a tile of at most
    /// `resolutions` resolutions per component, with `visits_left` for the
    /// walk; a bitmap when the volumes reach at most `dense_bits` packets,
    /// else a set if `poc_set`.
    fn new(
        volumes: &[Volume],
        walk: &PacketWalk<'_>,
        resolutions: usize,
        visits_left: u64,
        dense_bits: u64,
        poc_set: bool,
    ) -> Result<Self> {
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
        let words = usize::try_from(reached.div_ceil(64))
            .ok()
            .filter(|_| reached <= dense_bits);
        let layer_orders = volumes
            .iter()
            .all(|v| matches!(v.progression, Progression::Lrcp | Progression::Rlcp));
        let marks = match words {
            // Refusing a walk that cannot finish relies on strict decoding;
            // revisit it for partial decode (#214).
            _ if layer_orders && reached > visits_left => {
                return Err(Error::Limit(format!(
                    "tile packet walk exceeds its work bound: its POC volumes reach \
                     {reached} packets"
                )));
            }
            Some(words) => Marks::Dense(vec![0; words]),
            None if !poc_set => {
                return Err(Error::Limit(format!(
                    "tile packet walk exceeds its work bound: its POC volumes reach \
                     {reached} packets"
                )));
            }
            None => Marks::Sparse(HashSet::default()),
        };
        Ok(Emitted {
            starts,
            resolutions,
            marks,
        })
    }

    /// The visits a packet skipped as already emitted is charged: none from
    /// the bitmap, [`SET_SKIP_VISITS`] from the set.
    fn skip_visits(&self) -> usize {
        match self.marks {
            Marks::Dense(_) => 0,
            Marks::Sparse(_) => SET_SKIP_VISITS,
        }
    }

    /// Mark packet `(layer, res, comp, precinct)` emitted; whether it was not
    /// already.
    fn insert(
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
        let bit = start + u64::from(layer) * precincts + precinct as u64;
        match &mut self.marks {
            Marks::Dense(words) => {
                let word = usize::try_from(bit / 64)
                    .ok()
                    .and_then(|w| words.get_mut(w))
                    .ok_or_else(|| {
                        Error::Inconsistent(format!("packet {bit} past the POC bitmap"))
                    })?;
                let mask = 1u64 << (bit % 64);
                let new = *word & mask == 0;
                *word |= mask;
                Ok(new)
            }
            Marks::Sparse(set) => Ok(set.insert(bit)),
        }
    }
}

/// The hash of a packet's bit in [`Emitted`]'s set: SplitMix64's finalizer,
/// which spreads neighboring bits over every bucket. It is a bijection, so
/// two packets never hash alike, and the bits are laid out by the tile's
/// geometry, not chosen one by one.
#[derive(Default)]
struct PacketHasher(u64);

impl Hasher for PacketHasher {
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8) | u64::from(b);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
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
/// own starts at that point. The sweep step is minimized over *every* component
/// and resolution, not just the volume's range (OpenJPEG's `opj_pi_next_rpcl`
/// does the same in its `first` block), so a `POC` sub-range cannot coarsen it
/// and skip a corner; only which packets are *emitted* is restricted to the
/// range.
///
/// Each loop runs over the (component, resolution) pairs of the range that
/// hold a precinct ([`Plan::held`]): the others have no packet, and OpenJPEG's
/// loops find none there either. A step is charged for each pair a loop
/// point checks, and one for the point itself.
fn run_order<F>(
    volume: &Volume,
    walk: &PacketWalk<'_>,
    plan: &Plan,
    visits: &Cell<u64>,
    emit: &mut F,
) -> Result<()>
where
    F: FnMut(u32, usize, usize, usize) -> Result<()>,
{
    let layers = volume.layers;
    let res_range = volume.resolutions.clone();
    let comp_range = &volume.components;
    let precincts = |comp: usize, res: usize| walk.geoms[comp][res].precinct_count();

    let (tx0, ty0) = (u64::from(walk.tile.x0), u64::from(walk.tile.y0));
    let (tx1, ty1) = (u64::from(walk.tile.x1), u64::from(walk.tile.y1));

    match volume.progression {
        Progression::Lrcp => {
            for layer in 0..layers {
                for res in res_range.clone() {
                    let comps = plan.held(res, comp_range);
                    visit(visits, 1 + comps.len())?;
                    for &comp in comps {
                        for precinct in 0..precincts(comp, res) {
                            emit(layer, res, comp, precinct)?;
                        }
                    }
                }
            }
        }
        Progression::Rlcp => {
            for res in res_range.clone() {
                let comps = plan.held(res, comp_range);
                for layer in 0..layers {
                    visit(visits, 1 + comps.len())?;
                    for &comp in comps {
                        for precinct in 0..precincts(comp, res) {
                            emit(layer, res, comp, precinct)?;
                        }
                    }
                }
            }
        }
        // Resolution outermost, then the positional sweep, then component, then
        // layer. A resolution no component of the range holds a precinct at
        // has no packet, so it is not swept.
        Progression::Rpcl => {
            let Some((step_x, step_y)) = plan.step else {
                return Ok(());
            };
            for res in res_range.clone() {
                let comps = plan.held(res, comp_range);
                if comps.is_empty() {
                    continue;
                }
                let mut y = ty0;
                while y < ty1 {
                    let mut x = tx0;
                    while x < tx1 {
                        visit(visits, 1 + comps.len())?;
                        for &comp in comps {
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
            let Some((step_x, step_y)) = plan.step else {
                return Ok(());
            };
            let pairs = plan.pairs(volume);
            if pairs.is_empty() {
                return Ok(());
            }
            let mut y = ty0;
            while y < ty1 {
                let mut x = tx0;
                while x < tx1 {
                    visit(visits, 1 + pairs.len())?;
                    for &(comp, res) in &pairs {
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
        // Component outermost, so each gets its own sweep — and its own step, the
        // finest precinct over that component's resolutions alone. This is where
        // CPRL parts company with PCRL: PCRL interleaves the components inside one
        // shared sweep, CPRL finishes a component before starting the next.
        Progression::Cprl => {
            let pairs = plan.pairs(volume);
            for group in pairs.chunk_by(|a, b| a.0 == b.0) {
                let comp = group[0].0;
                let Some((step_x, step_y)) = both(plan.spans[comp]) else {
                    continue;
                };
                let mut y = ty0;
                while y < ty1 {
                    let mut x = tx0;
                    while x < tx1 {
                        visit(visits, 1 + group.len())?;
                        for &(_, res) in group {
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
