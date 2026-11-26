//! Tile packet walks kept across the feeds of an incremental decode.
//!
//! A decode with `strict` off of a prefix of a file reads each tile's
//! packets until its data ends, then walks the rest of the progression to
//! note the packets not read (see [`super::walk_packets`]). Each walk here
//! does the first part once over the whole of the file: it reads each
//! packet once the data holds all of it, and keeps where it stopped. A
//! packet the data cuts is read as far as it goes for the image of that
//! prefix, then put back, and read again once more data comes. The packets
//! after it are counted once, by a second walk of the order that reads no
//! data. From the two, each decode of a prefix gets the coded data and
//! the [`Reach`] a decode of that prefix on its own gets, and is charged
//! the visits that decode would spend, so that it refuses the work it
//! refuses.

use std::cell::Cell;

use crate::codestream::{MainHeader, Tile};
use crate::tier2::Rect;
use crate::{Error, Result};

use super::progression::{
    COUNTS_SKIP_VISITS, Cursor, Order, PacketId, Step, layer_orders, poc_gaps, reached,
};
use super::{
    BandState, Budget, CodedData, ComponentCoded, Delimiters, END_VISITS, Journal, Meter, Packet,
    PacketStreams, PacketWalk, Reach, Refused, Resolution, ResolutionGeom, Scratch, Undo,
    WorkRefusal, band_states, build_subbands, dropped_blocks, empty_components, mark_gap,
    parse_packet, refused, tile_geometry, undo,
};

/// The bytes the walks kept may hold whatever the bytes fed, when that is
/// more than [`KEPT_PER_BYTE`] bytes for each byte fed (see [`kept_room`]).
const KEPT_MIN: u64 = 1 << 20;

/// The bytes the walks kept may hold per byte fed, when that comes to more
/// than [`KEPT_MIN`]. Of the committed fixtures fed 1 KB at a time, this
/// keeps fewer walks than the image bound alone only for files made to test
/// the work bounds, and for `sample_limit/rgb_6000x4000.jp2` until 64 KB of
/// its 148 KB are in.
const KEPT_PER_BYTE: u64 = 32;

/// The bytes the walks kept may hold after `fed` bytes have been fed: the
/// larger of [`KEPT_MIN`] and [`KEPT_PER_BYTE`] bytes per byte fed.
pub(crate) fn kept_room(fed: u64) -> u64 {
    KEPT_MIN.max(fed.saturating_mul(KEPT_PER_BYTE))
}

/// How many times a decode's packet bound the walks that read a tile from
/// the start again (of tiles not kept, and walks started or built again)
/// may spend over all the decodes of an incremental decoder (see
/// [`Walks::walk`]). Of the committed fixtures fed in quarters, in pieces
/// of random sizes and one byte at a time, normal files spend at most
/// 0.008 of a bound on these walks, and files made to reach the work bounds
/// at most 3.
pub(crate) const REPEAT_BOUNDS: u64 = 4;

/// The packet walks of an incremental decode's tiles, by tile index.
///
/// The walks kept hold no more than about twice the decode's image plus
/// the largest tile walk so far under the current layer cap. They also hold
/// no more than [`kept_room`] of the bytes fed, so that a short file that
/// declares a large image holds little between decodes. A walk's size
/// is every heap allocation it holds, capacity included (see
/// [`TileWalk::bytes`]), and a walk that has read all its packets lets go
/// of its order, tag trees, counts and buffers. A tile whose walk would
/// pass either bound is not kept: each decode walks its packets from the
/// start, as a one-shot decode does, until the bytes fed make room for it.
/// Those walks are charged over all the decodes (see [`Walks::walk`]).
///
/// The walks do not decide when a refine decodes: the decoder tries on
/// the first refine with at least one byte fed, then at each doubling of
/// the bytes fed since its last try, whether that try gave an image or was
/// refused.
#[derive(Default)]
pub(crate) struct Walks {
    tiles: Vec<Slot>,
    /// The visits of the walks kept and of those let go (see
    /// [`Walks::visits`]).
    #[cfg(test)]
    visits: u64,
    /// The bytes the walks kept hold, as [`TileWalk::bytes`] counts them.
    kept: u64,
    /// The most bytes one tile's walk has held under the layer cap the walks
    /// were built for, kept or not.
    largest: u64,
    /// The reduction and the layer cap the walks were built for (see
    /// [`Walks::prepare`]).
    built_for: Option<(u8, Option<u16>)>,
}

/// What is kept of one tile's walk.
#[derive(Default)]
enum Slot {
    /// Nothing: the tile has had no data yet.
    #[default]
    Empty,
    /// The walk, and the bytes it holds.
    Kept(Box<TileWalk>, u64),
    /// A walk too large to keep, of `bytes`, for the tile's header as it
    /// was (see [`TileWalk::volumes`]).
    Unkept {
        bytes: u64,
        volumes: usize,
        packed: bool,
    },
    /// No walk, though an earlier one read the tile's packets: it was let
    /// go when the layer cap changed, when the reduction changed and its
    /// geometry had failed, or to make room for the tiles a window reads
    /// (see [`Walks::prepare`]). The next walk is built again, and charged
    /// as a walk of a tile not kept.
    Forgotten,
}

// At most 24 bytes, as the docs state: 24 on 64-bit targets, less on wasm32,
// so the bound is `<=`, not `==`.
const _: () = assert!(std::mem::size_of::<Slot>() <= 24);

