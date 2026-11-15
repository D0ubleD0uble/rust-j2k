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
//! precinct) — in the order that tile's `COD` names, or, when a `POC` marker is
//! present, through the sequence of progression volumes it prescribes. The
//! tile's data is then `header₀ body₀ header₁ body₁ …` with no padding, and the
//! packets must tile it exactly; a leftover byte means a misread field.
//!
//! A code-block's contributions accumulate across the layers that include it,
//! so a precinct's tag trees and per-block state outlive any one packet. See
//! [`for_each_packet`] for the orders and [`BlockState`] for what persists.
//!
//! `decode_packets_metered` also computes the resolution / subband /
//! code-block geometry from the [`MainHeader`], so it is the single source of
//! truth the assembly stage reuses.

pub mod bio;
mod geometry;
mod progression;
pub mod spans;
pub mod tagtree;

use crate::codestream::markers::Rect;
use crate::codestream::markers::marker;
use crate::codestream::{MainHeader, Tile};
use crate::{Error, Result};
use bio::BitReader;
use spans::UnreadSpans;
use std::cell::Cell;
use tagtree::TagTree;

pub(crate) use geometry::{
    BandGeom, MAX_CODE_BLOCKS, MAX_PRECINCTS, PrecinctGeom, resolution_geoms,
};
pub(crate) use progression::{PacketWalk, for_each_packet};

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

/// Which block coder a code-block was coded with.
///
/// A tile-component's code-block style fixes it for every block, except
/// under MIXED (style bits `0x40` and `0x80`, ISO/IEC 15444-15 A.4), where
/// each block is an HT block or a Part 1 one. Tier-2 tells them apart by the
/// block's first nonzero length field (see [`mixed_first_length`]), since the
/// two read their later length fields differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coder {
    /// The Part 1 block coder (ISO/IEC 15444-1 Annex D).
    Part1,
    /// The HTJ2K block coder (ISO/IEC 15444-15).
    Ht,
}

impl Coder {
    /// The coder of every code-block of a tile-component with code-block
    /// style `style`, or `None` under MIXED, where each block has its own.
    /// `ccap15` is the codestream's `Ccap15` word: when it says every
    /// code-block is an HT block (bits 15-14 are `00`, A.3.2), a MIXED style
    /// holds only HT blocks, as A.3.2's note allows.
    pub(crate) fn of_style(
        style: u8,
        ccap15: Option<crate::codestream::markers::Ccap15>,
    ) -> Option<Coder> {
        use crate::codestream::markers::code_block_style::{HT, HT_MIXED};
        if style & HT == 0 {
            Some(Coder::Part1)
        } else if style & HT_MIXED == 0 || ccap15.is_some_and(|c| c.ht_only()) {
            Some(Coder::Ht)
        } else {
            None
        }
    }
}

/// One code-block's coded contribution within its subband. `num_passes` is the
/// coding-pass count from the packet headers (0 if no layer the decode keeps
/// includes the block) and `zero_bit_planes` the all-zero most-significant
/// bit-plane count from the zero-bitplane tag tree. `segments` are the raw
/// coded byte slices Tier-1 decodes (none when the block is not included).
///
/// An HT block's placeholder passes are in none of these: they code no bytes,
/// and each three of them add one to `zero_bit_planes` instead (ISO/IEC
/// 15444-15 B.3). Its segments are a cleanup segment, then, if it has
/// refinement passes, a refinement segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBlock<'a> {
    /// Top-left of the block within its subband, in band-relative samples.
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    /// Coding passes accumulated over every layer the decode keeps that
    /// contributed.
    pub num_passes: u32,
    pub zero_bit_planes: u32,
    /// The block's codeword segments, in order. Tier-1 restarts the MQ decoder
    /// at each one and carries its bit-plane state across them.
    pub segments: Vec<CodedSegment<'a>>,
    /// Whether the last segment may be cut short: a layer the decode drops
    /// adds coding passes to it, or, with `strict` off, a later packet of the
    /// block's precinct, which may add some, is missing: the data ends
    /// before it or the `POC` volumes leave it out. Either
    /// way the encoder may have terminated it past the passes and bytes kept
    /// here, so Tier-1 does not hold it to `predictable termination`. Only
    /// the last segment can be cut, since a layer adds passes and bytes to
    /// the last segment or to new ones.
    pub cut: bool,
    /// The placeholder passes of an HT block in the layers the decode keeps,
    /// which `num_passes` leaves out. Always 0 for a Part 1 block.
    pub placeholders: u32,
    /// The block's coder. A block of a MIXED tile-component whose length
    /// fields are all zero is an HT block of placeholder passes only, which
    /// codes nothing (see [`mixed_first_length`]).
    pub coder: Coder,
}

/// One codeword segment: the coding passes it carries and the bytes coding
/// them, which a multi-layer codestream may deliver in several chunks.
///
/// Without per-pass termination a block has exactly one segment, and the MQ
/// codeword runs continuously across every layer that contributed to it. Under
/// `restart` (`termall`) each coding pass is terminated separately, so each is
/// its own segment and Tier-1 must re-initialize the MQ decoder for it.
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
///
/// The walk spends `meter`, what is left of the decode's work bound (see
/// [`decode_visits`]), and fails with [`Error::Limit`] once it is gone. A
/// POC sequence's packets are held in a bitmap of at most `poc_bitmap`
/// bits, and no more than the visits left, and past it in a set (see
/// [`for_each_packet`]). The code-blocks of the resolutions that
/// `reduction` drops are charged to `meter` too (see
/// [`Budget::reduction`]). The packets of the layers past `layers` are read
/// but add nothing to their code-blocks (see [`Budget::layers`]).
pub(crate) fn decode_packets_metered<'a>(
    header: &MainHeader,
    tile: &'a Tile<'a>,
    meter: &Meter,
    poc_bitmap: u64,
    reduction: u8,
    layers: Option<u16>,
) -> std::result::Result<CodedData<'a>, Refused> {
    meter.clear_refusal();
    let mut budget = Budget {
        reduction,
        layers,
        ..Budget::decode(meter.left(), poc_bitmap)
    };
    decode_packets_within(header, tile, &mut budget, meter).map_err(|error| refused(meter, error))
}

/// [`decode_packets_metered`] on a meter of `visits` that is read back into
/// it, for tests that walk a tile on its own.
#[cfg(test)]
pub fn decode_packets<'a>(
    header: &MainHeader,
    tile: &'a Tile<'a>,
    visits: &mut u64,
    poc_bitmap: u64,
    reduction: u8,
) -> std::result::Result<CodedData<'a>, Refused> {
    let meter = Meter::new(*visits);
    let coded = decode_packets_metered(header, tile, &meter, poc_bitmap, reduction, None);
    *visits = meter.left();
    coded
}

/// How a walk that failed with `error` was refused: for work when `meter`
/// recorded why, otherwise as the error it is.
fn refused(meter: &Meter, error: Error) -> Refused {
    match (meter.refusal(), error) {
        (Some(work), Error::Limit(_)) => Refused::Work(work),
        (_, error) => Refused::Other(error),
    }
}

/// The fewest visits (see [`Meter`]) a decode may spend walking its
/// tiles' packets. A visit costs about 2 to 15 ns, so this is under a
/// second. A packet a POC marker's volumes reach again is skipped from a
/// bitmap without a visit, in about 4 ns; skipped from the set a sequence
/// past the bitmap keeps, it is charged 12 visits (see
/// `progression::for_each_packet`).
pub(crate) const DECODE_VISITS_MIN: u64 = 1 << 24;

/// The visits a decode may spend per byte of its tiles' data, when that comes
/// to more than [`DECODE_VISITS_MIN`]. At 2 to 15 ns a visit, that is up to
/// 0.5 µs a byte, about what decoding a byte of real data costs: the
/// conformance codestreams past 100 KB decode at 0.19 to 0.73 µs a byte (the
/// best of five release-build decodes of each). So a file past the floor is
/// refused in no more than about the time its decode would take. The
/// cheapest visits, the layer orders' steps over resolutions with no
/// precinct, refuse a file at about 0.02 to 0.06 µs a byte.
///
/// Setting up a tile's geometry is counted only for the code-blocks of the
/// resolutions a reduction drops (see [`Budget::reduction`]). A resolution with no precinct
/// gets no band or code-block geometry, so its setup is a few small
/// allocations. The costliest shape measured, 64 tiles of 16384 components
/// with one byte of packet data each, decodes at about 10 µs a byte, about
/// twice what OpenJPEG takes.
pub(crate) const DECODE_VISITS_PER_BYTE: u64 = 32;

