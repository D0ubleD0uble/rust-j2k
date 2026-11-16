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

use crate::codestream::{MainHeader, Tile};
use crate::tier2::Rect;
use crate::{Error, Result};

use super::progression::{
    Cursor, Emitted, Order, PacketId, SET_SKIP_VISITS, Step, layer_orders, poc_gaps,
};
use super::{
    BandState, Budget, CodedData, ComponentCoded, DECODE_VISITS_MIN, Delimiters, END_VISITS,
    Journal, Meter, Packet, PacketStreams, PacketWalk, Reach, Refused, Resolution, ResolutionGeom,
    Scratch, Undo, WorkRefusal, band_states, build_subbands, mark_gap, parse_packet, refused,
    tile_geometry, undo,
};

/// The packet walks of an incremental decode's tiles, by tile index.
#[derive(Default)]
pub(crate) struct Walks {
    tiles: Vec<Option<TileWalk>>,
    /// The visits of walks started again (see [`Walks::walk`]).
    retired: u64,
}

impl Walks {
    /// What a decode with `strict` off of the bytes `tile` holds gets from
    /// its packets, under `header`, its resolved header: the coded data,
    /// and what was not read. `meter` is charged what that decode's walk
    /// would charge it, and the work bound refuses this walk where it
    /// refuses that one. `poc_bitmap`, `reduction` and `layers` are as
    /// [`super::decode_packets_partial`] takes them, and must be the same
    /// on every call for a tile.
    ///
    /// A tile whose later tile-part adds `POC` volumes, or packed packet
    /// headers where it had none, has another order of packets, or reads
    /// them from elsewhere: its walk is started again.
    pub(crate) fn walk<'t>(
        &mut self,
        header: &MainHeader,
        tile: &'t Tile<'t>,
        meter: &Meter,
        poc_bitmap: u64,
        reduction: u8,
        layers: Option<u16>,
    ) -> std::result::Result<(CodedData<'t>, Reach), Refused> {
        let index = tile.index as usize;
        if self.tiles.len() <= index {
            self.tiles.resize_with(index + 1, || None);
        }
        let packed = !tile.packed_headers.is_empty();
        let slot = &mut self.tiles[index];
        if slot
            .as_ref()
            .is_some_and(|walk| walk.volumes != header.poc.len() || walk.packed != packed)
        {
            self.retired = self
                .retired
                .saturating_add(slot.take().map_or(0, |walk| walk.visits()));
        }
        let walk = slot.get_or_insert_with(|| TileWalk::new(header, tile, reduction, layers));
        walk.walk(header, tile, meter, poc_bitmap)
    }

    /// The packet visits the walks have spent: the visits the decodes they
    /// stand for charge for the packets and progression points read or
    /// counted, each counted once however many decodes charged it.
    pub(crate) fn visits(&self) -> u64 {
        self.tiles
            .iter()
            .flatten()
            .map(TileWalk::visits)
            .fold(self.retired, u64::saturating_add)
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
    emitted: Option<Emitted>,
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
    /// Where the walk was before the packets it read ahead, if it did:
    /// packets read whole that more data may yet read otherwise.
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
    /// Whether the walk emitted it while reading ahead.
    emitted: bool,
    log: Vec<Undo>,
    journal: Journal,
}

/// A walk of the order that reads no data: how many packets it emits for
/// each precinct, and what a walk of all of it is charged.
struct Census {
    cursor: Cursor,
    emitted: Option<Emitted>,
    counts: Vec<Vec<Vec<u32>>>,
    loops: u64,
    packets: u64,
    skips: u64,
    done: bool,
}