impl Walks {
    /// Make the walks fit a decode of `fed` bytes that reads the tiles
    /// `reads` flags, under `reduction` and `layers`, and whose image is of
    /// `image` bytes. Called before the decode's first walk.
    ///
    /// A walk reads its tile's packets under one layer cap, which decides
    /// the coding passes it keeps, so when the cap has changed since the
    /// walks were built, every walk is let go, and built again by the next
    /// walk of its tile. The reduction decides only the code-blocks a walk
    /// charges for the resolutions it drops (see [`TileWalk::reduce`]): when
    /// it alone has changed, each walk kept is kept, and charges those of
    /// the new reduction from now on. A walk whose geometry failed holds no
    /// geometry to count them from, so it is let go. A window changes
    /// neither: the walks of the tiles it does not read are kept as
    /// they are, unless the walks kept are past the room of this decode,
    /// whose image is smaller. Then the walks of the tiles it does not read
    /// are let go, from the first tile, until the rest fit. A walk let go
    /// here is charged as a walk of a tile not kept when it is built again
    /// (see [`Walks::walk`]), so changing the options back and forth
    /// between decodes is held to the same bound as walking a tile again.
    /// The largest walk is reset only when every walk is let go. A change of
    /// reduction or window keeps `largest`: a window may let some walks go
    /// to fit the room, but the room never falls below what one decode
    /// holds.
    ///
    /// Under the same options on every decode, this changes nothing: the
    /// room only grows with the bytes fed.
    pub(crate) fn prepare(
        &mut self,
        reads: &[bool],
        reduction: u8,
        layers: Option<u16>,
        image: u64,
        fed: u64,
    ) {
        match self.built_for {
            Some((built, cap)) if cap == layers => {
                if built != reduction {
                    for index in 0..self.tiles.len() {
                        let reduced = match &mut self.tiles[index] {
                            Slot::Kept(walk, _) => walk.reduce(reduction),
                            _ => true,
                        };
                        if !reduced {
                            self.forget(index);
                        }
                    }
                }
            }
            _ => {
                for index in 0..self.tiles.len() {
                    self.forget(index);
                }
                // No walk of the old cap is left, so the largest of them
                // no longer sets the room. The walks built now count.
                self.largest = 0;
            }
        }
        self.built_for = Some((reduction, layers));
        let room = self.room(image, fed);
        for index in 0..self.tiles.len() {
            if self.kept <= room {
                break;
            }
            let read = reads.get(index).copied().unwrap_or(false);
            if !read && matches!(self.tiles[index], Slot::Kept(..)) {
                self.forget(index);
            }
        }
    }

    /// The bytes the walks kept may hold in a decode of `fed` bytes whose
    /// image is of `image` bytes.
    fn room(&self, image: u64, fed: u64) -> u64 {
        image
            .saturating_add(self.largest)
            .saturating_mul(2)
            .min(kept_room(fed))
    }

    /// Let go of the walk of tile `index`, if it has had one, so that its
    /// next walk is built again and charged (see [`Slot::Forgotten`]).
    fn forget(&mut self, index: usize) {
        if matches!(self.tiles[index], Slot::Kept(..) | Slot::Unkept { .. }) {
            self.let_go(index);
            self.tiles[index] = Slot::Forgotten;
        }
    }