/// The visits a decode may spend walking the packets of tiles that hold
/// `bytes` bytes of data in all, packed packet headers included.
///
/// A code-block entry that reads a bit of its packet header is paid for by
/// the header's bytes and is not counted. What is counted is the work that
/// reads none: each packet, each step over code-blocks whose inclusion is
/// already known to lie past the layer (see [`parse_band_header`]), and each
/// point of the progression order's loops. Encoder output spends 0.04 to 88
/// visits per byte, the high end only in files of a few hundred bytes,
/// under the floor; the largest share of the bound seen is 12.8%, from
/// `opj_compress` on a sparse 2048^2 image in 4x4 code-blocks and 64
/// layers. A packet header of one byte
/// can still make the walk step over each row of code-blocks of its
/// precinct, and a positional sweep can check millions of positions that
/// hold no packet, so without a bound a small file could make the walk run
/// for minutes. The one bound covers every tile of the decode, so many small
/// tiles cannot each spend the floor.
pub(crate) fn decode_visits(bytes: u64) -> u64 {
    DECODE_VISITS_MIN.max(bytes.saturating_mul(DECODE_VISITS_PER_BYTE))
}

/// What one walk of a tile's packets may hold: the geometry guards, the
/// POC packet storage and the reduction. The visits it spends are the
/// [`Meter`]'s. Running out of a guard fails the walk with [`Error::Limit`]
/// before the work or the allocation is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    /// Precincts across the tile's components (see [`resolution_geoms`]).
    pub(crate) precincts: usize,
    /// Code-blocks across the tile's components (see [`resolution_geoms`]).
    pub(crate) blocks: usize,
    /// The most bits a POC sequence's packets are held in a bitmap of (see
    /// [`for_each_packet`]).
    pub(crate) poc_bitmap: u64,
    /// Whether a POC sequence whose packets are more than
    /// [`poc_bitmap`](Self::poc_bitmap) holds them in a set; if not, its
    /// walk fails.
    pub(crate) poc_set: bool,
    /// Whether a POC sequence that leaves out packets of the tile is refused
    /// before the walk (see `progression::check_poc_coverage`). The decode
    /// sets it. `read_header`'s walk of a `Psot = 0` tile does not: it asks
    /// only where the packets the volumes reach end, and the decode refuses
    /// the tile anyway.
    pub(crate) poc_coverage: bool,
    /// The decode's resolution reduction. Each code-block of a resolution it
    /// drops costs a visit once the geometry is built, since every tile builds
    /// its geometry at full resolution whatever the reduction. Nothing else is
    /// charged for the geometry: each precinct is paid for by its packets'
    /// visits, and the code-blocks of the kept resolutions are no more than
    /// the reduced tile's samples, which the sample limit bounds, and the
    /// per-tile cap. Without
    /// the charge, many tiles of next to no data, each building close to the
    /// per-tile code-block cap, would make a small file take minutes to decode
    /// at a reduction. It is 0 for `read_header`'s walk.
    pub(crate) reduction: u8,
    /// The decode's layer cap (`DecodeOptions::max_quality_layers`): the
    /// packets of layer `layers` on are still read, since they frame the
    /// packets after them and their headers carry each code-block's state
    /// into the next layer, but their coding passes and bytes are not added
    /// to the code-blocks. Reading them costs the visits it does without a
    /// cap. `None` keeps every layer, as `read_header`'s walk does.
    pub(crate) layers: Option<u16>,
}

impl Budget {
    /// The decode's budget for one tile: the geometry guards, and a POC
    /// bitmap of no more bits than `visits`, the visits left on the decode's
    /// [`Meter`], or `poc_bitmap`, then a set. A walk that
    /// reads every packet takes a visit for each, so a bitmap of more bits
    /// than the visits left would be wasted. `poc_bitmap` is the decode's
    /// default bound, so a raised bound does not raise the bitmap's memory.
    pub(crate) const fn decode(visits: u64, poc_bitmap: u64) -> Budget {
        Budget {
            precincts: MAX_PRECINCTS,
            blocks: MAX_CODE_BLOCKS,
            poc_bitmap: if visits < poc_bitmap {
                visits
            } else {
                poc_bitmap
            },
            poc_set: true,
            poc_coverage: true,
            reduction: 0,
            layers: None,
        }
    }
}

/// What a walk with `strict` off found of a tile's packets: which were never
/// read, and what was left over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Reach {
    /// Per component, per resolution: the first layer with a packet that was
    /// not read, or `None` when every packet of it was read.
    pub(crate) missing_from: Vec<Vec<Option<u32>>>,
    /// Bytes of tile data not read: where they start and how many.
    pub(crate) leftover: Option<(usize, usize)>,
    /// Bytes of packed packet headers not read.
    pub(crate) leftover_packed: Option<usize>,
}

impl Reach {
    /// Note that packet (`layer`, `res`, `comp`) was not read.
    fn miss(&mut self, comp: usize, res: usize, layer: u32) {
        if let Some(from) = self.missing_from.get_mut(comp).and_then(|c| c.get_mut(res)) {
            *from = Some(from.map_or(layer, |f| f.min(layer)));
        }
    }
}

/// [`decode_packets_metered`] with `strict` off: the tile's data may end
/// anywhere, and what was not read comes back as a [`Reach`] beside the coded
/// data (see [`walk_packets`]).
pub fn decode_packets_partial<'a>(
    header: &MainHeader,
    tile: &'a Tile<'a>,
    meter: &Meter,
    poc_bitmap: u64,
    reduction: u8,
    layers: Option<u16>,
) -> std::result::Result<(CodedData<'a>, Reach), Refused> {
    meter.clear_refusal();
    let mut budget = Budget {
        reduction,
        layers,
        ..Budget::decode(meter.left(), poc_bitmap)
    };
    let mut reach = Reach::default();
    walk_packets(header, tile, &mut budget, meter, Some(&mut reach))
        .map(|coded| (coded, reach))
        .map_err(|error| refused(meter, error))
}

/// [`decode_packets_metered`] under `budget`, spending `meter`, which is left
/// holding what was not spent, also when the walk fails.
pub(crate) fn decode_packets_within<'a>(
    header: &MainHeader,
    tile: &'a Tile<'a>,
    budget: &mut Budget,
    meter: &Meter,
) -> Result<CodedData<'a>> {
    walk_packets(header, tile, budget, meter, None)
}