impl TileWalk {
    fn new(header: &MainHeader, tile: &Tile<'_>, reduction: u8, layers: Option<u16>) -> Self {
        let mut budget = Budget {
            reduction,
            layers,
            ..Budget::decode(0, 0)
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
                .ok_or_else(|| Error::Inconsistent(format!("no tile {} in `SIZ`", tile.index)))?;
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
        let (emitted, census_emitted, reached) = if order.volumes.len() > 1 && failure.is_none() {
            let (emitted, reached) =
                Emitted::sized(&order.volumes, &walk, order.max_res, DECODE_VISITS_MIN);
            let (census, _) =
                Emitted::sized(&order.volumes, &walk, order.max_res, DECODE_VISITS_MIN);
            (Some(emitted), Some(census), reached)
        } else {
            (None, None, 0)
        };
        let counts: Vec<Vec<Vec<u32>>> = geoms
            .iter()
            .map(|c| c.iter().map(|r| vec![0; r.precinct_count()]).collect())
            .collect();
        let end_visits = geoms.iter().map(Vec::len).sum::<usize>() as u64 * END_VISITS as u64;
        TileWalk {
            volumes: header.poc.len(),
            packed: !tile.packed_headers.is_empty(),
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
            emitted,
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
            tentative: None,
            read_ahead: Vec::new(),
            loops: 0,
            packets: 0,
            parsed: 0,
            skips: 0,
            census: Census {
                cursor: Cursor::default(),
                emitted: census_emitted,
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
            geoms,
            rect,
        }
    }

    /// The visits this walk has spent, each point and packet of the order
    /// counted once (see [`Walks::visits`]).
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
        // The packets a walk at its end has emitted are not asked for again.
        if self.done {
            self.emitted = None;
            self.census.emitted = None;
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
        let sparse = if self.order.volumes.len() > 1 {
            if self.layer_orders && self.reached > meter.left() {
                return Err(Refused::Work(WorkRefusal::PocReach {
                    reached: self.reached,
                }));
            }
            self.reached > dense_bits
        } else {
            false
        };
        let skip = if sparse { SET_SKIP_VISITS as u64 } else { 0 };
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
                Some(id) => (id, false),
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
                            if let Some(emitted) = &mut self.emitted {
                                match emitted.insert(&walk, id.layer, id.res, id.comp, id.precinct)
                                {
                                    Ok(true) => {}
                                    Ok(false) => {
                                        self.skips += 1;
                                        continue;
                                    }
                                    Err(error) => {
                                        return Advanced::Failed(error, self.loops + self.parsed);
                                    }
                                }
                            }
                            self.pending = Some(id);
                            (id, self.emitted.is_some())
                        }
                    }
                }
            };
            let (id, fresh) = id;
            let tentative = self.tentative.is_some()
                || (self.packed && self.delimiters.sop && tile.data.len() - self.body_pos < 2);
            let before = meter.spent();
            let (read, header_pos, body_pos) = self.parse(tile, id, &meter);
            let spent = meter.spent() - before;
            match read {
                Ok(Packet::Read) => {
                    if tentative {
                        // Packed headers and `SOP` markers: a packet read
                        // with no byte of the data after it may yet have an
                        // `SOP` before it, which more data brings. It is
                        // read for this image, then put back.
                        // The first is put back as the packet to read next,
                        // so it stays emitted; the rest are emitted again.
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
                            emitted: fresh && !first,
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
            if ahead.emitted {
                if let Some(emitted) = &mut self.emitted {
                    emitted.remove(&self.geoms, id.layer, id.res, id.comp, id.precinct);
                }
            }
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
                    census.emitted = None;
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
                    if let Some(emitted) = &mut census.emitted {
                        if !emitted
                            .insert(&walk, id.layer, id.res, id.comp, id.precinct)
                            .unwrap_or(false)
                        {
                            census.skips += 1;
                            continue;
                        }
                    }
                    meter.try_spend(1);
                    census.counts[id.comp][id.res][id.precinct] += 1;
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
                .ok_or_else(|| Error::Inconsistent(format!("no component {comp} in `SIZ`")))?;
            let mut resolutions = Vec::with_capacity(component.len());
            for (res, bands) in component.iter_mut().enumerate() {
                // A precinct whose packets the data has not all brought is
                // missing from its first packet not read.
                let read = &self.read[comp][res];
                let counts = &self.census.counts[comp][res];
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