    /// What a decode with `strict` off of the bytes `tile` holds gets from
    /// its packets, under `header`, its resolved header: the coded data,
    /// and what was not read. `meter` is charged what that decode's walk
    /// would charge it, and the work bound refuses this walk where it
    /// refuses that one. `poc_bitmap`, `reduction` and `layers` are as
    /// [`super::decode_packets_partial`] takes them; `reduction` and
    /// `layers` must be those the walks were built for, which
    /// [`Walks::prepare`] sets before each decode. `image` is the bytes of
    /// the image the decode returns, and `fed` the bytes the decode is of.
    ///
    /// A tile whose walk is not kept is walked from the start on each
    /// decode, as a one-shot decode walks it. So is a tile whose walk is
    /// started again, or built again once there is room for it, on the
    /// first decode of the new walk. `repeats` holds the visits those walks
    /// have spent over all the decodes, and this one's are added to it.
    /// So is the first walk of a tile whose walk [`Walks::prepare`] let
    /// go. The first walk of a tile kept from its start is not charged.
    /// Once they are past [`REPEAT_BOUNDS`] times the packet bound of this
    /// decode (`meter`'s first visits), the walk is refused
    /// for work, as a walk past the bound is. So the decodes together spend
    /// on these walks at most that many bounds, plus the one walk that went
    /// past.
    ///
    /// A tile whose later tile-part adds `POC` volumes, or packed packet
    /// headers where it had none, has another order of packets, or reads
    /// them from elsewhere: its walk is started again.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn walk<'t>(
        &mut self,
        header: &MainHeader,
        tile: &'t Tile<'t>,
        meter: &Meter,
        poc_bitmap: u64,
        reduction: u8,
        layers: Option<u16>,
        image: u64,
        fed: u64,
        repeats: &Repeats,
    ) -> std::result::Result<(CodedData<'t>, Reach), Refused> {
        let index = tile.index as usize;
        if self.tiles.len() <= index {
            self.tiles.resize_with(index + 1, Slot::default);
        }
        let volumes = header.poc.len();
        let packed = tile.packed;
        // Whether an earlier walk of the tile has read its packets, so a
        // walk built now reads them again.
        let walked_before = !matches!(self.tiles[index], Slot::Empty);
        let changed = match &self.tiles[index] {
            Slot::Empty | Slot::Forgotten => false,
            Slot::Kept(walk, _) => walk.volumes != volumes || walk.packed != packed,
            Slot::Unkept {
                volumes: v,
                packed: p,
                ..
            } => *v != volumes || *p != packed,
        };
        if changed {
            self.let_go(index);
            self.tiles[index] = Slot::Empty;
        }
        let fits =
            |walks: &Walks, bytes: u64| walks.kept.saturating_add(bytes) <= walks.room(image, fed);
        // Whether the walk is new, so none of its visits is counted yet.
        #[cfg(test)]
        let fresh = !matches!(self.tiles[index], Slot::Kept(..));
        match std::mem::take(&mut self.tiles[index]) {
            Slot::Kept(walk, bytes) => self.tiles[index] = Slot::Kept(walk, bytes),
            Slot::Empty | Slot::Forgotten => {
                let mut walk = TileWalk::new(header, tile, reduction, layers, meter.cap());
                walk.restarted = walked_before;
                let bytes = walk.bytes();
                self.largest = self.largest.max(bytes);
                self.tiles[index] = if fits(self, bytes) {
                    self.kept += bytes;
                    Slot::Kept(Box::new(walk), bytes)
                } else {
                    Slot::Unkept {
                        bytes,
                        volumes,
                        packed,
                    }
                };
            }
            Slot::Unkept { bytes, .. } if fits(self, bytes) => {
                let mut walk = TileWalk::new(header, tile, reduction, layers, meter.cap());
                walk.restarted = true;
                let bytes = walk.bytes();
                self.kept += bytes;
                self.tiles[index] = Slot::Kept(Box::new(walk), bytes);
            }
            unkept => self.tiles[index] = unkept,
        }
        let Slot::Kept(walk, bytes) = &mut self.tiles[index] else {
            return walk_again(header, tile, meter, poc_bitmap, reduction, layers, repeats);
        };
        // A walk started again reads from the start what an earlier walk
        // of the tile read, so its first walk is charged as a walk of a
        // tile not kept.
        if walk.restarted && past_repeats(meter, repeats) {
            return Err(repeats.refuse(meter));
        }
        #[cfg(test)]
        let visits = if fresh { 0 } else { walk.visits() };
        let left = meter.left();
        let mut walked = walk.walk(header, tile, meter, poc_bitmap);
        if std::mem::take(&mut walk.restarted) {
            walked = charge_repeat(meter, left, repeats, walked);
        }
        #[cfg(test)]
        {
            self.visits = self.visits + walk.visits() - visits;
        }
        let now = walk.bytes();
        self.kept = self.kept - *bytes + now;
        *bytes = now;
        self.largest = self.largest.max(now);
        // A walk that has grown past the room is let go. Its decode has
        // what it read; the next walks of the tile go from the start.
        if !fits(self, 0) {
            self.let_go(index);
            self.tiles[index] = Slot::Unkept {
                bytes: now,
                volumes,
                packed,
            };
        }
        walked
    }

    /// Let go of the walk of tile `index`, if one is kept. Its visits stay
    /// counted.
    fn let_go(&mut self, index: usize) {
        if let Slot::Kept(_, bytes) = &self.tiles[index] {
            self.kept -= *bytes;
            self.tiles[index] = Slot::Empty;
        }
    }

    /// The most bytes one tile's walk has held under the current layer cap.
    #[cfg(test)]
    pub(crate) fn largest(&self) -> u64 {
        self.largest
    }

    /// How many tiles have a walk kept, and how many one too large to keep.
    #[cfg(any(test, feature = "__fuzz"))]
    pub(crate) fn kept(&self) -> (usize, usize) {
        let kept = self
            .tiles
            .iter()
            .filter(|s| matches!(s, Slot::Kept(..)))
            .count();
        let unkept = self
            .tiles
            .iter()
            .filter(|s| matches!(s, Slot::Unkept { .. }))
            .count();
        (kept, unkept)
    }

    /// The bytes the walks kept hold, as [`TileWalk::bytes`] counts them.
    #[cfg(test)]
    pub(crate) fn kept_bytes(&self) -> u64 {
        self.kept
    }

    /// Per tile, the code-blocks of the resolutions the reduction drops that
    /// its walk kept charges for each component, or `None` for a tile with
    /// no walk kept.
    #[cfg(test)]
    pub(crate) fn dropped(&self) -> Vec<Option<Vec<u64>>> {
        self.tiles
            .iter()
            .map(|slot| match slot {
                Slot::Kept(walk, _) => Some(walk.dropped.clone()),
                _ => None,
            })
            .collect()
    }

    /// The packet visits the walks have spent: the visits the decodes they
    /// stand for charge for the packets and progression points read or
    /// counted, each counted once however many decodes charged it. A walk
    /// let go, or started again, keeps its count; the walks of a tile not
    /// kept are not counted.
    #[cfg(test)]
    pub(crate) fn visits(&self) -> u64 {
        self.visits
    }
}

/// The walk of a tile not kept, from the start, as a one-shot decode walks
/// it, with its visits added to `repeats` (see [`Walks::walk`]). It is
/// refused for work when `repeats` is past [`REPEAT_BOUNDS`] packet bounds,
/// before the walk or after it. A walk that fails otherwise fails as the
/// one-shot walk does.
fn walk_again<'t>(
    header: &MainHeader,
    tile: &'t Tile<'t>,
    meter: &Meter,
    poc_bitmap: u64,
    reduction: u8,
    layers: Option<u16>,
    repeats: &Repeats,
) -> std::result::Result<(CodedData<'t>, Reach), Refused> {
    if past_repeats(meter, repeats) {
        return Err(repeats.refuse(meter));
    }
    let left = meter.left();
    let walked = super::decode_packets_partial(header, tile, meter, poc_bitmap, reduction, layers);
    charge_repeat(meter, left, repeats, walked)
}