/// The packet walk behind [`decode_packets_within`] and
/// [`decode_packets_partial`]: strict when `reach` is `None`.
///
/// Not strict, the walk reads packets until the data ends (see
/// [`parse_packet`]), then goes on through the rest of the progression
/// without reading, to note each packet not read in `reach`. That costs a
/// visit a packet, from the same meter, and the end of the data costs
/// [`END_VISITS`] for each resolution of each component, for the setup no
/// byte paid for. Packets the `POC` volumes leave out are noted the same way
/// rather than refused, and bytes left after the last packet are noted
/// rather than refused. Every other fault, the work bound included, fails as
/// it does when strict.
fn walk_packets<'a>(
    header: &MainHeader,
    tile: &'a Tile<'a>,
    budget: &mut Budget,
    meter: &Meter,
    mut reach: Option<&mut Reach>,
) -> Result<CodedData<'a>> {
    let data: &'a [u8] = &tile.data;

    // Precincts are bounded *as the geometry is built*, not after: a precinct's
    // grid dimensions are cheap to compute but each precinct then costs a
    // `PrecinctGeom` per band and a pair of tag trees, each tree a heap
    // allocation, and the block count does not bound them — a band empty in
    // one axis has no blocks while its resolution still has a full column of
    // precincts. A 1 × 2^26 image under a 2^1 precinct is the shape that
    // exploits it: ~2^25 precincts, built before any after-the-fact check
    // could fire. So the cap is threaded through `resolution_geoms` and
    // enforced before the allocation, across every component of the tile
    // (`resolution_geoms` mutates the running total).
    //
    // The code-block count is bounded the same way and for the same reason:
    // the per-block `BlockState`s and tag-tree leaves below, and the
    // `CodeBlock`s after them, are sized by it, and legal 4×4 blocks push the
    // count toward samples/16, so the cap has to bite before they are
    // allocated rather than summing them afterwards. Both running totals are
    // threaded through `resolution_geoms`; the geometry itself holds nothing
    // per block.
    //
    // The code-blocks of the resolutions the reduction drops are charged to
    // `meter` as each component is built (see [`Budget::reduction`]), so a
    // tile can pass the bound by at most one component's geometry, which the
    // caps bound.
    let component_count = header.siz.components.len();
    let mut geoms = Vec::with_capacity(component_count);
    for c in 0..component_count {
        let resolutions = resolution_geoms(
            header,
            tile.index,
            c,
            &mut budget.precincts,
            &mut budget.blocks,
        )?;
        let kept = resolutions
            .len()
            .saturating_sub(usize::from(budget.reduction));
        let dropped: usize = resolutions[kept..]
            .iter()
            .flat_map(|resolution| &resolution.bands)
            .map(BandGeom::block_count)
            .sum();
        meter.spend(
            dropped,
            WorkRefusal::DroppedBlocks {
                blocks: dropped as u64,
            },
        )?;
        geoms.push(resolutions);
    }

    // One state per (component, resolution, band), carried across every layer.
    // The tag trees decode incrementally and there is a pair per *precinct*, so
    // they must outlive the packet that starts them.
    let mut states: Vec<Vec<Vec<BandState<'a>>>> = geoms
        .iter()
        .zip(&header.components)
        .map(|(component, params)| {
            let coder = Coder::of_style(params.coding.code_block_style, header.ccap15);
            component
                .iter()
                .map(|level| {
                    level
                        .bands
                        .iter()
                        .map(|band| BandState::new(band, coder))
                        .collect()
                })
                .collect()
        })
        .collect();

    let walk = PacketWalk {
        siz: &header.siz,
        tile: header
            .siz
            .tile_rect(tile.index)
            .ok_or_else(|| Error::Inconsistent(format!("no tile {} in `SIZ`", tile.index)))?,
        geoms: &geoms,
    };
    let delimiters = Delimiters {
        sop: header.cod.use_sop,
        eph: header.cod.use_eph,
    };
    // When the tile carries PPT packed headers, packet headers come from that
    // buffer and only the bodies stay inline; otherwise both are `data` and the
    // two cursors move together.
    let packed = !tile.packed_headers.is_empty();
    let mut streams = PacketStreams {
        header: if packed { &tile.packed_headers } else { data },
        header_pos: 0,
        data,
        body_pos: 0,
        packed,
        tile: tile.index,
    };
    if let Some(reach) = reach.as_deref_mut() {
        reach.missing_from = geoms.iter().map(|c| vec![None; c.len()]).collect();
        progression::poc_gaps(header, &walk, |comp, res, layer| {
            reach.miss(comp, res, u32::from(layer));
            // The packets left out may add passes to the last segment of
            // any code-block of the resolution, as packets the data never
            // reached may.
            let bands = &mut states[comp][res];
            for precinct in 0..bands.first().map_or(0, |b| b.precincts.len()) {
                mark_unread(bands, precinct);
            }
            Ok(())
        })?;
    } else if budget.poc_coverage {
        progression::check_poc_coverage(header, &walk, tile.index)?;
    }
    let strict = reach.is_none();
    let end_visits = geoms
        .iter()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_mul(END_VISITS);
    let mut log = Vec::new();
    let mut scratch = Scratch::default();
    // Not strict: whether the data has ended.
    let mut ended = false;
    let mut packet_index = 0u32;
    for_each_packet(
        header,
        &walk,
        meter,
        budget.poc_bitmap,
        budget.poc_set,
        |layer, resolution, component, precinct| {
            if let Some(reach) = reach.as_deref_mut() {
                if ended {
                    // Past the end of the data: count the packet, not read.
                    meter.spend(1, WorkRefusal::PacketWalk)?;
                    reach.miss(component, resolution, layer);
                    mark_unread(&mut states[component][resolution], precinct);
                    return Ok(());
                }
            }
            let geom = &geoms[component][resolution];
            let packet = parse_packet(
                &mut streams,
                meter,
                layer,
                packet_index,
                delimiters,
                header.components[component].coding.code_block_style,
                &geom.bands,
                precinct,
                &mut states[component][resolution],
                &mut scratch,
                budget.layers.is_none_or(|cap| layer < u32::from(cap)),
                if strict { None } else { Some(&mut log) },
            )?;
            if packet != Packet::Read {
                ended = true;
                mark_unread(&mut states[component][resolution], precinct);
                meter.spend(end_visits, WorkRefusal::PacketWalk)?;
                if let Some(reach) = reach.as_deref_mut() {
                    reach.miss(component, resolution, layer);
                }
            }
            packet_index += 1;
            Ok(())
        },
    )?;

    let mut components = Vec::with_capacity(states.len());
    for (comp, (component_states, component_geoms)) in states.into_iter().zip(&geoms).enumerate() {
        let rect = header
            .siz
            .tile_component_rect(tile.index, comp)
            .ok_or_else(|| Error::Inconsistent(format!("no component {comp} in `SIZ`")))?;
        let nl = (component_geoms.len() - 1) as u32;
        let mut resolutions = Vec::with_capacity(component_states.len());
        for (r, (band_states, level)) in component_states
            .into_iter()
            .zip(component_geoms)
            .enumerate()
        {
            let subbands = if level.bands.is_empty() {
                empty_subbands(rect, nl, r as u32)
            } else {
                build_subbands(&level.bands, band_states)
            };
            resolutions.push(Resolution { subbands });
        }
        components.push(ComponentCoded { resolutions });
    }

    // Not strict, bytes left after the last packet are noted rather than
    // refused. Data that ran out leaves none.
    if let Some(reach) = reach {
        if !ended {
            if streams.body_pos < data.len() {
                reach.leftover = Some((streams.body_pos, data.len() - streams.body_pos));
            }
            if packed && streams.header_pos < streams.header.len() {
                reach.leftover_packed = Some(streams.header.len() - streams.header_pos);
            }
        }
        return Ok(CodedData { components });
    }

    // The packets tile the tile-part data exactly, with no padding, up to the
    // closing EOC. Any remainder means a misread field — a dropped layer shows up
    // here as leftover bytes (this doubles as the parse self-check).
    if streams.body_pos != data.len() {
        return Err(Error::Codestream(format!(
            "tile {} has {} byte(s) left after the last packet",
            tile.index,
            data.len() - streams.body_pos
        )));
    }
    // The packed headers must be exhausted too: a leftover header byte is the
    // same kind of misread, caught on the header stream instead of the body.
    if packed && streams.header_pos != streams.header.len() {
        return Err(Error::Codestream(format!(
            "tile {} has {} byte(s) of packed packet headers left after the last packet",
            tile.index,
            streams.header.len() - streams.header_pos
        )));
    }

    Ok(CodedData { components })
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

/// What a codeword segment codes. It decides how many passes the segment may
/// hold and what the next segment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    /// Part 1 passes the MQ coder codes: every segment of a block without
    /// `bypass`, and the cleanup segments of one with it.
    Mq,
    /// The raw segment of a `bypass` block: a significance and a refinement
    /// pass coded as raw bits.
    Raw,
    /// An HT cleanup segment (ISO/IEC 15444-15 B.2). A block's first one
    /// holds `placeholders` passes before its cleanup pass, a multiple of
    /// three; every later one holds its cleanup pass alone. `None` while every
    /// pass read is a placeholder pass: the cleanup pass, and so the segment's
    /// end, is still to come (B.3 NOTE 4). See [`ht_first_cleanup`].
    HtCleanup { placeholders: Option<u32> },
    /// An HT refinement segment: a SigProp pass and, if present, a MagRef
    /// pass.
    HtRefinement,
}

/// The kind of the *next* codeword segment, and how many coding passes it may
/// hold, given the block's coder, the code-block style and the last segment
/// opened (`opj_t2_init_seg`).
///
/// `termall` terminates the MQ coder after every pass, so each pass is its own
/// segment. `bypass` (lazy) terminates at every raw/MQ boundary: the first
/// segment holds the ten MQ passes of the four most significant bit-planes, then
/// each lower plane splits into a two-pass raw segment (significance +
/// refinement) and a one-pass MQ segment (cleanup) — the `10, 2, 1, 2, 1, …`
/// pattern OpenJPEG spells with `prev.maxpasses in {1, 10} ? 2 : 1`. Otherwise a
/// segment takes up to 109 passes — `3 * Mb - 2` with `Mb` capped at 37 — which a
/// code-block can never exceed, so the default never splits.
///
/// An HT block's segments alternate by ISO/IEC 15444-15 B.2: after a cleanup
/// segment comes a refinement segment of up to two passes, then a one-pass
/// cleanup segment. Its first segment is opened by [`ht_first_cleanup`]
/// instead, since where it ends depends on the placeholder passes of the
/// contribution that holds the cleanup pass. The other style bits mean nothing
/// to an HT block.
fn next_segment(coder: Coder, style: u8, last: Option<&SegmentState<'_>>) -> (SegmentKind, u32) {
    use crate::codestream::markers::code_block_style::{LAZY, TERMALL};
    let last = last.map(|s| s.kind);
    if coder == Coder::Ht {
        match last {
            Some(SegmentKind::HtCleanup { .. }) => (SegmentKind::HtRefinement, 2),
            _ => (
                SegmentKind::HtCleanup {
                    placeholders: Some(0),
                },
                1,
            ),
        }
    } else if style & TERMALL != 0 {
        // Every pass ends its segment, so nothing turns on the kind.
        (SegmentKind::Mq, 1)
    } else if style & LAZY != 0 {
        match last {
            None => (SegmentKind::Mq, 10),
            Some(SegmentKind::Mq) => (SegmentKind::Raw, 2),
            Some(_) => (SegmentKind::Mq, 1),
        }
    } else {
        (SegmentKind::Mq, 109)
    }
}

