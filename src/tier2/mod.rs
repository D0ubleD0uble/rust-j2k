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
pub(crate) mod resume;
pub mod spans;
pub mod tagtree;

use crate::codestream::markers::Rect;
use crate::codestream::markers::marker;
use crate::codestream::{MainHeader, Tile};
use crate::{Error, Result};
use bio::BitReader;
use spans::UnreadSpans;
use std::borrow::Cow;
use std::cell::Cell;
use tagtree::{Saved, TagTree};

pub(crate) use geometry::{
    BandGeom, MAX_CODE_BLOCKS, MAX_PRECINCTS, PrecinctGeom, ResolutionGeom, check_counts,
    check_tile, resolution_geoms, tile_counts, within_caps,
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
    pub segments: Segments<'a>,
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
    /// With `strict` off, the first layer whose packet of the block's
    /// precinct was not read whole; `None` when every packet of it was read,
    /// and always when strict.
    pub missing_from: Option<u32>,
}

/// A code-block's codeword segments, in order: per segment its coding passes
/// and the bytes coding them, which a multi-layer codestream may deliver in
/// several chunks.
///
/// Without per-pass termination a block has exactly one segment, and the MQ
/// codeword runs continuously across every layer that contributed to it. Under
/// `restart` (`termall`) each coding pass is terminated separately, so each is
/// its own segment and Tier-1 must re-initialize the MQ decoder for it.
///
/// A segment is held as its pass count and where its chunks end, 8 bytes,
/// and the chunks of all the block's segments share one list. A packet header
/// can open a segment for 3 bits (`Lblock` is at least 3), so a record of its
/// own per segment, with a list of chunks each, costs many times the input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segments<'a> {
    heads: Box<[SegmentHead]>,
    chunks: Box<[&'a [u8]]>,
}

/// One segment of [`Segments`]: its passes, and where its chunks end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SegmentHead {
    passes: u32,
    /// The end of the segment's chunks in [`Segments::chunks`]. They start
    /// where the segment before ends.
    end: u32,
}

/// One codeword segment of a [`Segments`]: the coding passes it carries and
/// the chunks of bytes coding them, in layer order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodedSegment<'s, 'a> {
    pub passes: u32,
    pub chunks: &'s [&'a [u8]],
}

impl<'a> CodedSegment<'_, 'a> {
    /// The segment's bytes, concatenated across the layers that carried them.
    /// A segment of one chunk is borrowed from the codestream, not copied.
    pub fn bytes(&self) -> Cow<'a, [u8]> {
        match self.chunks[..] {
            [] => Cow::Borrowed(&[]),
            [chunk] => Cow::Borrowed(chunk),
            _ => Cow::Owned(self.chunks.concat()),
        }
    }
}

/// A chunk or segment index of one code-block as a `u32`. A block has
/// fewer than 2^32 of either: a contribution's segments and chunks are at
/// most its passes, at most 164, and a precinct has at most 65535 layers.
fn index_u32(index: usize) -> u32 {
    debug_assert!(u32::try_from(index).is_ok());
    index as u32
}

impl<'a> Segments<'a> {
    /// The segments of `heads` over `chunks`.
    fn from_parts(heads: Vec<SegmentHead>, chunks: Vec<&'a [u8]>) -> Self {
        debug_assert!(heads.windows(2).all(|w| w[0].end <= w[1].end));
        debug_assert!(heads.last().is_none_or(|h| h.end as usize == chunks.len()));
        Segments {
            heads: heads.into_boxed_slice(),
            chunks: chunks.into_boxed_slice(),
        }
    }

    /// How many segments the block has.
    pub fn len(&self) -> usize {
        self.heads.len()
    }

    /// Whether the block has no segment.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }

    /// Segment `index`, or `None` past the last.
    pub fn get(&self, index: usize) -> Option<CodedSegment<'_, 'a>> {
        let head = self.heads.get(index)?;
        let start = match index.checked_sub(1) {
            Some(before) => self.heads[before].end as usize,
            None => 0,
        };
        Some(CodedSegment {
            passes: head.passes,
            chunks: &self.chunks[start..head.end as usize],
        })
    }

    /// The first segment, or `None` when there is none.
    pub fn first(&self) -> Option<CodedSegment<'_, 'a>> {
        self.get(0)
    }

    /// The segments in order.
    pub fn iter(&self) -> impl Iterator<Item = CodedSegment<'_, 'a>> {
        (0..self.len()).filter_map(|index| self.get(index))
    }

    /// Add a segment of `passes` passes, coded by `chunks`, after the others.
    /// It copies the segments there, so it is for building a block by hand.
    pub fn push(&mut self, passes: u32, chunks: &[&'a [u8]]) {
        let mut all = std::mem::take(&mut self.chunks).into_vec();
        all.extend_from_slice(chunks);
        let mut heads = std::mem::take(&mut self.heads).into_vec();
        heads.push(SegmentHead {
            passes,
            end: index_u32(all.len()),
        });
        *self = Segments::from_parts(heads, all);
    }
}

impl<'a, C: AsRef<[&'a [u8]]>> FromIterator<(u32, C)> for Segments<'a> {
    /// Segments from `(passes, chunks)` pairs, for building a block by hand.
    fn from_iter<I: IntoIterator<Item = (u32, C)>>(iter: I) -> Self {
        let mut segments = Segments::default();
        for (passes, chunks) in iter {
            segments.push(passes, chunks.as_ref());
        }
        segments
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
/// level, `[Hl, Lh, Hh]` at every finer level, or none when the resolution
/// holds no precinct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<'a> {
    pub subbands: Vec<Subband<'a>>,
}