/// Whether the walks charged to `repeats` are past [`REPEAT_BOUNDS`]
/// packet bounds of the decode `meter` meters.
fn past_repeats(meter: &Meter, repeats: &Repeats) -> bool {
    repeats.visits.get() > meter.first().saturating_mul(REPEAT_BOUNDS)
}

/// Add to `repeats` the visits `meter` has spent since it had `left`, and
/// refuse for work a walk that took `repeats` past its bound. A walk that
/// failed stays as it failed.
fn charge_repeat<T>(
    meter: &Meter,
    left: u64,
    repeats: &Repeats,
    walked: std::result::Result<T, Refused>,
) -> std::result::Result<T, Refused> {
    let spent = left.saturating_sub(meter.left());
    repeats
        .visits
        .set(repeats.visits.get().saturating_add(spent));
    match walked {
        Ok(_) if past_repeats(meter, repeats) => Err(repeats.refuse(meter)),
        walked => walked,
    }
}

/// The walks charged as walks of tiles not kept, over all the decodes of
/// an incremental decoder (see [`Walks::walk`]).
#[derive(Debug, Default)]
pub(crate) struct Repeats {
    /// The packet visits they have spent.
    pub(crate) visits: Cell<u64>,
    /// Whether their bound refused a walk of this decode.
    pub(crate) refused: Cell<bool>,
    /// The packet bound of the decode whose walk their bound refused, or 0.
    pub(crate) bound: Cell<u64>,
}

impl Repeats {
    /// Walks that have spent `visits`, none refused yet.
    pub(crate) fn new(visits: u64) -> Self {
        Repeats {
            visits: Cell::new(visits),
            refused: Cell::new(false),
            bound: Cell::new(0),
        }
    }

    /// Refuse a walk for work in the decode `meter` meters, and note that
    /// their bound refused it, and that decode's packet bound.
    fn refuse(&self, meter: &Meter) -> Refused {
        self.refused.set(true);
        self.bound.set(meter.first());
        Refused::Work(WorkRefusal::PacketWalk)
    }
}

/// How a step of the walk over new data ended.
enum Advanced {
    /// At the end of the data, or of the order.
    Stopped,
    /// Short of visits: the decode of this prefix is refused too.
    Refused,
    /// At a fault in the packet the walk read: the decode of this prefix
    /// fails there too, unless the visits it has spent by then, given, are
    /// more than it has.
    Failed(Error, u64),
}

/// What [`TileWalk::bytes`] counts for the walk's own struct. It is a
/// constant, not `size_of::<TileWalk>()`, so that where an
/// `IncrementalDecoder` keeps or lets go of a walk, and so what a refine
/// spends, does not move with the compiler's choice of layout. It is a
/// fixed count no smaller than the struct on 64-bit targets; a unit test
/// holds that. If the struct grows past it, raise the count and measure the
/// spend again against main's: a change to this number is a change to that
/// behavior.
const WALK_SELF_BYTES: usize = 1200;

/// One tile's walk.
struct TileWalk {
    /// How many `POC` volumes the tile's header had when the walk started.
    volumes: usize,
    /// Whether the tile's packet headers were packed (`PPT` or `PPM`).
    packed: bool,
    /// The code-blocks each component's geometry holds in the resolutions
    /// the reduction drops, which a decode charges as it builds them.
    dropped: Vec<u64>,
    /// The fault that ended the geometry, after the components `dropped`
    /// holds.
    failure: Option<Error>,
    geoms: Vec<Vec<ResolutionGeom>>,
    /// The tile's rectangle on the reference grid.
    rect: Rect,
    states: Vec<Vec<Vec<BandState>>>,
    delimiters: Delimiters,
    /// Each component's code-block style.
    styles: Vec<u8>,
    /// The decode's layer cap.
    layers: Option<u16>,
    order: Order,
    /// The reading walk.
    cursor: Cursor,
    /// Whether the order is a `POC` sequence, whose volumes may emit a
    /// packet again: then a packet of a layer below the precinct's packets
    /// read is one already emitted (see [`TileWalk::advance`]).
    dedup: bool,
    /// The packets the `POC` volumes reach, and whether all of them are
    /// layer orders (see `progression::Emitted`).
    reached: u64,
    layer_orders: bool,
    /// Per component and resolution: the first layer the `POC` volumes
    /// leave out.
    gaps: Vec<Vec<Option<u32>>>,
    /// Per component, resolution and precinct: the packets read whole,
    /// which are its first layers.
    read: Vec<Vec<Vec<u32>>>,
    header_pos: usize,
    body_pos: usize,
    packet_index: u32,
    /// The packet the reading walk is at, not yet read whole.
    pending: Option<PacketId>,
    /// Whether `pending` is read in part into the state, for an image.
    cut: bool,
    /// The visits reading `pending` in part spent.
    pending_visits: u64,
    /// Whether every packet of the order has been read.
    done: bool,
    /// Whether the walk, at its end, has let go of what reads packets.
    shrunk: bool,
    /// Where the walk was before the packets it read ahead, if it did:
    /// packets read whole that more data may yet read otherwise, from the
    /// first that starts, with `SOP` markers, with fewer than two bytes of
    /// the data left.
    tentative: Option<Mark>,
    read_ahead: Vec<Ahead>,
    /// What the reading walk has spent: on the points of the order, on the
    /// packets read whole, and how many packets it found already emitted.
    loops: u64,
    packets: u64,
    parsed: u64,
    skips: u64,
    /// The walk that counts the packets of the order.
    census: Census,
    scratch: Scratch,
    log: Vec<Undo>,
    journal: Journal,
    /// What the end of the data costs (see [`END_VISITS`]).
    end_visits: u64,
    /// Whether the walk was built for a tile an earlier walk had read, and
    /// has not walked yet: its first walk reads those packets again, and
    /// is charged as a walk of a tile not kept (see [`Walks::walk`]).
    restarted: bool,
}

