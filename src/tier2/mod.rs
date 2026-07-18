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
//! `decode_packets` also computes the resolution / subband / code-block geometry
//! from the [`MainHeader`](crate::codestream::MainHeader), so it is the single
//! source of truth the assembly stage reuses.

pub mod bio;
mod geometry;
mod progression;
pub mod tagtree;

use crate::codestream::markers::marker;
use crate::codestream::{MainHeader, Tile};
use crate::{Error, Result};
use bio::BitReader;
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
    //
    // The code-block count is bounded the same way and for the same reason: each
    // band's `blocks` vector plus its per-block `BlockState`/tag trees dwarf the
    // geometry tuple, and legal 4×4 blocks push the count toward samples/16, so
    // the cap has to bite before those vectors fill rather than summing them
    // afterwards. Both running totals are threaded through `resolution_geoms`.
    let mut precinct_budget = MAX_PRECINCTS;
    let mut block_budget = MAX_CODE_BLOCKS;
    let component_count = header.siz.components.len();
    let geoms = (0..component_count)
        .map(|c| {
            resolution_geoms(
                header,
                tile.index,
                c,
                &mut precinct_budget,
                &mut block_budget,
            )
        })
        .collect::<Result<Vec<_>>>()?;

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
    };
    let mut packet_index = 0u32;
    for_each_packet(header, &walk, |layer, resolution, component, precinct| {
        let geom = &geoms[component][resolution];
        parse_packet(
            &mut streams,
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

    // The packets tile the tile-part data exactly, with no padding, up to the
    // closing EOC. Any remainder means a misread field — a dropped layer shows up
    // here as leftover bytes (this doubles as the parse self-check).
    if streams.body_pos != data.len() {
        return Err(Error::Codestream(format!(
            "tile-part has {} body byte(s) left after the last packet",
            data.len() - streams.body_pos
        )));
    }
    // The packed headers must be exhausted too: a leftover header byte is the
    // same kind of misread, caught on the header stream instead of the body.
    if packed && streams.header_pos != streams.header.len() {
        return Err(Error::Codestream(format!(
            "PPT packed headers have {} byte(s) left after the last packet",
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

/// How many coding passes the *next* codeword segment may hold, given the
/// code-block style and the segments already opened (`opj_t2_init_seg`).
///
/// `termall` terminates the MQ coder after every pass, so each pass is its own
/// segment. `bypass` (lazy) terminates at every raw/MQ boundary: the first
/// segment holds the ten MQ passes of the four most significant bit-planes, then
/// each lower plane splits into a two-pass raw segment (significance +
/// refinement) and a one-pass MQ segment (cleanup) — the `10, 2, 1, 2, 1, …`
/// pattern OpenJPEG spells with `prev.maxpasses in {1, 10} ? 2 : 1`. Otherwise a
/// segment takes up to 109 passes — `3 * Mb - 2` with `Mb` capped at 37 — which a
/// code-block can never exceed, so the default never splits.
fn segment_max_passes(style: u8, segments: &[SegmentState<'_>]) -> u32 {
    use crate::codestream::markers::code_block_style::{LAZY, TERMALL};
    if style & TERMALL != 0 {
        1
    } else if style & LAZY != 0 {
        match segments.last() {
            None => 10,
            Some(prev) => {
                if prev.max_passes == 1 || prev.max_passes == 10 {
                    2
                } else {
                    1
                }
            }
        }
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
    /// 1 under `termall`, the `10, 2, 1, 2, 1, …` pattern under `bypass`,
    /// otherwise 109 — the most a code-block can carry (`3 * Mb - 2` with
    /// `Mb <= 37`), so the default never splits. See [`segment_max_passes`],
    /// which mirrors OpenJPEG's `opj_t2_init_seg`.
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
    streams: &mut PacketStreams<'a>,
    layer: u32,
    packet_index: u32,
    delimiters: Delimiters,
    style: u8,
    bands: &[BandGeom],
    precinct: usize,
    states: &mut [BandState<'a>],
) -> Result<()> {
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
        return Err(Error::Codestream(
            "tile-part ends before the last packet".into(),
        ));
    }

    let mut bio = BitReader::new(&streams.header[streams.header_pos..]);

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
    // between the header and the body — after an empty packet's header too — and
    // moves into the packed buffer with the headers, so it is read from the
    // header stream.
    bio.align();
    streams.header_pos += bio.bytes_consumed();
    if delimiters.eph {
        if peek_marker(streams.header, streams.header_pos) != Some(marker::EPH) {
            return Err(Error::Codestream(
                "COD signals EPH but the packet header is not followed by one".into(),
            ));
        }
        streams.header_pos += 2;
    }

    // Inline, the body follows the header in the same buffer.
    if !streams.packed {
        streams.body_pos = streams.header_pos;
    }
    for contribution in &contributions {
        let end = streams
            .body_pos
            .checked_add(contribution.seg_len)
            .filter(|&e| e <= streams.data.len())
            .ok_or_else(|| {
                Error::Codestream("packet body segment overruns the tile-part".into())
            })?;
        states[contribution.band].blocks[contribution.block].segments[contribution.segment]
            .chunks
            .push(&streams.data[streams.body_pos..end]);
        streams.body_pos = end;
    }
    // Inline, keep the two cursors together for the next packet.
    if !streams.packed {
        streams.header_pos = streams.body_pos;
    }

    Ok(())
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
        // (`termall`, `bypass`); there OpenJPEG's `do { ... } while (n > 0)`
        // reads a length per segment, which the loop below mirrors.
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
                let max_passes = segment_max_passes(style, &block.segments);
                block.segments.push(SegmentState {
                    passes: 0,
                    max_passes,
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