/// The width of a length field: `Lblock + floor(log2(n))` bits for a codeword
/// segment of `n` passes from one packet (ISO B.10.7.1). An HT block uses the
/// same rule for each codeword segment (ISO/IEC 15444-15 B.3), so the field of
/// its first cleanup segment counts the placeholder passes in it too.
fn length_bits(lblock: u32, passes: u32) -> u32 {
    lblock + passes.ilog2()
}

/// The longest HT cleanup segment, `Lcup < 65535`, and HT refinement segment,
/// `Lref < 2047` (ISO/IEC 15444-15 7.1.1, 7.1.2).
const HT_CLEANUP_MAX: u64 = 65534;
const HT_REFINEMENT_MAX: u64 = 2046;

/// Add a length field's `bytes` to `segment`, which an HT segment may hold
/// only within its limit. An HT segment's bytes are those of its codeword
/// segments over the layers (B.3), so the limit is on the sum.
fn add_length(segment: &mut SegmentState<'_>, bytes: u32) -> Result<()> {
    segment.length += u64::from(bytes);
    let (what, max) = match segment.kind {
        SegmentKind::HtCleanup { .. } => ("cleanup", HT_CLEANUP_MAX),
        SegmentKind::HtRefinement => ("refinement", HT_REFINEMENT_MAX),
        SegmentKind::Mq | SegmentKind::Raw => return Ok(()),
    };
    if segment.length > max {
        return Err(Error::Codestream(format!(
            "HT {what} segment is {} bytes, over the limit of {max}",
            segment.length
        )));
    }
    Ok(())
}

/// What the first length field of an HT block's contribution is, while the
/// block's cleanup pass is still to come. See [`ht_first_cleanup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstCleanup {
    /// Every pass of the contribution is a placeholder pass.
    Placeholders,
    /// The contribution holds the cleanup pass, `placeholders` passes from
    /// the block's first, and its first codeword segment has `passes` passes,
    /// up to the cleanup pass, and `length` bytes.
    Cleanup {
        placeholders: u32,
        passes: u32,
        length: u32,
    },
}

/// Read the first length field of an HT block's contribution of `new` passes,
/// after `prior` passes that were all placeholders, and say whether the
/// contribution holds the cleanup pass (ISO/IEC 15444-15 B.1 to B.3).
///
/// The packet with a block's first cleanup pass holds no other cleanup pass
/// and at most two refinement passes after it (B.3). If this contribution
/// holds it, it is pass `3 P0` (from 0), `P0 = floor((prior + new - 1) / 3)`,
/// and the contribution's first codeword segment ends there. A contribution
/// of placeholder passes only (NOTE 4) is one codeword segment of all `new`
/// passes, whose length is zero, since placeholder passes carry no bytes.
///
/// The pass counts cannot tell the two apart, but the length can. The first
/// cleanup segment is at least 2 bytes long (B.3), so the field read at the
/// cleanup reading's width is nonzero exactly when the contribution holds the
/// cleanup pass. That width is never more than the placeholder reading's, so
/// a zero is the leading bits of an all-zero field, and the rest of it is read
/// next. When `3 P0` falls before `prior`, the contribution cannot hold the
/// cleanup pass, and its field is read at the placeholder width at once.
fn ht_first_cleanup(
    lblock: u32,
    prior: u32,
    new: u32,
    bio: &mut BitReader,
) -> Result<FirstCleanup> {
    let placeholders = 3 * ((prior + new - 1) / 3);
    let mut read = 0;
    if placeholders >= prior {
        let passes = placeholders + 1 - prior;
        let bits = length_bits(lblock, passes);
        match bio.read(bits) {
            0 => read = bits,
            1 => {
                return Err(Error::Codestream(
                    "HT code-block's first cleanup segment is 1 byte long; it must be at least 2"
                        .into(),
                ));
            }
            length => {
                return Ok(FirstCleanup::Cleanup {
                    placeholders,
                    passes,
                    length,
                });
            }
        }
    }
    if bio.read(length_bits(lblock, new) - read) != 0 {
        return Err(Error::Codestream(
            "HT code-block placeholder passes carry bytes".into(),
        ));
    }
    Ok(FirstCleanup::Placeholders)
}

/// What the first length field of a contribution says about a code-block of
/// a MIXED tile-component whose earlier length fields were all zero. See
/// [`mixed_first_length`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MixedFirst {
    /// The field is zero: the contribution codes no bytes, and the block's
    /// coder is still open.
    Empty,
    /// The block is an HT block, and the contribution holds its cleanup pass.
    Ht(FirstCleanup),
    /// The block is a Part 1 block, and its one codeword segment takes
    /// `length` bytes from the contribution.
    Part1 { length: u32 },
}

/// Read the first length field of a contribution of `new` passes to a
/// code-block of a MIXED tile-component, after `prior` passes whose length
/// fields were all zero, and say which coder the block uses (ISO/IEC
/// 15444-15 A.4).
///
/// The two coders read the field at different widths when the contribution
/// holds an HT cleanup pass and refinement passes after it: an HT block's
/// first field ends at its cleanup pass (see [`ht_first_cleanup`]), a Part 1
/// block's one field spans all `new` passes, since A.4 allows a Part 1 block
/// here neither bypass nor termination on each pass. Both fields start at
/// the same bit, and A.4 requires of an HT block that the first bit of its
/// first nonzero length field be 0 and that `Lblock` be above 3 there. So a
/// nonzero field with `Lblock` at 3 or a leading 1 is a Part 1 block's.
/// Otherwise the field is read as an HT block's, and a value an HT block
/// cannot have (a cleanup length of 1, or bytes on placeholder passes) makes
/// it a Part 1 block's: that is the A.4 note's "processing the code-block
/// assuming it conforms to Annex B", and it fails here, in the length field,
/// or not at all.
///
/// The bits read are the same whichever reading wins: a zero field is
/// `Lblock + floor(log2(new))` bits in both, and a Part 1 field is read to
/// that width. So the choice is made once, here, and the walk never holds
/// two readings of a block.
fn mixed_first_length(
    lblock: u32,
    prior: u32,
    new: u32,
    bio: &mut BitReader,
) -> Result<MixedFirst> {
    let width = length_bits(lblock, new);
    // The HT reading's first field, when the contribution can hold the
    // cleanup pass: its width, and its `placeholders` before the pass.
    let placeholders = 3 * ((prior + new - 1) / 3);
    let cleanup = (placeholders >= prior)
        .then(|| (length_bits(lblock, placeholders + 1 - prior), placeholders));
    // The Part 1 reading of the field, whose first `read` bits, of value
    // `top`, are read already.
    let part1 = |top: u32, read: u32, bio: &mut BitReader| match (top << (width - read))
        | bio.read(width - read)
    {
        0 => MixedFirst::Empty,
        length => MixedFirst::Part1 { length },
    };
    if lblock <= 3 {
        return Ok(part1(0, 0, bio));
    }
    if bio.read_bit() == 1 {
        return Ok(part1(1, 1, bio));
    }
    let Some((bits, placeholders)) = cleanup else {
        return Ok(part1(0, 1, bio));
    };
    match bio.read(bits - 1) {
        top @ (0 | 1) => Ok(part1(top, bits, bio)),
        length => Ok(MixedFirst::Ht(FirstCleanup::Cleanup {
            placeholders,
            passes: placeholders + 1 - prior,
            length,
        })),
    }
}

/// Settle a code-block of a MIXED tile-component as a Part 1 block, once
/// `passes` passes in all have reached it. Its earlier passes, held as
/// placeholder passes of no bytes, become passes of its one MQ codeword
/// segment, which holds every pass a Part 1 block may have: A.4 allows it
/// neither bypass nor termination on each pass.
fn become_part1(block: &mut BlockState<'_>, passes: u32) -> Result<()> {
    let (kind, max_passes) = next_segment(Coder::Part1, 0, None);
    if passes > max_passes {
        return Err(Error::Codestream(format!(
            "Part 1 code-block of a MIXED tile-component has {passes} coding passes, \
             over the {max_passes} one codeword segment can hold"
        )));
    }
    block.coder = Some(Coder::Part1);
    match block.segments.first_mut() {
        Some(segment) => {
            segment.kind = kind;
            segment.max_passes = max_passes;
            block.num_passes += segment.kept;
        }
        None => block.segments.push(SegmentState::new(kind, max_passes)),
    }
    Ok(())
}