/// Where a walk was, to go back to (see [`TileWalk::tentative`]).
struct Mark {
    cursor: Cursor,
    pending: Option<PacketId>,
    header_pos: usize,
    body_pos: usize,
    packet_index: u32,
    loops: u64,
    packets: u64,
    parsed: u64,
    skips: u64,
}

/// A packet read ahead, with what puts it back.
struct Ahead {
    id: PacketId,
    log: Vec<Undo>,
    journal: Journal,
}

/// A walk of the order that reads no data: how many packets it emits for
/// each precinct, and what a walk of all of it is charged.
struct Census {
    cursor: Cursor,
    counts: Vec<Vec<Vec<u32>>>,
    loops: u64,
    packets: u64,
    skips: u64,
    done: bool,
}

impl TileWalk {
    fn new(
        header: &MainHeader,
        tile: &Tile<'_>,
        reduction: u8,
        layers: Option<u16>,
        cap: crate::samples::Cap,
    ) -> Self {
        let mut budget = Budget {
            reduction,
            layers,
            ..Budget::decode(0, 0).within(cap)
        };
        let mut dropped = Vec::new();
        let built = tile_geometry(header, tile.index, &mut budget, |blocks| {
            dropped.push(blocks as u64);
            Ok(())
        })
        .and_then(|geoms| {
            let rect = header
                .siz
                .tile_rect(tile.index)
                .ok_or_else(|| Error::inconsistent(format!("no tile {} in `SIZ`", tile.index)))?;
            Ok((geoms, rect))
        });
        let (geoms, rect, failure) = match built {
            Ok((geoms, rect)) => (geoms, rect, None),
            Err(error) => (
                Vec::new(),
                Rect {
                    x0: 0,
                    y0: 0,
                    x1: 0,
                    y1: 0,
                },
                Some(error),
            ),
        };
        let mut states = band_states(header, &geoms);
        let walk = PacketWalk {
            siz: &header.siz,
            tile: rect,
            geoms: &geoms,
        };
        let order = Order::new(header, &walk);
        let mut gaps: Vec<Vec<Option<u32>>> = geoms.iter().map(|c| vec![None; c.len()]).collect();
        if failure.is_none() {
            // Not strict, a gap is noted, never refused.
            let _ = poc_gaps(header, &walk, |comp, res, layer| {
                let layer = u32::from(layer);
                gaps[comp][res] = Some(gaps[comp][res].map_or(layer, |l| l.min(layer)));
                mark_gap(&mut states[comp][res], layer);
                Ok(())
            });
        }
        let dedup = order.volumes.len() > 1;
        let reached = if dedup && failure.is_none() {
            reached(&order.volumes, &walk, order.max_res)
        } else {
            0
        };
        let counts: Vec<Vec<Vec<u32>>> = geoms
            .iter()
            .map(|c| c.iter().map(|r| vec![0; r.precinct_count()]).collect())
            .collect();
        let end_visits = geoms.iter().map(Vec::len).sum::<usize>() as u64 * END_VISITS as u64;
        TileWalk {
            volumes: header.poc.len(),
            packed: tile.packed,
            dropped,
            failure,
            states,
            delimiters: Delimiters {
                sop: header.cod.use_sop,
                eph: header.cod.use_eph,
            },
            styles: header
                .components
                .iter()
                .map(|c| c.coding.code_block_style)
                .collect(),
            layers,
            layer_orders: layer_orders(&order.volumes),
            order,
            cursor: Cursor::default(),
            dedup,
            reached,
            gaps,
            read: counts.clone(),
            header_pos: 0,
            body_pos: 0,
            packet_index: 0,
            pending: None,
            cut: false,
            pending_visits: 0,
            done: false,
            shrunk: false,
            tentative: None,
            read_ahead: Vec::new(),
            loops: 0,
            packets: 0,
            parsed: 0,
            skips: 0,
            census: Census {
                cursor: Cursor::default(),
                counts,
                loops: 0,
                packets: 0,
                skips: 0,
                done: false,
            },
            scratch: Scratch::default(),
            log: Vec::new(),
            journal: Journal::default(),
            end_visits,
            restarted: false,
            geoms,
            rect,
        }
    }

    /// Charge the code-blocks `reduction` drops from now on, counted from
    /// the walk's geometry as [`tile_geometry`] counts them for a decode at
    /// that reduction. Nothing else the walk holds depends on the
    /// reduction: the geometry is built at full resolution whatever it is,
    /// and the order, the states and the coded data are the same. `false`
    /// for a walk whose geometry failed, which holds no geometry to count
    /// from; the count it holds is then left as it was.
    fn reduce(&mut self, reduction: u8) -> bool {
        // Defensive: a decoder only keeps a walk once `check_tile` has
        // built the same frame and passed the same caps, so a kept walk
        // whose geometry failed is not reachable through the public API.
        if self.failure.is_some() {
            return false;
        }
        debug_assert_eq!(self.dropped.len(), self.geoms.len());
        for (dropped, resolutions) in self.dropped.iter_mut().zip(&self.geoms) {
            *dropped = dropped_blocks(resolutions, reduction) as u64;
        }
        true
    }