/// One component's coded data, grouped by resolution (coarsest first, index 0
/// the `NLLL` band) so Tier-1 can decode each block independently and the
/// assembly stage can place it back.
///
/// A resolution that holds no precinct lists no subbands: each is empty and
/// owns no code-block. The inverse DWT still needs their origins and sizes,
/// which [`band`](Self::band) gives from `rect`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentCoded<'a> {
    pub resolutions: Vec<Resolution<'a>>,
    /// The tile-component's rectangle on its own grid, as the walk saw it.
    /// `None` for coded data built by hand.
    pub rect: Option<Rect>,
}

/// Where a subband sits and how big it is: [`Subband`] without its blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandExtent {
    pub origin: (u32, u32),
    pub width: usize,
    pub height: usize,
}

impl BandExtent {
    /// The extent of a band on `[x0, x1) x [y0, y1)`, empty where the bounds
    /// meet or cross.
    fn of_bounds(x0: i64, x1: i64, y0: i64, y1: i64) -> Self {
        BandExtent {
            origin: (x0.max(0) as u32, y0.max(0) as u32),
            width: (x1 - x0).max(0) as usize,
            height: (y1 - y0).max(0) as usize,
        }
    }
}

impl ComponentCoded<'_> {
    /// The origin and size of the `kind` band of resolution `r`: what the
    /// resolution lists or, when it lists none because it holds no precinct,
    /// what its rectangle gives (see [`geometry::band_rects`]). `None` for a
    /// band that does not exist.
    pub fn band(&self, r: usize, kind: BandKind) -> Option<BandExtent> {
        let resolution = self.resolutions.get(r)?;
        if let Some(sb) = resolution.subbands.iter().find(|s| s.kind == kind) {
            return Some(BandExtent {
                origin: sb.origin,
                width: sb.width,
                height: sb.height,
            });
        }
        if !resolution.subbands.is_empty() {
            return None;
        }
        let nl = u32::try_from(self.resolutions.len() - 1).ok()?;
        geometry::band_rects(self.rect?, nl, u32::try_from(r).ok()?)
            .find(|b| b.0 == kind)
            .map(|(_, x0, x1, y0, y1)| BandExtent::of_bounds(x0, x1, y0, y1))
    }

    /// The `HL`, `LH` and `HH` bands of resolution `r` when it lists none
    /// (see [`band`](Self::band)) and is not the coarsest.
    pub fn detail_extents(&self, r: usize) -> Option<[BandExtent; 3]> {
        if r == 0 || !self.resolutions.get(r)?.subbands.is_empty() {
            return None;
        }
        let nl = u32::try_from(self.resolutions.len() - 1).ok()?;
        let mut bands = geometry::band_rects(self.rect?, nl, u32::try_from(r).ok()?)
            .map(|(_, x0, x1, y0, y1)| BandExtent::of_bounds(x0, x1, y0, y1));
        Some([bands.next()?, bands.next()?, bands.next()?])
    }
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
/// bits, and no more than the visits left, and past it as a count per
/// precinct (see [`for_each_packet`]). The code-blocks of the resolutions that
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
        ..Budget::decode(meter.left(), poc_bitmap).within(meter.cap())
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
/// tiles' packets. A visit costs about 1 to 55 ns (see
/// [`DECODE_VISITS_PER_BYTE`]), so this is under a second. A packet a POC
/// marker's volumes reach again is skipped from a bitmap without a visit, in
/// about 4 ns; skipped by the per-precinct counts a sequence past the bitmap
/// keeps, it is charged 12 visits (see `progression::for_each_packet`).
pub(crate) const DECODE_VISITS_MIN: u64 = 1 << 24;