/// Which packet delimiters `COD` signals (`Scod` bits 1 and 2).
#[derive(Debug, Clone, Copy)]
struct Delimiters {
    /// SOP *may* precede each packet.
    sop: bool,
    /// EPH *shall* follow each packet header.
    eph: bool,
}

/// The two byte streams a packet is read from, with a cursor into each.
///
/// A packet is a header followed by its body. Normally both are inline in the
/// tile data and the two cursors move in lockstep. When the tile carries `PPT`
/// packed headers, the header stream is that separate buffer and the body stream
/// is still the tile data, so the cursors advance independently (A.7.5). SOP
/// stays in the body stream; EPH moves with the headers.
struct PacketStreams<'a> {
    /// Where packet *headers* are read: the `PPT` buffer, or `data` when inline.
    header: &'a [u8],
    header_pos: usize,
    /// Where packet *bodies* (and SOP markers) are read: always the tile data.
    data: &'a [u8],
    body_pos: usize,
    /// Whether the header stream is separate from the body stream.
    packed: bool,
    /// The tile's index, for an error message.
    tile: u32,
}

/// Why a packet walk was refused for work, with the numbers a caller needs to
/// name a [`WorkLimit`](crate::WorkLimit) factor. It travels beside the
/// [`Error`] the walk fails with (see [`Refused`]), so nothing reads it back
/// out of the message. A new work refusal (the sample-area one, say) is a new
/// variant here, a line in [`WorkRefusal::why`], and a function in
/// `limit_hint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkRefusal {
    /// The walk ran out of visits.
    PacketWalk,
    /// A POC sequence's volumes reach `reached` packets, more than the visits
    /// left.
    PocReach { reached: u64 },
    /// A component's geometry holds `blocks` code-blocks in the resolutions
    /// the reduction drops, more than the visits left (see
    /// [`Budget::reduction`]).
    DroppedBlocks { blocks: u64 },
}

impl WorkRefusal {
    /// The plain message of the [`Error::Limit`] for this refusal, without
    /// the tile or a retry factor. [`decode`](crate::decode) replaces it with
    /// the full message from `limit_hint`.
    pub(crate) fn why(&self) -> String {
        match self {
            WorkRefusal::PacketWalk => "reading the packet headers goes past the packet work \
                                        bound (see `DecodeOptions::packet_work_limit`)"
                .into(),
            WorkRefusal::PocReach { reached } => format!(
                "the `POC` volumes reach {reached} packets, a step each, more than \
                 the steps left of the packet work bound (see \
                 `DecodeOptions::packet_work_limit`)"
            ),
            WorkRefusal::DroppedBlocks { blocks } => format!(
                "the resolutions the reduction drops hold {blocks} code-blocks in \
                 one component, a step each, more than the steps left of the \
                 packet work bound (see `DecodeOptions::packet_work_limit`)"
            ),
        }
    }
}

/// How [`decode_packets_metered`] fails: for work, with the typed reason, or
/// for anything else, as the [`Error`] it is.
#[derive(Debug)]
pub(crate) enum Refused {
    /// The tile went past the packet work bound.
    Work(WorkRefusal),
    /// Any other failure.
    Other(Error),
}

impl From<Refused> for Error {
    fn from(refused: Refused) -> Error {
        match refused {
            Refused::Work(work) => Error::Limit(work.why()),
            Refused::Other(error) => error,
        }
    }
}

/// A decode's packet work count: the visits it started with, what is left,
/// and, once a walk is refused for work, why. The decode owns one and every
/// tile's walk spends it, so there is a single count to read afterwards.
///
/// A visit is a step of a walk that reads no bit: packets read, steps over
/// code-blocks a packet header does not name a bit for, and the points of
/// the progression order's loops, such as each position of a positional
/// sweep for each (component, resolution) it checks. A packet header can
/// be a single byte and still pass over every row of code-blocks of its
/// precinct, and a sweep can check millions of points that hold no
/// packet, so this bounds the work where the other limits cannot.
pub(crate) struct Meter {
    first: u64,
    left: Cell<u64>,
    refusal: Cell<Option<WorkRefusal>>,
}

impl Meter {
    /// A meter with `first` visits to spend.
    pub(crate) fn new(first: u64) -> Self {
        Meter {
            first,
            left: Cell::new(first),
            refusal: Cell::new(None),
        }
    }

    /// The visits it started with.
    pub(crate) fn first(&self) -> u64 {
        self.first
    }

    /// The visits left.
    pub(crate) fn left(&self) -> u64 {
        self.left.get()
    }

    /// The visits spent so far.
    pub(crate) fn spent(&self) -> u64 {
        self.first.saturating_sub(self.left.get())
    }

    /// Why the walk was refused for work, once it is.
    fn refusal(&self) -> Option<WorkRefusal> {
        self.refusal.get()
    }

    /// Forget a refusal an earlier walk recorded, so that it cannot label
    /// this walk's other limit errors as work.
    fn clear_refusal(&self) {
        self.refusal.set(None);
    }

    /// Spend `n` visits, or leave the count as it is and say whether they
    /// were there. For a charge whose refusal is not a [`WorkRefusal`].
    pub(crate) fn try_spend(&self, n: u64) -> bool {
        match self.left.get().checked_sub(n) {
            Some(rest) => {
                self.left.set(rest);
                true
            }
            None => false,
        }
    }

    /// Spend `n` visits, or refuse the walk for `why` when fewer are left.
    pub(crate) fn spend(&self, n: usize, why: WorkRefusal) -> Result<()> {
        if self.try_spend(n as u64) {
            Ok(())
        } else {
            Err(self.refuse(why))
        }
    }

    /// Record `why` as the reason the walk is refused, and give the [`Error`]
    /// to fail it with.
    pub(crate) fn refuse(&self, why: WorkRefusal) -> Error {
        self.refusal.set(Some(why));
        Error::Limit(why.why())
    }
}

/// What the end of a tile's data costs with `strict` off, in visits, for
/// each resolution of each of the tile's components.
///
/// A strict decode stops at the first tile whose data runs out; one with
/// `strict` off goes on to the next. The geometry built for a tile and the
/// decode after its walk cost about a microsecond for each resolution of
/// each component, which the bytes of its packets pay for when they are
/// there. When they are not, a tile-part of 14 bytes would buy that setup
/// for thousands of components, so this charges it: at 32 visits, the
/// default bound's 2^24 visits hold about half a second of it. The data of
/// a codestream a strict decode reads never ends early, so this costs that
/// decode nothing.
const END_VISITS: usize = 32;