    /// The bytes the walk holds on the heap, capacity included: its
    /// geometry, its packet state, its order, its counts and its buffers.
    /// Only the message of a fault in the geometry is left out; a walk
    /// with one holds no geometry.
    fn bytes(&self) -> u64 {
        use std::mem::size_of;
        /// The bytes of a list of lists: the outer list's, and the inner
        /// lists' items.
        fn nested<T>(lists: &Vec<Vec<T>>) -> usize {
            lists.capacity() * size_of::<Vec<T>>()
                + lists.iter().map(Vec::capacity).sum::<usize>() * size_of::<T>()
        }
        let mut bytes = WALK_SELF_BYTES
            + self.dropped.capacity() * size_of::<u64>()
            + self.styles.capacity()
            + nested(&self.geoms)
            + self.states.capacity() * size_of::<Vec<Vec<BandState>>>()
            + nested(&self.gaps)
            + self.order.heap_bytes()
            + self.scratch.heap_bytes()
            + self.log.capacity() * size_of::<Undo>()
            + self.journal.heap_bytes()
            + self.read_ahead.capacity() * size_of::<Ahead>();
        for resolution in self.geoms.iter().flatten() {
            bytes += resolution.heap_bytes();
        }
        for component in &self.states {
            bytes += nested(component);
            for bands in component {
                bytes += bands.iter().map(BandState::heap_bytes).sum::<usize>();
            }
        }
        for counts in [&self.read, &self.census.counts] {
            bytes += counts.capacity() * size_of::<Vec<Vec<u32>>>();
            bytes += counts.iter().map(nested).sum::<usize>();
        }
        for ahead in &self.read_ahead {
            bytes += ahead.log.capacity() * size_of::<Undo>() + ahead.journal.heap_bytes();
        }
        bytes as u64
    }

    /// Let go of what reads packets, once the walk has read them all: the
    /// order, the tag trees, the counts of packets read, and the buffers.
    /// What builds the image, and the `POC` gaps its report notes, stay.
    fn shrink(&mut self) {
        for component in &mut self.states {
            for bands in component {
                for band in bands {
                    band.drop_trees();
                }
            }
        }
        self.order = Order::default();
        self.styles = Vec::new();
        self.read = Vec::new();
        self.census.counts = Vec::new();
        self.scratch = Scratch::default();
        self.log = Vec::new();
        self.journal = Journal::default();
        self.read_ahead = Vec::new();
        self.shrunk = true;
    }

    /// The visits this walk has spent, each point and packet of the order
    /// counted once (see [`Walks::visits`]).
    #[cfg(test)]
    fn visits(&self) -> u64 {
        let order = (self.loops + self.packets).max(self.census.loops + self.census.packets);
        let spans = self.parsed - self.packets;
        self.dropped
            .iter()
            .fold(order + spans, |a, &b| a.saturating_add(b))
    }