/// The visits a decode may spend per byte of its tiles' data, when that comes
/// to more than [`DECODE_VISITS_MIN`]. The cheapest visits, the layer
/// orders' steps over resolutions with no precinct, refuse a file at about
/// 0.02 to 0.06 µs a byte, 1 to 2 ns a visit. The costliest measured are a
/// positional walk's points over 16384 components and 200 tiles, each with
/// a packet to read: 24 to 28 ns a visit over 32 `POC` volumes and 33 to
/// 55 ns with none, the packet reads included. So the bound comes to between
/// about 0.02 and 1.8 µs a byte, against the 0.19 to 0.73 µs a byte the
/// conformance codestreams past 100 KB take to decode (the best of five
/// release-build decodes of each). At most, a file past the floor is refused
/// in about 2.5 times the slowest conformance decode per byte.
///
/// Setting up a tile's geometry is counted only for the code-blocks of the
/// resolutions a reduction drops (see [`Budget::reduction`]) and for the
/// components a tile holds no sample of (see [`empty_components`]). A resolution with no precinct
/// gets no band or code-block geometry and lists no subband, so its setup
/// allocates nothing. The costliest shape measured, 64 tiles of 16384
/// components with one byte of packet data each, decodes at about 6 µs a
/// byte, about 1.2 times what OpenJPEG takes.
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
    /// [`poc_bitmap`](Self::poc_bitmap) holds them as a count per precinct;
    /// if not, its walk fails.
    pub(crate) poc_counts: bool,
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
    /// [`Meter`], or `poc_bitmap`, then a count per precinct. A walk that
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
            poc_counts: true,
            poc_coverage: true,
            reduction: 0,
            layers: None,
        }
    }

    /// This budget with the geometry guards of a tile under the sample limit
    /// `cap` ([`Cap::tile_precincts`](crate::samples::Cap::tile_precincts),
    /// [`Cap::tile_blocks`](crate::samples::Cap::tile_blocks)), which the
    /// decode has already held the tile to ([`check_tile`]).
    pub(crate) fn within(self, cap: crate::samples::Cap) -> Budget {
        let guard = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
        Budget {
            precincts: guard(cap.tile_precincts()),
            blocks: guard(cap.tile_blocks()),
            ..self
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
        ..Budget::decode(meter.left(), poc_bitmap).within(meter.cap())
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
    let geoms = tile_geometry(header, tile.index, budget, |dropped| {
        meter.spend(
            dropped,
            WorkRefusal::DroppedBlocks {
                blocks: dropped as u64,
            },
        )
    })?;
    if let Some(why @ WorkRefusal::EmptyComponents { steps, .. }) =
        empty_components(header, tile.index)
    {
        meter.spend(usize::try_from(steps).unwrap_or(usize::MAX), why)?;
    }
    let mut states = band_states(header, &geoms);

    let walk = PacketWalk {
        siz: &header.siz,
        tile: header
            .siz
            .tile_rect(tile.index)
            .ok_or_else(|| Error::inconsistent(format!("no tile {} in `SIZ`", tile.index)))?,
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
            mark_gap(&mut states[comp][res], u32::from(layer));
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
        budget.poc_counts,
        |layer, resolution, component, precinct| {
            if let Some(reach) = reach.as_deref_mut() {
                if ended {
                    // Past the end of the data: count the packet, not read.
                    meter.spend(1, WorkRefusal::PacketWalk)?;
                    reach.miss(component, resolution, layer);
                    mark_unread(&mut states[component][resolution], precinct, layer);
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
                None,
            )?;
            if packet != Packet::Read {
                ended = true;
                mark_unread(&mut states[component][resolution], precinct, layer);
                meter.spend(end_visits, WorkRefusal::PacketWalk)?;
                if let Some(reach) = reach.as_deref_mut() {
                    reach.miss(component, resolution, layer);
                }
            }
            packet_index += 1;
            Ok(())
        },
    )?;
    // The scratch holds the contributions of the largest packet read, an
    // entry for each of its segments with bytes. Let go of it before the
    // segments Tier-1 reads are built beside the walk's.
    drop(scratch);

    let mut components = Vec::with_capacity(states.len());
    for (comp, (component_states, component_geoms)) in states.iter().zip(&geoms).enumerate() {
        let rect = header
            .siz
            .tile_component_rect(tile.index, comp)
            .ok_or_else(|| Error::inconsistent(format!("no component {comp} in `SIZ`")))?;
        let mut resolutions = Vec::with_capacity(component_states.len());
        for (band_states, level) in component_states.iter().zip(component_geoms) {
            let subbands = if level.bands.is_empty() {
                Vec::new()
            } else {
                build_subbands(&level.bands, band_states, data)
            };
            resolutions.push(Resolution { subbands });
        }
        components.push(ComponentCoded {
            resolutions,
            rect: Some(rect),
        });
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
        return Err(Error::codestream(format!(
            "tile {} has {} byte(s) left after the last packet",
            tile.index,
            data.len() - streams.body_pos
        )));
    }
    // The packed headers must be exhausted too: a leftover header byte is the
    // same kind of misread, caught on the header stream instead of the body.
    if packed && streams.header_pos != streams.header.len() {
        return Err(Error::codestream(format!(
            "tile {} has {} byte(s) of packed packet headers left after the last packet",
            tile.index,
            streams.header.len() - streams.header_pos
        )));
    }

    Ok(CodedData { components })
}

/// The geometry of each component of tile `tile`, built under `budget`'s
/// caps. After each component, `dropped` is called with the code-blocks of
/// the resolutions `budget`'s reduction drops (see [`Budget::reduction`]),
/// and an error from it ends the build.
fn tile_geometry(
    header: &MainHeader,
    tile: u32,
    budget: &mut Budget,
    mut dropped: impl FnMut(usize) -> Result<()>,
) -> Result<Vec<Vec<ResolutionGeom>>> {
    let component_count = header.siz.components.len();
    let mut geoms = Vec::with_capacity(component_count);
    for c in 0..component_count {
        let resolutions =
            resolution_geoms(header, tile, c, &mut budget.precincts, &mut budget.blocks)?;
        dropped(dropped_blocks(&resolutions, budget.reduction))?;
        geoms.push(resolutions);
    }
    Ok(geoms)
}

/// The code-blocks of the resolutions `reduction` drops from one
/// component's `resolutions`: its finest `reduction` levels, or all of them
/// when it has no more.
pub(super) fn dropped_blocks(resolutions: &[ResolutionGeom], reduction: u8) -> usize {
    let kept = resolutions.len().saturating_sub(usize::from(reduction));
    resolutions[kept..]
        .iter()
        .flat_map(|resolution| &resolution.bands)
        .map(BandGeom::block_count)
        .sum()
}

/// The charge for the components of tile `tile` that hold no sample of it,
/// under `header`, the tile's header, or `None` when every component holds
/// one.
///
/// Such a tile-component has no packet, so no byte pays for its setup and
/// its decode, about 0.3 µs plus 0.1 µs for each resolution. A file of
/// 65025 one-sample tiles and 16384 components, each but the first holding
/// a sample in one tile only, is 1 MB and would take minutes. So each
/// resolution of each such component costs [`END_VISITS`], as the end of a
/// tile's data does: the default bound's 2^24 visits hold about a quarter
/// of a second of it. A tile-component that holds a sample has a packet,
/// which pays for it, so a file without an empty one is charged nothing.
pub(super) fn empty_components(header: &MainHeader, tile: u32) -> Option<WorkRefusal> {
    let (mut components, mut resolutions) = (0u64, 0u64);
    for (comp, params) in header.components.iter().enumerate() {
        if header
            .siz
            .tile_component_rect(tile, comp)
            .is_some_and(|rect| rect.is_empty())
        {
            components += 1;
            resolutions += u64::from(params.coding.decomposition_levels) + 1;
        }
    }
    (components > 0).then_some(WorkRefusal::EmptyComponents {
        components,
        steps: resolutions * END_VISITS as u64,
    })
}

/// One state per (component, resolution, band) of `geoms`, carried across
/// every layer. The tag trees decode incrementally and there is a pair per
/// *precinct*, so they must outlive the packet that starts them.
fn band_states(header: &MainHeader, geoms: &[Vec<ResolutionGeom>]) -> Vec<Vec<Vec<BandState>>> {
    geoms
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
        .collect()
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
fn next_segment(coder: Coder, style: u8, last: Option<SegmentKind>) -> (SegmentKind, u32) {
    use crate::codestream::markers::code_block_style::{LAZY, TERMALL};
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

/// Add a length field's `bytes` to the last segment of `segment`, which an HT
/// segment may hold only within its limit. An HT segment's bytes are those of
/// its codeword segments over the layers (B.3), so the limit is on the sum.
fn add_length(segment: &mut SegmentStates, bytes: u32) -> Result<()> {
    segment.length += u64::from(bytes);
    let (what, max) = match segment.last {
        SegmentKind::HtCleanup { .. } => ("cleanup", HT_CLEANUP_MAX),
        SegmentKind::HtRefinement => ("refinement", HT_REFINEMENT_MAX),
        SegmentKind::Mq | SegmentKind::Raw => return Ok(()),
    };
    if segment.length > max {
        return Err(Error::codestream(format!(
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
                return Err(Error::codestream(
                    "HT code-block's first cleanup segment is 1 byte long; it must be at least 2",
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
        return Err(Error::codestream(
            "HT code-block placeholder passes carry bytes",
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
/// cannot have (a cleanup length of 1 or over `HT_CLEANUP_MAX`, or bytes on
/// placeholder passes) makes it a Part 1 block's: that is the A.4 note's "processing the code-block
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
        // A cleanup segment over its limit (7.1.1) is not an HT block's.
        top if u64::from(top) > HT_CLEANUP_MAX => Ok(part1(top, bits, bio)),
        length => Ok(MixedFirst::Ht(FirstCleanup::Cleanup {
            placeholders,
            passes: placeholders + 1 - prior,
            length,
        })),
    }
}

/// Settle a code-block of a MIXED tile-component as a Part 1 block. Its
/// earlier passes, held as placeholder passes of no bytes, become passes of
/// its one MQ codeword segment. A.4 allows a Part 1 block there neither
/// bypass nor termination on each pass, so all its passes, in every layer,
/// are one codeword segment with one length field per contribution
/// (ISO/IEC 15444-1 B.10.7.1). The segment has no pass limit: OpenJPEG's
/// 109-pass split ([`next_segment`]) is for the Part 1 tile-components it
/// decodes. A block of more passes than tier-1 can decode is refused there.
fn become_part1(block: &mut BlockState) {
    block.coder = Some(Coder::Part1);
    match block.segments.as_deref_mut() {
        Some(segments) => {
            // Until now the block's one segment was an open HT cleanup
            // segment, so it is the first and the last.
            debug_assert_eq!(segments.passes.len(), 1);
            segments.first = SegmentKind::Mq;
            segments.last = SegmentKind::Mq;
            segments.max_passes = u32::MAX;
            block.num_passes += segments.kept_of(0);
        }
        None => block.segments = Some(SegmentStates::new(SegmentKind::Mq, u32::MAX)),
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
    /// A tile holds no sample of `components` components, whose setup costs
    /// `steps`, more than the visits left (see [`empty_components`]).
    EmptyComponents { components: u64, steps: u64 },
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
            WorkRefusal::EmptyComponents { components, steps } => format!(
                "the tile holds no sample of {components} components, whose setup costs \
                 {steps} steps, more than the steps left of the packet work bound (see \
                 `DecodeOptions::packet_work_limit`)"
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
            Refused::Work(work) => Error::limit_unhinted(crate::LimitKind::PacketWork, work.why()),
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
///
/// It also carries the decode's sample limit, which sets the precincts and
/// code-blocks each tile's walk may build ([`Budget::within`]).
pub(crate) struct Meter {
    first: u64,
    left: Cell<u64>,
    refusal: Cell<Option<WorkRefusal>>,
    cap: crate::samples::Cap,
}

impl Meter {
    /// A meter with `first` visits to spend, under the floor of the default
    /// sample limit.
    pub(crate) fn new(first: u64) -> Self {
        Meter {
            first,
            left: Cell::new(first),
            refusal: Cell::new(None),
            cap: crate::samples::cap(&crate::SampleLimit::Default, 0),
        }
    }

    /// This meter under the sample limit `cap`.
    pub(crate) fn under(self, cap: crate::samples::Cap) -> Self {
        Meter { cap, ..self }
    }

    /// The sample limit the walks build their geometry under.
    pub(crate) fn cap(&self) -> crate::samples::Cap {
        self.cap
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
        Error::limit_unhinted(crate::LimitKind::PacketWork, why.why())
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
    Error::codestream("a packet header runs past the end of its `PPM` or `PPT` packed headers")
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
        return Err(Error::codestream("truncated `SOP` marker segment"));
    }
    let lsop = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
    if lsop != 4 {
        return Err(Error::marker(format!(
            "`SOP` declares `Lsop` {lsop}, expected 4"
        )));
    }
    let nsop = u16::from_be_bytes([data[pos + 4], data[pos + 5]]);
    // Nsop is 16 bits and wraps; a tile with more than 65536 packets restarts it.
    let expected = (packet_index % 65536) as u16;
    if nsop != expected {
        return Err(Error::codestream(format!(
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
struct BlockState {
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
    /// The block's codeword segments, `None` until it has one. A block
    /// state that holds none costs no more than the pointer.
    segments: Option<Box<SegmentStates>>,
}

/// A code-block's codeword segments while they are filled from successive
/// layers. The last one may still take passes; every one before it holds all
/// it may (see [`max_passes`](Self::max_passes)), since a new segment opens
/// only once the last is full.
///
/// A packet header can open a segment for 3 bits, so what a segment costs
/// on its own is kept to its pass count: the bytes go in one list for the
/// block, and what only the first or the last segment needs is held once.
struct SegmentStates {
    /// Each segment's coding passes from every layer read, which decide
    /// where the next segment starts. Never empty. An HT block's first
    /// cleanup segment counts its placeholder passes.
    passes: Vec<u32>,
    /// Where each layer's bytes of a segment lie in the tile data, in
    /// segment order and, within a segment, in layer order: a layer adds
    /// bytes to the last segment or to new ones. A length of zero files none.
    chunks: Vec<Chunk>,
    /// The passes from the layers the decode keeps, over all segments: all
    /// of them without a layer cap. The layers past the cap only add passes
    /// after these, and add no chunks, so a segment's kept passes are what
    /// of these falls in it (see [`kept_of`](Self::kept_of)).
    kept: u32,
    /// The first segment's kind, which says how many placeholder passes an
    /// HT block has.
    first: SegmentKind,
    /// The last segment's kind.
    last: SegmentKind,
    /// How many passes the last segment may hold before the next one starts:
    /// 1 under `termall`, the `10, 2, 1, 2, 1, …` pattern under `bypass`,
    /// otherwise 109 — the most a code-block can carry (`3 * Mb - 2` with
    /// `Mb <= 37`), so the default never splits. See [`next_segment`],
    /// which mirrors OpenJPEG's `opj_t2_init_seg`. A Part 1 block of a MIXED
    /// tile-component has one segment of any number of passes
    /// ([`become_part1`]). An HT block's first
    /// cleanup segment holds its placeholder passes and the cleanup pass,
    /// and until the cleanup pass comes it takes any number.
    max_passes: u32,
    /// The bytes the length fields gave the last segment, over every layer
    /// read.
    length: u64,
}

/// One layer's bytes of one codeword segment. They are offsets, not
/// slices, so that the state can outlive the bytes it was read from and go
/// on over a longer copy of them.
#[derive(Clone, Copy)]
struct Chunk {
    start: usize,
    len: u32,
    /// The segment's index in the block.
    segment: u32,
}

impl SegmentStates {
    /// A block's first segment, of no passes yet.
    fn new(kind: SegmentKind, max_passes: u32) -> Box<Self> {
        Box::new(SegmentStates {
            passes: vec![0],
            chunks: Vec::new(),
            kept: 0,
            first: kind,
            last: kind,
            max_passes,
            length: 0,
        })
    }

    /// Open a segment of no passes after the last.
    fn open(&mut self, kind: SegmentKind, max_passes: u32) {
        self.passes.push(0);
        self.last = kind;
        self.max_passes = max_passes;
        self.length = 0;
    }

    /// Whether the last segment holds all the passes it may.
    fn last_full(&self) -> bool {
        self.passes.last().is_some_and(|&p| p >= self.max_passes)
    }

    /// Add `passes` to the last segment, kept or not.
    fn add_passes(&mut self, passes: u32, keep: bool) {
        if let Some(last) = self.passes.last_mut() {
            *last += passes;
        }
        if keep {
            self.kept += passes;
        }
    }

    /// The kept passes of segment `index`, which starts `start` passes into
    /// the block: what of [`kept`](Self::kept) falls in it.
    fn kept_in(&self, index: usize, start: u32) -> u32 {
        self.kept
            .saturating_sub(start)
            .min(self.passes.get(index).copied().unwrap_or(0))
    }

    /// The kept passes of segment `index`.
    fn kept_of(&self, index: usize) -> u32 {
        let start = self.passes[..index].iter().sum();
        self.kept_in(index, start)
    }

    /// The placeholder passes of the first segment's kept passes: all of
    /// them while the cleanup pass is still to come. No other segment holds
    /// any.
    fn kept_placeholders(&self) -> u32 {
        let kept = self.kept_of(0);
        match self.first {
            SegmentKind::HtCleanup {
                placeholders: Some(p),
            } => kept.min(p),
            SegmentKind::HtCleanup { placeholders: None } => kept,
            _ => 0,
        }
    }

    /// The kept passes Tier-1 decodes of each segment in turn: its kept
    /// passes without the placeholder passes, which code nothing.
    fn coded_kept(&self) -> impl Iterator<Item = u32> + '_ {
        let placeholders = self.kept_placeholders();
        let mut start = 0u32;
        self.passes.iter().enumerate().map(move |(index, &passes)| {
            let kept = self.kept_in(index, start);
            start = start.saturating_add(passes);
            if index == 0 {
                kept - placeholders
            } else {
                kept
            }
        })
    }

    /// The segments Tier-1 decodes, over the tile data `data`, and whether
    /// the last of them may be cut short (see [`CodeBlock::cut`]).
    ///
    /// Under a layer cap, the segments the kept layers never reached are
    /// left out, and the last one kept may hold only some of its passes.
    /// Without a cap every pass is kept. Stopping at the first segment of no
    /// coded kept pass is what keeps a cut segment last: Tier-1 exempts only
    /// the last segment from `predictable termination`, so an empty segment
    /// left after the cut one would have it checked and refused.
    ///
    /// With `unread`, a later packet of the block's precinct was not read,
    /// so the block may have lost later passes of its last segment, unless
    /// that segment holds all the passes it can: then the next pass opens a
    /// new segment, and this one is whole. Every segment before the last is
    /// whole.
    fn coded<'a>(&self, data: &'a [u8], unread: bool) -> (Segments<'a>, bool) {
        let count = self.coded_kept().take_while(|&p| p > 0).count();
        let Some(last) = count.checked_sub(1) else {
            return (Segments::default(), false);
        };
        let start: u32 = self.passes[..last].iter().sum();
        let cut = self.kept_in(last, start) < self.passes[last]
            || (unread && last + 1 == self.passes.len() && !self.last_full());
        // The chunks are in segment order, so those of the segments kept
        // come first.
        debug_assert!(self.chunks.windows(2).all(|w| w[0].segment <= w[1].segment));
        let kept_chunks = self
            .chunks
            .partition_point(|c| (c.segment as usize) < count);
        let chunks: Vec<&'a [u8]> = self.chunks[..kept_chunks]
            .iter()
            .map(|c| &data[c.start..c.start + c.len as usize])
            .collect();
        let mut heads = Vec::with_capacity(count);
        let mut end = 0;
        for (index, passes) in self.coded_kept().take(count).enumerate() {
            while end < kept_chunks && self.chunks[end].segment as usize == index {
                end += 1;
            }
            heads.push(SegmentHead {
                passes,
                end: index_u32(end),
            });
        }
        (Segments::from_parts(heads, chunks), cut)
    }

    /// The bytes these hold on the heap, capacity included.
    fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        size_of::<Self>()
            + self.passes.capacity() * size_of::<u32>()
            + self.chunks.capacity() * size_of::<Chunk>()
    }
}

impl BlockState {
    fn new(coder: Option<Coder>) -> Self {
        BlockState {
            coder,
            included: false,
            lblock: 3,
            zero_bit_planes: 0,
            num_passes: 0,
            segments: None,
        }
    }

    /// The magnitude bit-planes an HT block skips past those the tag tree
    /// gives: one per three placeholder passes (`P0`, ISO/IEC 15444-15 B.3),
    /// counted only when the decode keeps the cleanup pass after them. A
    /// block without its cleanup pass decodes to zeros whatever the count.
    fn skipped_planes(&self) -> u32 {
        match self.segments.as_deref() {
            Some(
                s @ SegmentStates {
                    first:
                        SegmentKind::HtCleanup {
                            placeholders: Some(p),
                        },
                    ..
                },
            ) if s.kept_of(0) > s.kept_placeholders() => p / 3,
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
struct BandState {
    precincts: Vec<PrecinctState>,
    blocks: Vec<BlockState>,
}

/// The two tag trees one precinct runs over one subband: which code-blocks the
/// packet includes, and how many of their leading bit-planes are all zero.
struct PrecinctState {
    inclusion: TagTree,
    zero_bits: TagTree,
    /// With `strict` off: the first layer of the precinct whose packet the
    /// `POC` volumes leave out. It stays missing however much data comes.
    gap: Option<u32>,
    /// With `strict` off: the first layer whose packet of the precinct was
    /// not read, or read only in part, because the data ended. More data
    /// may bring it.
    ended: Option<u32>,
}

impl PrecinctState {
    /// The first layer of the precinct whose packet was not read whole,
    /// for either reason. Its code-blocks may then lack passes a later layer
    /// adds to their last segment.
    fn unread(&self) -> Option<u32> {
        match (self.gap, self.ended) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

impl BandState {
    /// The bytes the band's state holds on the heap.
    fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        let precincts = self.precincts.capacity() * size_of::<PrecinctState>()
            + self
                .precincts
                .iter()
                .map(|p| p.inclusion.heap_bytes() + p.zero_bits.heap_bytes())
                .sum::<usize>();
        let blocks = self.blocks.capacity() * size_of::<BlockState>()
            + self
                .blocks
                .iter()
                .filter_map(|b| b.segments.as_deref())
                .map(SegmentStates::heap_bytes)
                .sum::<usize>();
        precincts + blocks
    }

    /// Free each precinct's tag trees, once no packet is left to read
    /// them, and forget where the data ended.
    fn drop_trees(&mut self) {
        for precinct in &mut self.precincts {
            precinct.inclusion = TagTree::new(1, 1);
            precinct.zero_bits = TagTree::new(1, 1);
            precinct.ended = None;
        }
        self.precincts.shrink_to_fit();
    }

    /// `coder` is the tile-component's coder, `None` under MIXED.
    fn new(band: &BandGeom, coder: Option<Coder>) -> Self {
        BandState {
            precincts: band
                .precincts
                .iter()
                .map(|precinct| PrecinctState {
                    inclusion: TagTree::new(precinct.cols as u32, precinct.rows as u32),
                    zero_bits: TagTree::new(precinct.cols as u32, precinct.rows as u32),
                    gap: None,
                    ended: None,
                })
                .collect(),
            blocks: (0..band.block_count())
                .map(|_| BlockState::new(coder))
                .collect(),
        }
    }
}

/// The tag-tree nodes a packet's reads changed, as they were before, per
/// band of its resolution: the inclusion tree's and the zero bit-plane
/// tree's. With the block log (see [`Undo`]) and the stream positions, it
/// undoes a packet whole, so that a walk can stop inside a packet the data
/// cuts and read it again once more data comes.
#[derive(Debug, Default)]
pub(crate) struct Journal {
    trees: Vec<[Vec<Saved>; 2]>,
}

impl Journal {
    /// The bytes the notes hold on the heap, capacity included.
    fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        self.trees.capacity() * size_of::<[Vec<Saved>; 2]>()
            + self
                .trees
                .iter()
                .flatten()
                .map(|notes| notes.capacity() * size_of::<Saved>())
                .sum::<usize>()
    }

    /// Forget what the last packet noted.
    fn clear(&mut self) {
        for [inclusion, zero_bits] in &mut self.trees {
            inclusion.clear();
            zero_bits.clear();
        }
    }

    /// The two trees' notes for band `band`.
    fn band(&mut self, band: usize) -> &mut [Vec<Saved>; 2] {
        if self.trees.len() <= band {
            self.trees.resize_with(band + 1, Default::default);
        }
        &mut self.trees[band]
    }

    /// Put back the trees of `precinct` in `states` as they were before the
    /// packet that noted them.
    fn restore(&self, states: &mut [BandState], precinct: usize) {
        for (state, [inclusion, zero_bits]) in states.iter_mut().zip(&self.trees) {
            if let Some(trees) = state.precincts.get_mut(precinct) {
                trees.zero_bits.restore(zero_bits);
                trees.inclusion.restore(inclusion);
            }
        }
    }
}

/// What one codeword segment takes from *this* packet: the length of the byte
/// range that follows the header, and where to file it. A packet holds one
/// for each segment it gives bytes, up to one a byte of its body, so the
/// fields are kept narrow.
struct Contribution {
    block: usize,
    /// The band's index in its resolution, at most 2.
    band: u32,
    segment: u32,
    seg_len: u32,
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

impl Scratch {
    /// The bytes the buffers hold on the heap, capacity included.
    fn heap_bytes(&self) -> usize {
        self.contributions.capacity() * std::mem::size_of::<Contribution>()
            + self.unread.heap_bytes()
    }
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
    /// The block's segments before the packet, `None` when it had none.
    segments: Option<LastSegment>,
}

/// A block's segments before a packet, which the packet may add segments,
/// passes and bytes to, or, for an HT block's first cleanup segment, end.
struct LastSegment {
    /// How many segments and chunks the block had.
    segments: usize,
    chunks: usize,
    /// The last segment's passes, and what [`SegmentStates`] holds once.
    passes: u32,
    kept: u32,
    first: SegmentKind,
    last: SegmentKind,
    max_passes: u32,
    length: u64,
}

impl LastSegment {
    fn of(segments: &SegmentStates) -> Self {
        LastSegment {
            segments: segments.passes.len(),
            chunks: segments.chunks.len(),
            passes: segments.passes.last().copied().unwrap_or(0),
            kept: segments.kept,
            first: segments.first,
            last: segments.last,
            max_passes: segments.max_passes,
            length: segments.length,
        }
    }

    /// Put `segments` back as they were.
    fn restore(&self, segments: &mut SegmentStates) {
        segments.passes.truncate(self.segments);
        if let Some(last) = segments.passes.last_mut() {
            *last = self.passes;
        }
        segments.chunks.truncate(self.chunks);
        segments.kept = self.kept;
        segments.first = self.first;
        segments.last = self.last;
        segments.max_passes = self.max_passes;
        segments.length = self.length;
    }
}

/// Put back the state of every block in `log`, latest first, so that the
/// packet that logged them adds nothing to those blocks. Their earlier
/// layers stay.
fn undo(log: &[Undo], states: &mut [BandState]) {
    for entry in log.iter().rev() {
        let block = &mut states[entry.band].blocks[entry.block];
        block.coder = entry.coder;
        block.included = entry.included;
        block.lblock = entry.lblock;
        block.zero_bit_planes = entry.zero_bit_planes;
        block.num_passes = entry.num_passes;
        match &entry.segments {
            // A packet never takes a block's segments away, so they are there.
            Some(was) => {
                if let Some(segments) = block.segments.as_deref_mut() {
                    was.restore(segments);
                }
            }
            None => block.segments = None,
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
fn parse_packet(
    streams: &mut PacketStreams<'_>,
    meter: &Meter,
    layer: u32,
    packet_index: u32,
    delimiters: Delimiters,
    style: u8,
    bands: &[BandGeom],
    precinct: usize,
    states: &mut [BandState],
    scratch: &mut Scratch,
    keep: bool,
    mut log: Option<&mut Vec<Undo>>,
    mut journal: Option<&mut Journal>,
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
        return Err(Error::codestream(format!(
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
                journal.as_deref_mut().map(|j| j.band(band_index)),
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
            return Err(Error::codestream(
                "`COD` signals `EPH` but the packet header is not followed by one",
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
            .checked_add(contribution.seg_len as usize)
            .filter(|&e| e <= streams.data.len())
        {
            Some(end) => end,
            None => {
                if let Some(log) = log.as_deref() {
                    // Keep the blocks before this one; undo this one and every
                    // block after it in the header.
                    let from = log
                        .iter()
                        .position(|u| {
                            (u.band, u.block) == (contribution.band as usize, contribution.block)
                        })
                        .unwrap_or(0);
                    undo(&log[from..], states);
                    return Ok(Packet::Ended);
                }
                return Err(Error::codestream(format!(
                    "a packet body segment overruns the data of tile {}",
                    streams.tile
                )));
            }
        };
        if keep {
            // A contribution is filed only after the header gave its block
            // a segment for it.
            if let Some(segments) = states[contribution.band as usize].blocks[contribution.block]
                .segments
                .as_deref_mut()
            {
                segments.chunks.push(Chunk {
                    start: streams.body_pos,
                    len: contribution.seg_len,
                    segment: contribution.segment,
                });
            }
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
/// width, but they are not counted as kept (see [`SegmentStates::kept`]).
#[allow(clippy::too_many_arguments)]
fn parse_band_header(
    meter: &Meter,
    band_index: usize,
    precinct: &PrecinctGeom,
    trees: &mut PrecinctState,
    blocks: &mut [BlockState],
    layer: u32,
    style: u8,
    keep: bool,
    bio: &mut BitReader,
    contributions: &mut Vec<Contribution>,
    unread: &mut UnreadSpans,
    mut log: Option<&mut Vec<Undo>>,
    mut saved: Option<&mut [Vec<Saved>; 2]>,
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
            let saved = saved.as_deref_mut().map(|[inclusion, _]| inclusion);
            match trees.inclusion.read_saving(bx, by, threshold, bio, saved) {
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
                segments: block.segments.as_deref().map(LastSegment::of),
            });
        }

        // The zero-bitplane tree is resolved in full the first time a block is
        // included, so a single read at the ceiling settles it. Later layers
        // never read it again.
        if !was_included {
            let saved = saved.as_deref_mut().map(|[_, zero_bits]| zero_bits);
            blocks[block_index].zero_bit_planes = trees
                .zero_bits
                .read_saving(bx, by, ZBP_LIMIT, bio, saved)
                .ok_or_else(|| {
                    Error::codestream(format!(
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
                return Err(Error::codestream(format!(
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
        let first_open = block.segments.as_ref().is_none_or(|s| s.first == open);
        let prior = block.segments.as_ref().map_or(0, |s| s.passes[0]);
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
                    become_part1(block);
                    part1_length = Some(length);
                    None
                }
            },
        };
        if let Some(first) = first {
            // The first segment is open, so it is the block's only one.
            let segment = block
                .segments
                .get_or_insert_with(|| SegmentStates::new(open, u32::MAX));
            debug_assert_eq!(segment.passes.len(), 1);
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
                    let kind = SegmentKind::HtCleanup {
                        placeholders: Some(before),
                    };
                    segment.first = kind;
                    segment.last = kind;
                    segment.max_passes = before + 1;
                    add_length(segment, length)?;
                    contributions.push(Contribution {
                        block: block_index,
                        band: band_index as u32,
                        segment: 0,
                        seg_len: length,
                    });
                    passes
                }
            };
            segment.add_passes(passes, keep);
            remaining -= passes;
        }
        if keep {
            block.num_passes += num_passes - placeholders;
        }
        while remaining > 0 {
            let coder = block.coder.unwrap_or(Coder::Ht);
            let segment: &mut SegmentStates = match &mut block.segments {
                Some(segments) => {
                    if segments.last_full() {
                        let (kind, max_passes) = next_segment(coder, style, Some(segments.last));
                        segments.open(kind, max_passes);
                    }
                    segments
                }
                none => {
                    let (kind, max_passes) = next_segment(coder, style, None);
                    none.insert(SegmentStates::new(kind, max_passes))
                }
            };
            let segment_index = segment.passes.len() - 1;
            let last_passes = segment.passes[segment_index];
            let new_passes = remaining.min(segment.max_passes - last_passes);

            // The length field's width uses *this* segment's new passes.
            let seg_len = match part1_length.take() {
                Some(length) => length,
                None => bio.read(length_bits(block.lblock, new_passes)),
            };
            add_length(segment, seg_len)?;

            // A layer may add coding passes and no bytes (B.10.7 sets no
            // lower bound on the length). The MQ codeword is continuous
            // within a segment, so the encoder's rate split can put passes in
            // one layer and the bytes carrying them in the next. A segment
            // that never gets a byte is decoded from the `0xFF` fill (D.4.1),
            // as OpenJPEG decodes it. The passes still count and still fill
            // the segment, so a later layer's passes land in the right one.
            // A zero length files no chunk, and reading it took at least the
            // header bits above, so it costs no more than one with bytes.
            segment.add_passes(new_passes, keep);
            if seg_len > 0 {
                contributions.push(Contribution {
                    block: block_index,
                    band: band_index as u32,
                    segment: index_u32(segment_index),
                    seg_len,
                });
            }

            remaining -= new_passes;
        }
    }
    Ok(())
}

/// Note that the packet of `precinct` in `layer` was not read whole because
/// the data ended (see [`PrecinctState::ended`]). A packet spans every band
/// of the resolution.
fn mark_unread(bands: &mut [BandState], precinct: usize, layer: u32) {
    for band in bands {
        if let Some(state) = band.precincts.get_mut(precinct) {
            state.ended = Some(state.ended.map_or(layer, |from| from.min(layer)));
        }
    }
}

/// Note that the `POC` volumes leave out the packets of every precinct of
/// a resolution from `layer` on (see [`PrecinctState::gap`]).
fn mark_gap(bands: &mut [BandState], layer: u32) {
    for band in bands {
        for state in &mut band.precincts {
            state.gap = Some(state.gap.map_or(layer, |from| from.min(layer)));
        }
    }
}

/// Turn the accumulated per-layer state of one resolution into its subbands.
///
/// A block may end up with coding passes and no bytes, in one codeword segment
/// or in all of them, since B.10.7 lets a contribution's length be zero. Tier-1
/// decodes such a segment from the `0xFF` bytes D.4.1 appends, as OpenJPEG
/// does.
///
/// `data` is the tile data the segments' chunks are offsets into. The state
/// is left as it was, so a walk can go on from it.
fn build_subbands<'a>(
    bands: &[BandGeom],
    states: &[BandState],
    data: &'a [u8],
) -> Vec<Subband<'a>> {
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
                if let Some(from) = precinct.unread() {
                    unread.resize(state.blocks.len(), None);
                    for by in 0..geom.rows {
                        for bx in 0..geom.cols {
                            if let Some(flag) = unread.get_mut(geom.block(bx, by)) {
                                *flag = Some(from);
                            }
                        }
                    }
                }
            }
            let mut blocks = Vec::with_capacity(state.blocks.len());
            blocks.extend(band.block_rects().zip(&state.blocks).enumerate().map(
                |(index, ((x, y, width, height), block))| {
                    let missing_from = unread.get(index).copied().flatten();
                    let unread = missing_from.is_some();
                    // An HT block's placeholder passes code nothing, so they
                    // count as neither passes nor a segment (see
                    // [`SegmentStates::coded`]); they skip bit-planes
                    // instead.
                    let coder = block.coder.unwrap_or(Coder::Ht);
                    let zero_bit_planes = block.zero_bit_planes + block.skipped_planes();
                    let placeholders = block
                        .segments
                        .as_deref()
                        .map_or(0, SegmentStates::kept_placeholders);
                    let (segments, cut) = block
                        .segments
                        .as_deref()
                        .map_or((Segments::default(), false), |s| s.coded(data, unread));
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
                        missing_from,
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