/// The error for a packed packet header that reads past the end of the
/// `PPM` or `PPT` bytes it is packed in.
fn packed_overrun() -> Error {
    Error::Codestream(
        "a packet header runs past the end of its `PPM` or `PPT` packed headers".into(),
    )
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
/// The counter therefore lives in [`decode_packets_metered`], which runs once
/// per tile over that tile's tile-parts joined end to end: it starts at zero
/// for each tile and does not restart at a tile-part boundary.
///
/// OpenJPEG leaves `Nsop` unchecked — `/* TODO : check the Nsop value */` — so
/// this is stricter than the oracle. Validating the sequence number is the whole
/// point of a resynchronization marker.
fn read_sop(data: &[u8], pos: usize, packet_index: u32) -> Result<usize> {
    let end = pos + 6;
    if end > data.len() {
        return Err(Error::Codestream("truncated `SOP` marker segment".into()));
    }
    let lsop = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
    if lsop != 4 {
        return Err(Error::Codestream(format!(
            "`SOP` declares `Lsop` {lsop}, expected 4"
        )));
    }
    let nsop = u16::from_be_bytes([data[pos + 4], data[pos + 5]]);
    // Nsop is 16 bits and wraps; a tile with more than 65536 packets restarts it.
    let expected = (packet_index % 65536) as u16;
    if nsop != expected {
        return Err(Error::Codestream(format!(
            "`SOP` sequence number {nsop} for packet {packet_index}, expected {expected}"
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
/// per packet — which a single-layer decoder can get away with — desynchronizes
/// the header bit-reader on the second layer.
struct BlockState<'a> {
    /// The block's coder: the tile-component's, or, under MIXED, `None`
    /// until the block's first nonzero length field settles it (see
    /// [`mixed_first_length`]). While it is `None` every length field read
    /// was zero, which both coders read alike: the block's passes sit in one
    /// open HT cleanup segment of no bytes, as placeholder passes, and become
    /// passes of its one MQ segment if the block turns out to be a Part 1
    /// block.
    coder: Option<Coder>,
    included: bool,
    /// Length-indicator width, grown by a unary run of 1s and never reset.
    lblock: u32,
    /// The zero bit-planes the tag tree gives, without an HT block's skipped
    /// ones (see [`skipped_planes`](Self::skipped_planes)).
    zero_bit_planes: u32,
    /// Coding passes summed over every contributing layer the decode keeps
    /// (see [`Budget::layers`]), an HT block's placeholder passes left out.
    num_passes: u32,
    /// Codeword segments in order; the last one may still take passes.
    segments: Vec<SegmentState<'a>>,
}

/// A codeword segment while it is being filled from successive layers.
struct SegmentState<'a> {
    kind: SegmentKind,
    /// Coding passes from every layer read, which decide where the next
    /// segment starts. An HT block's first cleanup segment counts its
    /// placeholder passes.
    passes: u32,
    /// The passes of [`passes`](Self::passes) from the layers the decode
    /// keeps: all of them without a layer cap. The layers past the cap only
    /// add passes after these, and add no chunks.
    kept: u32,
    /// How many passes this segment may hold before the next one starts:
    /// 1 under `termall`, the `10, 2, 1, 2, 1, …` pattern under `bypass`,
    /// otherwise 109 — the most a code-block can carry (`3 * Mb - 2` with
    /// `Mb <= 37`), so the default never splits. See [`next_segment`],
    /// which mirrors OpenJPEG's `opj_t2_init_seg`. An HT block's first
    /// cleanup segment holds its placeholder passes and the cleanup pass,
    /// and until the cleanup pass comes it takes any number.
    max_passes: u32,
    /// The bytes the length fields gave it, over every layer read.
    length: u64,
    chunks: Vec<&'a [u8]>,
}

impl SegmentState<'_> {
    fn new(kind: SegmentKind, max_passes: u32) -> Self {
        SegmentState {
            kind,
            passes: 0,
            kept: 0,
            max_passes,
            length: 0,
            chunks: Vec::new(),
        }
    }

    /// The placeholder passes of [`kept`](Self::kept): all of them while the
    /// cleanup pass is still to come.
    fn kept_placeholders(&self) -> u32 {
        match self.kind {
            SegmentKind::HtCleanup {
                placeholders: Some(p),
            } => self.kept.min(p),
            SegmentKind::HtCleanup { placeholders: None } => self.kept,
            _ => 0,
        }
    }

    /// The kept passes Tier-1 decodes: [`kept`](Self::kept) without the
    /// placeholder passes, which code nothing.
    fn coded_kept(&self) -> u32 {
        self.kept - self.kept_placeholders()
    }
}

impl BlockState<'_> {
    fn new(coder: Option<Coder>) -> Self {
        BlockState {
            coder,
            included: false,
            lblock: 3,
            zero_bit_planes: 0,
            num_passes: 0,
            segments: Vec::new(),
        }
    }

    /// The magnitude bit-planes an HT block skips past those the tag tree
    /// gives: one per three placeholder passes (`P0`, ISO/IEC 15444-15 B.3),
    /// counted only when the decode keeps the cleanup pass after them. A
    /// block without its cleanup pass decodes to zeros whatever the count.
    fn skipped_planes(&self) -> u32 {
        match self.segments.first() {
            Some(
                s @ SegmentState {
                    kind:
                        SegmentKind::HtCleanup {
                            placeholders: Some(p),
                        },
                    ..
                },
            ) if s.coded_kept() > 0 => p / 3,
            _ => 0,
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
    /// With `strict` off: whether a packet of the precinct was not read, or
    /// read only in part, because the data ended, or is one the `POC`
    /// volumes leave out. Its code-blocks may then lack passes a later layer
    /// adds to their last segment.
    unread: bool,
}

impl BandState<'_> {
    /// `coder` is the tile-component's coder, `None` under MIXED.
    fn new(band: &BandGeom, coder: Option<Coder>) -> Self {
        BandState {
            precincts: band
                .precincts
                .iter()
                .map(|precinct| PrecinctState {
                    inclusion: TagTree::new(precinct.cols as u32, precinct.rows as u32),
                    zero_bits: TagTree::new(precinct.cols as u32, precinct.rows as u32),
                    unread: false,
                })
                .collect(),
            blocks: (0..band.block_count())
                .map(|_| BlockState::new(coder))
                .collect(),
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

/// The buffers a packet read fills and empties again, kept across the
/// packets of a walk so that a packet allocates nothing of its own.
#[derive(Default)]
struct Scratch {
    /// The packet's contributions, in header order.
    contributions: Vec<Contribution>,
    /// The unread spans of one band's inclusion pass.
    unread: UnreadSpans,
}

/// How a packet read ended, when the walk is not strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Packet {
    /// The packet was read whole.
    Read,
    /// The data ended at or inside the packet. What it holds of whole
    /// code-blocks is kept; the rest of it is undone.
    Ended,
}

/// A code-block's state before a packet added to it, so that the packet can
/// be undone (see [`undo`]).
struct Undo {
    band: usize,
    block: usize,
    coder: Option<Coder>,
    included: bool,
    lblock: u32,
    zero_bit_planes: u32,
    num_passes: u32,
    /// How many segments the block had, and the state of the last of them.
    segments: usize,
    last: Option<LastSegment>,
}

/// The state of a block's last segment before a packet, which the packet may
/// add passes and bytes to, or, for an HT block's first cleanup segment, end.
struct LastSegment {
    kind: SegmentKind,
    passes: u32,
    kept: u32,
    max_passes: u32,
    length: u64,
    chunks: usize,
}

impl LastSegment {
    fn of(segment: &SegmentState<'_>) -> Self {
        LastSegment {
            kind: segment.kind,
            passes: segment.passes,
            kept: segment.kept,
            max_passes: segment.max_passes,
            length: segment.length,
            chunks: segment.chunks.len(),
        }
    }
}

/// Put back the state of every block in `log`, latest first, so that the
/// packet that logged them adds nothing to those blocks. Their earlier
/// layers stay.
fn undo(log: &[Undo], states: &mut [BandState<'_>]) {
    for entry in log.iter().rev() {
        let block = &mut states[entry.band].blocks[entry.block];
        block.coder = entry.coder;
        block.included = entry.included;
        block.lblock = entry.lblock;
        block.zero_bit_planes = entry.zero_bit_planes;
        block.num_passes = entry.num_passes;
        block.segments.truncate(entry.segments);
        if let (Some(last), Some(was)) = (block.segments.last_mut(), &entry.last) {
            last.kind = was.kind;
            last.passes = was.passes;
            last.kept = was.kept;
            last.max_passes = was.max_passes;
            last.length = was.length;
            last.chunks.truncate(was.chunks);
        }
    }
}

/// Parse one packet — one precinct of one resolution of one component in one
/// layer — starting at byte `start` of the tile-part `data`.
///
/// Folds the packet's contributions into `states` and returns the byte offset
/// where the next packet begins. The subbands are built once every layer has
/// been read; see [`build_subbands`].
///
/// With `keep` false (a layer past the decode's cap, see [`Budget::layers`]),
/// the packet is read the same way and its header still moves each
/// code-block's state on, but its passes and bytes are not added.
///
/// With `log` given (a walk with `strict` off), running out of data is not a
/// fault. A packet that ends past the data adds the code-blocks whose data is
/// all there and undoes the rest, from the first code-block whose data runs
/// out on, back to their earlier layers; a packet whose inline header runs out
/// adds nothing. Either is [`Packet::Ended`]. Every other fault fails as it
/// does when strict, a packed header that runs out included, and a packet the
/// strict walk reads is read the same way.
#[allow(clippy::too_many_arguments)]
fn parse_packet<'a>(
    streams: &mut PacketStreams<'a>,
    meter: &Meter,
    layer: u32,
    packet_index: u32,
    delimiters: Delimiters,
    style: u8,
    bands: &[BandGeom],
    precinct: usize,
    states: &mut [BandState<'a>],
    scratch: &mut Scratch,
    keep: bool,
    mut log: Option<&mut Vec<Undo>>,
) -> Result<Packet> {
    meter.spend(1, WorkRefusal::PacketWalk)?;
    let partial = log.is_some();
    if partial
        && delimiters.sop
        && peek_marker(streams.data, streams.body_pos) == Some(marker::SOP)
        && streams.body_pos + 6 > streams.data.len()
    {
        return Ok(Packet::Ended);
    }
    // SOP precedes the packet in the *body* stream when COD allows it — but only
    // *may* (A.8.1), so its absence is not an error, and it stays inline even
    // when the headers are packed. OpenJPEG warns and carries on.
    //
    // The peek is unambiguous: a packet header can never begin `FF 91`. Its
    // first byte may well be `0xFF`, but the header's bit stuffing then forces
    // the next byte's most significant bit to zero, so that byte is at most
    // `0x7F` and never `0x91`. See [`bio`].
    if delimiters.sop && peek_marker(streams.data, streams.body_pos) == Some(marker::SOP) {
        streams.body_pos = read_sop(streams.data, streams.body_pos, packet_index)?;
    }

    // Inline, the header starts where the body cursor now sits (past any SOP);
    // packed, it continues in the separate buffer.
    if !streams.packed {
        streams.header_pos = streams.body_pos;
    }
    // A packet header occupies at least one byte, so a cursor at or past the end
    // of the header stream means the codestream promised more packets than it
    // carries.
    if streams.header_pos >= streams.header.len() {
        if partial {
            return Ok(Packet::Ended);
        }
        return Err(Error::Codestream(format!(
            "the data of tile {} ends before the last packet",
            streams.tile
        )));
    }

    let mut bio = BitReader::new(&streams.header[streams.header_pos..]);
    if let Some(log) = log.as_deref_mut() {
        log.clear();
    }

    // The first bit flags an empty packet (no contributions) against a present
    // one. An empty packet still costs its layer: the tag trees are untouched,
    // not reset.
    let present = bio.read_bit() == 1;
    let contributions = &mut scratch.contributions;
    contributions.clear();
    let mut fault = None;
    if present {
        for (band_index, (band, state)) in bands.iter().zip(states.iter_mut()).enumerate() {
            let parsed = parse_band_header(
                meter,
                band_index,
                &band.precincts[precinct],
                &mut state.precincts[precinct],
                &mut state.blocks,
                layer,
                style,
                keep,
                &mut bio,
                contributions,
                &mut scratch.unread,
                log.as_deref_mut(),
            );
            if let Err(error) = parsed {
                fault = Some(error);
                break;
            }
        }
    }
    // A packed header that reads past its bytes is damaged, strict or not:
    // the bits it needs are not in the codestream. Its bytes sit in the main
    // or tile-part headers, which are there whole or not at all, so this is
    // never the data ending. It is checked before any fault, which the fill
    // bits may have caused, so the overrun is what is reported.
    if streams.packed && bio.overran() {
        return Err(packed_overrun());
    }
    if let Some(error) = fault {
        // Not strict, a fault in an inline header that runs past the data is
        // the data ending, not damage: the bits past the end are fill.
        return match log.as_deref() {
            Some(log) if bio.overran() => {
                undo(log, states);
                Ok(Packet::Ended)
            }
            _ => Err(error),
        };
    }

    // The header is a whole number of bytes. EPH, when COD signals it, sits
    // between the header and the body — after an empty packet's header too — and
    // moves into the packed buffer with the headers, so it is read from the
    // header stream.
    //
    // Not strict, an inline header whose bits run past the data is the data
    // ending. That is taken before the alignment, which past a last byte of
    // 0xFF reads one more byte that a strict decode does not ask for. Packed
    // headers are not checked here: their bytes sit in the main or tile-part
    // headers, which are there whole or not at all, so one that runs short
    // was refused above, as damage, and one the alignment runs past is
    // refused below.
    let header_overran = bio.overran() && !streams.packed;
    bio.align();
    // A packed header whose last byte is 0xFF needs the stuffed byte after it
    // (B.10.1). Without one the header is damaged, as OpenJPEG's
    // `opj_bio_inalign` also finds.
    if streams.packed && bio.overran() {
        return Err(packed_overrun());
    }
    streams.header_pos += bio.bytes_consumed();
    if let Some(log) = log.as_deref() {
        let eph_cut = delimiters.eph && streams.header_pos + 2 > streams.header.len();
        if header_overran || eph_cut {
            undo(log, states);
            return Ok(Packet::Ended);
        }
    }
    if delimiters.eph {
        if peek_marker(streams.header, streams.header_pos) != Some(marker::EPH) {
            return Err(Error::Codestream(
                "`COD` signals `EPH` but the packet header is not followed by one".into(),
            ));
        }
        streams.header_pos += 2;
    }

    // Inline, the body follows the header in the same buffer.
    if !streams.packed {
        streams.body_pos = streams.header_pos;
    }
    for contribution in contributions.iter() {
        let end = match streams
            .body_pos
            .checked_add(contribution.seg_len)
            .filter(|&e| e <= streams.data.len())
        {
            Some(end) => end,
            None => {
                if let Some(log) = log.as_deref() {
                    // Keep the blocks before this one; undo this one and every
                    // block after it in the header.
                    let from = log
                        .iter()
                        .position(|u| (u.band, u.block) == (contribution.band, contribution.block))
                        .unwrap_or(0);
                    undo(&log[from..], states);
                    return Ok(Packet::Ended);
                }
                return Err(Error::Codestream(format!(
                    "a packet body segment overruns the data of tile {}",
                    streams.tile
                )));
            }
        };
        if keep {
            states[contribution.band].blocks[contribution.block].segments[contribution.segment]
                .chunks
                .push(&streams.data[streams.body_pos..end]);
        }
        streams.body_pos = end;
    }
    // Inline, keep the two cursors together for the next packet.
    if !streams.packed {
        streams.header_pos = streams.body_pos;
    }

    Ok(Packet::Read)
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
///
/// A block not yet included can sit under an inclusion-tree node already
/// known to lie past this layer. Its entry then reads no bit, nor does any
/// other entry under that node, so the walk steps over that node's blocks at
/// once (see [`TagTree::unread_span`]): along the row, or over all of its rows
/// when it spans the whole precinct width, and again on each row below that
/// it reaches (see [`UnreadSpans`]). Each such step spends a visit. Any
/// other entry reads at least one bit, so its work is paid for by the header's
/// bytes and is not counted.
///
/// With `keep` false the entries are read the same way, and the passes still
/// fill the segments so that the next layer's lengths are read at the right
/// width, but they are not counted as kept (see [`SegmentState::kept`]).
#[allow(clippy::too_many_arguments)]
fn parse_band_header(
    meter: &Meter,
    band_index: usize,
    precinct: &PrecinctGeom,
    trees: &mut PrecinctState,
    blocks: &mut [BlockState<'_>],
    layer: u32,
    style: u8,
    keep: bool,
    bio: &mut BitReader,
    contributions: &mut Vec<Contribution>,
    unread: &mut UnreadSpans,
    mut log: Option<&mut Vec<Undo>>,
) -> Result<()> {
    let cols = precinct.cols as u32;
    let threshold = layer + 1;

    unread.clear();
    let mut entry = 0;
    while entry < precinct.cols * precinct.rows {
        let bx = entry as u32 % cols;
        let by = entry as u32 / cols;
        let block_index = precinct.block(bx as usize, by as usize);
        entry += 1;

        let was_included = blocks[block_index].included;
        let span = if was_included {
            None
        } else {
            unread.find(&trees.inclusion, bx, by, threshold)
        };
        if let Some(span) = span {
            meter.spend(1, WorkRefusal::PacketWalk)?;
            entry = span.next_entry(by, cols);
            continue;
        }
        let contributes = if was_included {
            // Already included: one bit says whether this layer adds passes.
            bio.read_bit() == 1
        } else {
            // Not yet included: the inclusion tree resolves to the first layer
            // that carries this block. Reading at `layer + 1` asks "is that
            // layer at most this one?" and leaves the tree part-decoded when it
            // is not, so the next layer resumes where this one stopped.
            match trees.inclusion.read(bx, by, threshold, bio) {
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
        if let Some(log) = log.as_deref_mut() {
            let block = &blocks[block_index];
            log.push(Undo {
                band: band_index,
                block: block_index,
                coder: block.coder,
                included: was_included,
                lblock: block.lblock,
                zero_bit_planes: block.zero_bit_planes,
                num_passes: block.num_passes,
                segments: block.segments.len(),
                last: block.segments.last().map(LastSegment::of),
            });
        }

        // The zero-bitplane tree is resolved in full the first time a block is
        // included, so a single read at the ceiling settles it. Later layers
        // never read it again.
        if !was_included {
            blocks[block_index].zero_bit_planes = trees
                .zero_bits
                .read(bx, by, ZBP_LIMIT, bio)
                .ok_or_else(|| {
                    Error::Codestream(format!(
                        "code-block zero-bitplane count is over {}, the most a valid stream \
                         can signal",
                        ZBP_LIMIT - 1
                    ))
                })?;
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
        // (`termall`, `bypass`); there OpenJPEG's `do { ... } while (n > 0)`
        // reads a length per segment, which the loop below mirrors.
        let block = &mut blocks[block_index];
        while bio.read_bit() == 1 {
            block.lblock += 1;
            if block.lblock > LBLOCK_MAX {
                return Err(Error::Codestream(format!(
                    "code-block `Lblock` grows past the limit of {LBLOCK_MAX}"
                )));
            }
        }
        // The passes are handed to codeword segments in turn, each with its own
        // length field. Without termination one segment swallows them all;
        // under `restart` each pass terminates, so each gets a segment and a
        // length of its own. OpenJPEG spells this `do { ... } while (n > 0)`.
        //
        // An HT block's segments follow ISO/IEC 15444-15 B.2 instead (see
        // [`next_segment`]), once its first cleanup segment has ended. Until
        // then a contribution may be placeholder passes only, or end that
        // segment at its cleanup pass, which [`ht_first_cleanup`] tells apart.
        let mut remaining = num_passes;
        let mut placeholders = 0;
        let open = SegmentKind::HtCleanup { placeholders: None };
        let first_open = block.segments.first().is_none_or(|s| s.kind == open);
        let prior = block.segments.first().map_or(0, |s| s.passes);
        // A Part 1 block's length field, when reading it settled the coder
        // of a MIXED block.
        let mut part1_length = None;
        let first = match block.coder {
            Some(Coder::Ht) if first_open => {
                Some(ht_first_cleanup(block.lblock, prior, num_passes, bio)?)
            }
            Some(_) => None,
            None => match mixed_first_length(block.lblock, prior, num_passes, bio)? {
                MixedFirst::Empty => Some(FirstCleanup::Placeholders),
                MixedFirst::Ht(first) => {
                    block.coder = Some(Coder::Ht);
                    Some(first)
                }
                MixedFirst::Part1 { length } => {
                    become_part1(block, prior + num_passes)?;
                    part1_length = Some(length);
                    None
                }
            },
        };
        if let Some(first) = first {
            if block.segments.is_empty() {
                block.segments.push(SegmentState::new(open, u32::MAX));
            }
            let segment = &mut block.segments[0];
            let passes = match first {
                FirstCleanup::Placeholders => {
                    placeholders = num_passes;
                    num_passes
                }
                FirstCleanup::Cleanup {
                    placeholders: before,
                    passes,
                    length,
                } => {
                    placeholders = before - prior;
                    segment.kind = SegmentKind::HtCleanup {
                        placeholders: Some(before),
                    };
                    segment.max_passes = before + 1;
                    add_length(segment, length)?;
                    contributions.push(Contribution {
                        band: band_index,
                        block: block_index,
                        segment: 0,
                        seg_len: length as usize,
                    });
                    passes
                }
            };
            segment.passes += passes;
            if keep {
                segment.kept += passes;
            }
            remaining -= passes;
        }
        if keep {
            block.num_passes += num_passes - placeholders;
        }
        while remaining > 0 {
            if block
                .segments
                .last()
                .is_none_or(|s| s.passes >= s.max_passes)
            {
                let coder = block.coder.unwrap_or(Coder::Ht);
                let (kind, max_passes) = next_segment(coder, style, block.segments.last());
                block.segments.push(SegmentState::new(kind, max_passes));
            }
            let segment_index = block.segments.len() - 1;
            let segment = &mut block.segments[segment_index];
            let new_passes = remaining.min(segment.max_passes - segment.passes);

            // The length field's width uses *this* segment's new passes.
            let seg_len = match part1_length.take() {
                Some(length) => length,
                None => bio.read(length_bits(block.lblock, new_passes)),
            };
            add_length(segment, seg_len)?;
            let seg_len = seg_len as usize;

            // A layer may add coding passes and no bytes (B.10.7 sets no
            // lower bound on the length). The MQ codeword is continuous
            // within a segment, so the encoder's rate split can put passes in
            // one layer and the bytes carrying them in the next. A segment
            // that never gets a byte is decoded from the `0xFF` fill (D.4.1),
            // as OpenJPEG decodes it. The passes still count and still fill
            // the segment, so a later layer's passes land in the right one.
            // A zero length files no chunk, and reading it took at least the
            // header bits above, so it costs no more than one with bytes.
            let segment = &mut block.segments[segment_index];
            segment.passes += new_passes;
            if keep {
                segment.kept += new_passes;
            }
            if seg_len > 0 {
                contributions.push(Contribution {
                    band: band_index,
                    block: block_index,
                    segment: segment_index,
                    seg_len,
                });
            }

            remaining -= new_passes;
        }
    }
    Ok(())
}

/// The subbands of resolution `r` of a tile-component on `rect` with `nl`
/// levels, when the resolution holds no precinct. Each is empty and owns no
/// code-block, so the geometry has none of them (see
/// [`ResolutionGeom`](geometry::ResolutionGeom)), but the inverse DWT still
/// reads their origins and sizes. Their code-block grid is 0 x 0.
fn empty_subbands<'a>(rect: Rect, nl: u32, r: u32) -> Vec<Subband<'a>> {
    geometry::band_rects(rect, nl, r)
        .map(|(kind, x0, x1, y0, y1)| {
            debug_assert!(
                x1 <= x0 || y1 <= y0,
                "band of an empty resolution is not empty"
            );
            Subband {
                kind,
                origin: (x0.max(0) as u32, y0.max(0) as u32),
                width: (x1 - x0).max(0) as usize,
                height: (y1 - y0).max(0) as usize,
                block_cols: 0,
                block_rows: 0,
                blocks: Vec::new(),
            }
        })
        .collect()
}

/// Note that a packet of `precinct` was not read whole (see
/// [`PrecinctState::unread`]). A packet spans every band of the resolution.
fn mark_unread(bands: &mut [BandState<'_>], precinct: usize) {
    for band in bands {
        if let Some(state) = band.precincts.get_mut(precinct) {
            state.unread = true;
        }
    }
}

/// Turn the accumulated per-layer state of one resolution into its subbands.
///
/// A block may end up with coding passes and no bytes, in one codeword segment
/// or in all of them, since B.10.7 lets a contribution's length be zero. Tier-1
/// decodes such a segment from the `0xFF` bytes D.4.1 appends, as OpenJPEG
/// does.
fn build_subbands<'a>(bands: &[BandGeom], states: Vec<BandState<'a>>) -> Vec<Subband<'a>> {
    bands
        .iter()
        .zip(states)
        .map(|(band, state)| {
            // The rectangles come from a nested loop, which gives no exact
            // length, so the vector is sized up front rather than grown.
            debug_assert_eq!(
                state.blocks.len(),
                band.block_count(),
                "one block state per code-block of the band"
            );
            // The code-blocks of the precincts the data ended in or before.
            // A precinct owns a rectangle of the band's blocks, and each
            // block lies in one precinct, so this visits each block at most
            // once. A walk that read every packet allocates nothing here.
            let mut unread = Vec::new();
            for (geom, precinct) in band.precincts.iter().zip(&state.precincts) {
                if precinct.unread {
                    unread.resize(state.blocks.len(), false);
                    for by in 0..geom.rows {
                        for bx in 0..geom.cols {
                            if let Some(flag) = unread.get_mut(geom.block(bx, by)) {
                                *flag = true;
                            }
                        }
                    }
                }
            }
            let mut blocks = Vec::with_capacity(state.blocks.len());
            blocks.extend(band.block_rects().zip(state.blocks).enumerate().map(
                |(index, ((x, y, width, height), block))| {
                    let unread = unread.get(index).copied().unwrap_or(false);
                    // Under a layer cap, the segments the kept layers never
                    // reached are left out, and the last one kept may hold
                    // only some of its passes. Without a cap every pass is
                    // kept. The filter is what keeps a cut segment last:
                    // Tier-1 exempts only the last segment from `predictable
                    // termination`, so an empty segment left after the cut
                    // one would have it checked and refused.
                    //
                    // An HT block's placeholder passes code nothing, so they
                    // count as neither passes nor a segment (see
                    // [`SegmentState::coded_kept`]); they skip bit-planes
                    // instead.
                    //
                    // With `strict` off, a block whose precinct the data
                    // ended in may have lost later passes of its last
                    // segment, unless that segment holds all the passes it
                    // can: then the next pass opens a new segment, and this
                    // one is whole.
                    let mut cut = false;
                    let coder = block.coder.unwrap_or(Coder::Ht);
                    let zero_bit_planes = block.zero_bit_planes + block.skipped_planes();
                    let placeholders = block
                        .segments
                        .first()
                        .map_or(0, SegmentState::kept_placeholders);
                    let segments = block
                        .segments
                        .into_iter()
                        .take_while(|s| s.coded_kept() > 0)
                        .map(|s| {
                            cut = s.kept < s.passes || (unread && s.passes < s.max_passes);
                            CodedSegment {
                                passes: s.coded_kept(),
                                chunks: s.chunks,
                            }
                        })
                        .collect();
                    CodeBlock {
                        x,
                        y,
                        width,
                        height,
                        num_passes: block.num_passes,
                        zero_bit_planes,
                        segments,
                        cut,
                        placeholders,
                        coder,
                    }
                },
            ));
            Subband {
                kind: band.kind,
                origin: band.origin,
                width: band.width,
                height: band.height,
                block_cols: band.block_cols,
                block_rows: band.block_rows,
                blocks,
            }
        })
        .collect()
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
mod partial_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod work_bound_tests;