    /// See [`Walks::walk`].
    fn walk<'t>(
        &mut self,
        header: &MainHeader,
        tile: &'t Tile<'t>,
        meter: &Meter,
        poc_bitmap: u64,
    ) -> std::result::Result<(CodedData<'t>, Reach), Refused> {
        let walked = self.charge(header, tile, meter, poc_bitmap).and_then(|()| {
            self.view(header, tile)
                .map_err(|error| refused(meter, error))
        });
        // A packet read in part is put back, to be read again once more
        // data comes.
        self.unwind();
        // A walk at its end reads no more packets: only what builds the
        // image is kept.
        if self.done && !self.shrunk {
            self.shrink();
        }
        walked
    }

    /// Bring the walk up to the data `tile` holds, and charge `meter` as a
    /// decode of that data charges it, in the same order: the geometry,
    /// the `POC` packets, and then the walk.
    fn charge(
        &mut self,
        header: &MainHeader,
        tile: &Tile<'_>,
        meter: &Meter,
        poc_bitmap: u64,
    ) -> std::result::Result<(), Refused> {
        meter.clear_refusal();
        // The decode's bitmap holds at most the visits left as its walk
        // starts (see `Budget::decode`).
        let dense_bits = meter.left().min(poc_bitmap);
        for &blocks in &self.dropped {
            if !meter.try_spend(blocks) {
                return Err(Refused::Work(WorkRefusal::DroppedBlocks { blocks }));
            }
        }
        if let Some(error) = &self.failure {
            return Err(Refused::Other(error.clone()));
        }
        // The components the tile holds no sample of, charged once the
        // geometry is built, as a decode charges them.
        if let Some(why @ WorkRefusal::EmptyComponents { steps, .. }) =
            empty_components(header, tile.index)
        {
            if !meter.try_spend(steps) {
                return Err(Refused::Work(why));
            }
        }
        // `dedup` is whether the order has more than one volume; the order
        // itself is let go once the walk is done.
        let sparse = if self.dedup {
            if self.layer_orders && self.reached > meter.left() {
                return Err(Refused::Work(WorkRefusal::PocReach {
                    reached: self.reached,
                }));
            }
            self.reached > dense_bits
        } else {
            false
        };
        let skip = if sparse { COUNTS_SKIP_VISITS as u64 } else { 0 };
        // Each walk here spends no more than the decode's own: each step
        // it takes is one the decode takes. So a walk short of what the
        // decode has left is refused with it.
        let left = meter.left();
        let charge = match self.advance(header, tile, left) {
            Advanced::Refused => return Err(Refused::Work(WorkRefusal::PacketWalk)),
            Advanced::Failed(error, spent) => {
                let spent = spent.saturating_add(self.skips.saturating_mul(skip));
                if !meter.try_spend(spent) {
                    return Err(Refused::Work(WorkRefusal::PacketWalk));
                }
                return Err(Refused::Other(error));
            }
            Advanced::Stopped if self.done => self.loops + self.parsed + self.skips * skip,
            Advanced::Stopped => {
                if !self.count(header, left) {
                    return Err(Refused::Work(WorkRefusal::PacketWalk));
                }
                // The decode reads the packets read whole and the one the
                // data cuts, is charged for the end of the data, and then
                // counts every packet after it.
                let census = &self.census;
                let after = census.packets.saturating_sub(self.packets + 1);
                [
                    census.loops,
                    census.skips.saturating_mul(skip),
                    self.parsed,
                    self.pending_visits,
                    self.end_visits,
                    after,
                ]
                .into_iter()
                .fold(0, u64::saturating_add)
            }
        };
        if !meter.try_spend(charge) {
            return Err(Refused::Work(WorkRefusal::PacketWalk));
        }
        Ok(())
    }

    /// Read the packets the data now holds, spending no more than `left`
    /// visits on the walk in all.
    fn advance(&mut self, header: &MainHeader, tile: &Tile<'_>, left: u64) -> Advanced {
        let meter = Meter::new(left.saturating_sub(self.loops + self.parsed));
        loop {
            let id = match self.pending {
                Some(id) => id,
                None if self.done => return Advanced::Stopped,
                None => {
                    let walk = PacketWalk {
                        siz: &header.siz,
                        tile: self.rect,
                        geoms: &self.geoms,
                    };
                    let before = self.cursor;
                    match self.order.next(&walk, &mut self.cursor) {
                        None => {
                            self.done = true;
                            return Advanced::Stopped;
                        }
                        Some(Step::Point(visits)) => {
                            if !meter.try_spend(visits as u64) {
                                self.cursor = before;
                                return Advanced::Refused;
                            }
                            self.loops += visits as u64;
                            continue;
                        }
                        Some(Step::Packet(id)) => {
                            // Each volume emits a precinct's layers from 0
                            // up, so the layers a `POC` sequence has emitted
                            // of a precinct are always its first ones: those
                            // read whole, as no packet is pending here. A
                            // packet of a layer below them is one emitted
                            // before, which the decode's bitmap or counts
                            // find there.
                            if self.dedup {
                                let read = self.read[id.comp][id.res][id.precinct];
                                debug_assert!(id.layer <= read, "{id:?} after {read} layers");
                                if id.layer < read {
                                    self.skips += 1;
                                    continue;
                                }
                            }
                            self.pending = Some(id);
                            id
                        }
                    }
                }
            };
            // With `SOP` markers, a packet that starts with fewer than two
            // bytes of the data left may yet have an `SOP` before it, which
            // the decode cannot see until more data comes. Read whole, it
            // is read from what may be the marker's first byte, inline, or
            // with no body byte to look at, packed.
            let tentative = self.tentative.is_some()
                || (self.delimiters.sop && tile.data.len().saturating_sub(self.body_pos) < 2);
            let before = meter.spent();
            let (read, header_pos, body_pos) = self.parse(tile, id, &meter);
            let spent = meter.spent() - before;
            match read {
                Ok(Packet::Read) => {
                    if tentative {
                        // It is read for this image, then put back.
                        // The first is put back as the packet to read next;
                        // the rest are emitted again.
                        let first = self.tentative.is_none();
                        if first {
                            self.tentative = Some(Mark {
                                cursor: self.cursor,
                                pending: self.pending,
                                header_pos: self.header_pos,
                                body_pos: self.body_pos,
                                packet_index: self.packet_index,
                                loops: self.loops,
                                packets: self.packets,
                                parsed: self.parsed,
                                skips: self.skips,
                            });
                        }
                        self.read_ahead.push(Ahead {
                            id,
                            log: std::mem::take(&mut self.log),
                            journal: std::mem::take(&mut self.journal),
                        });
                    }
                    self.header_pos = header_pos;
                    self.body_pos = body_pos;
                    self.packet_index += 1;
                    self.read[id.comp][id.res][id.precinct] += 1;
                    self.packets += 1;
                    self.parsed += spent;
                    self.pending = None;
                }
                Ok(Packet::Ended) => {
                    // The decode keeps what the packet holds of whole
                    // code-blocks; it stays in the state for the image.
                    self.cut = true;
                    self.pending_visits = spent;
                    return Advanced::Stopped;
                }
                Err(error) => {
                    self.cut = true;
                    if meter.refusal().is_some() {
                        return Advanced::Refused;
                    }
                    return Advanced::Failed(error, self.loops + self.parsed + spent);
                }
            }
        }
    }

    /// Read packet `id` from where the walk is in `tile`'s data, spending
    /// `meter`; with the stream positions after it.
    fn parse(
        &mut self,
        tile: &Tile<'_>,
        id: PacketId,
        meter: &Meter,
    ) -> (Result<Packet>, usize, usize) {
        let data: &[u8] = &tile.data;
        let mut streams = PacketStreams {
            header: if self.packed {
                &tile.packed_headers
            } else {
                data
            },
            header_pos: self.header_pos,
            data,
            body_pos: self.body_pos,
            packed: self.packed,
            tile: tile.index,
        };
        // The logs hold this packet's changes alone, so that it can be put
        // back whole.
        self.log.clear();
        self.journal.clear();
        let read = parse_packet(
            &mut streams,
            meter,
            id.layer,
            self.packet_index,
            self.delimiters,
            self.styles[id.comp],
            &self.geoms[id.comp][id.res].bands,
            id.precinct,
            &mut self.states[id.comp][id.res],
            &mut self.scratch,
            self.layers.is_none_or(|cap| id.layer < u32::from(cap)),
            Some(&mut self.log),
            Some(&mut self.journal),
        );
        (read, streams.header_pos, streams.body_pos)
    }

    /// Put back the packet read in part, if one is, and then the packets
    /// read ahead, latest first.
    fn unwind(&mut self) {
        if std::mem::take(&mut self.cut) {
            if let Some(id) = self.pending {
                let states = &mut self.states[id.comp][id.res];
                undo(&self.log, states);
                self.journal.restore(states, id.precinct);
            }
            self.pending_visits = 0;
        }
        let Some(mark) = self.tentative.take() else {
            return;
        };
        for ahead in self.read_ahead.drain(..).rev() {
            let id = ahead.id;
            let states = &mut self.states[id.comp][id.res];
            undo(&ahead.log, states);
            ahead.journal.restore(states, id.precinct);
            self.read[id.comp][id.res][id.precinct] -= 1;
        }
        self.cursor = mark.cursor;
        self.pending = mark.pending;
        self.header_pos = mark.header_pos;
        self.body_pos = mark.body_pos;
        self.packet_index = mark.packet_index;
        self.loops = mark.loops;
        self.packets = mark.packets;
        self.parsed = mark.parsed;
        self.skips = mark.skips;
        self.done = false;
    }

    /// Count the packets of the order, spending no more than `left` visits
    /// on the count in all; whether it got to the end.
    fn count(&mut self, header: &MainHeader, left: u64) -> bool {
        let census = &mut self.census;
        if census.done {
            return true;
        }
        let meter = Meter::new(left.saturating_sub(census.loops + census.packets));
        let walk = PacketWalk {
            siz: &header.siz,
            tile: self.rect,
            geoms: &self.geoms,
        };
        loop {
            let before = census.cursor;
            match self.order.next(&walk, &mut census.cursor) {
                None => {
                    census.done = true;
                    return true;
                }
                Some(Step::Point(visits)) => {
                    if !meter.try_spend(visits as u64) {
                        census.cursor = before;
                        return false;
                    }
                    census.loops += visits as u64;
                }
                Some(Step::Packet(id)) => {
                    if meter.left() == 0 {
                        census.cursor = before;
                        return false;
                    }
                    // As in the reading walk, a packet of a layer below
                    // the precinct's count is one already emitted.
                    let count = &mut census.counts[id.comp][id.res][id.precinct];
                    if self.dedup && id.layer < *count {
                        census.skips += 1;
                        continue;
                    }
                    meter.try_spend(1);
                    *count += 1;
                    census.packets += 1;
                }
            }
        }
    }

    /// The coded data and the reach of the walk as it stands.
    fn view<'t>(
        &mut self,
        header: &MainHeader,
        tile: &'t Tile<'t>,
    ) -> Result<(CodedData<'t>, Reach)> {
        let data: &'t [u8] = &tile.data;
        let mut reach = Reach {
            missing_from: self.gaps.clone(),
            ..Reach::default()
        };
        let mut components = Vec::with_capacity(self.states.len());
        for (comp, component) in self.states.iter_mut().enumerate() {
            let rect = header
                .siz
                .tile_component_rect(tile.index, comp)
                .ok_or_else(|| Error::inconsistent(format!("no component {comp} in `SIZ`")))?;
            let mut resolutions = Vec::with_capacity(component.len());
            for (res, bands) in component.iter_mut().enumerate() {
                // A precinct whose packets the data has not all brought is
                // missing from its first packet not read.
                let (read, counts): (&[u32], &[u32]) = if self.shrunk {
                    (&[], &[])
                } else {
                    (&self.read[comp][res], &self.census.counts[comp][res])
                };
                for (precinct, &read) in read.iter().enumerate() {
                    let ended = (!self.done && read < counts[precinct]).then_some(read);
                    if let Some(layer) = ended {
                        let from = &mut reach.missing_from[comp][res];
                        *from = Some(from.map_or(layer, |f| f.min(layer)));
                    }
                    for band in bands.iter_mut() {
                        if let Some(state) = band.precincts.get_mut(precinct) {
                            state.ended = ended;
                        }
                    }
                }
                let level = &self.geoms[comp][res];
                let subbands = if level.bands.is_empty() {
                    Vec::new()
                } else {
                    build_subbands(&level.bands, bands, data)
                };
                resolutions.push(Resolution { subbands });
            }
            components.push(ComponentCoded {
                resolutions,
                rect: Some(rect),
            });
        }
        // Bytes left after the last packet are noted; data that ran out
        // leaves none.
        if self.done {
            if self.body_pos < data.len() {
                reach.leftover = Some((self.body_pos, data.len() - self.body_pos));
            }
            if self.packed && self.header_pos < tile.packed_headers.len() {
                reach.leftover_packed = Some(tile.packed_headers.len() - self.header_pos);
            }
        }
        Ok((CodedData { components }, reach))
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    /// The fixed count of a walk's own struct is no smaller than the
    /// struct. If the struct grows past it, raise the count and measure the
    /// spend again.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_walk_self_count_covers_the_struct() {
        assert!(std::mem::size_of::<TileWalk>() <= WALK_SELF_BYTES);
    }
}
