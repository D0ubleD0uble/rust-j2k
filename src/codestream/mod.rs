//! Stage 1 — codestream parsing (ISO/IEC 15444-1 Annex A).
//!
//! Walks the marker segments of a raw J2K codestream: the main header
//! (SIZ / COD / QCD, plus optional COC / QCC / RGN / TLM / PLM / COM), then
//! the tile-parts (SOT, optional RGN / PLT / COM, SOD, data). Produces a
//! [`MainHeader`] of decode parameters and the byte ranges of each tile's
//! packet data — everything the later stages need, with no interpretation of
//! the entropy-coded bytes yet.
//!
//! The main header is located before it is judged: [`walk_main_header`] finds
//! every marker segment without caring what the decoder supports, and
//! [`parse_main_header`] then interprets them. That split is what lets a
//! codestream be traversed past a feature we cannot decode, and is what makes
//! the reserved segment-less markers and unknown segments handleable at all.

pub mod markers;

use std::borrow::Cow;

use crate::{Error, Result};
use markers::{
    Cod, Coding, ComponentParams, Progression, Qcd, QuantStyle, Siz, SizComponent, TlmEntry,
    Transform, marker,
};

/// Parsed main-header decode parameters. COD/QCD carry the codestream-wide
/// defaults; [`components`](Self::components) carries each component's effective
/// parameters after any COC/QCC override and any RGN maxshift — the main
/// header's, replaced by the tile-part header's where one appears (A.6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHeader {
    pub siz: Siz,
    pub cod: Cod,
    pub qcd: Qcd,
    /// Effective coding and quantization parameters per component, in SIZ order:
    /// COD/QCD's defaults with any COC/QCC override already applied. Every stage
    /// past the header reads these rather than [`cod`](Self::cod) or
    /// [`qcd`](Self::qcd), which hold the codestream-wide defaults.
    pub components: Vec<ComponentParams>,
    /// Tile-part lengths from any TLM markers, in marker order; empty when the
    /// codestream carries none. Informational (A.7.1): decoding does not use
    /// them, so they are recorded for callers to check, not consulted.
    pub tlm: Vec<TlmEntry>,
}

impl MainHeader {
    /// A header whose components all take the COD/QCD defaults — the shape of
    /// every codestream that carries no COC and no QCC.
    pub fn new(siz: Siz, cod: Cod, qcd: Qcd) -> Self {
        let components = vec![
            ComponentParams {
                coding: cod.coding(),
                quant: qcd.clone(),
                roi_shift: 0,
            };
            siz.components.len()
        ];
        MainHeader {
            siz,
            cod,
            qcd,
            components,
            tlm: Vec::new(),
        }
    }
}

/// One tile of the image: its index in the tile grid, the decode parameters in
/// force inside it, and its packet data.
///
/// A tile can be split across several tile-parts, interleaved with other tiles'
/// (A.4.2). They are gathered here: `data` is every one of this tile's
/// tile-parts' packet bytes, concatenated in `TPsot` order. Concatenation is
/// exact, not an approximation — a tile-part holds a whole number of packets
/// (B.9), so no packet straddles the seam — and it is what OpenJPEG does
/// (`opj_j2k_read_sod` appends each tile-part into the tile's buffer). It stays
/// borrowed for the overwhelmingly common single-part tile, and only copies when
/// there is more than one part to join.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile<'a> {
    pub index: u32,
    /// This tile's own tile-part-header markers, as parsed. Resolve them against
    /// the main header with [`Codestream::tile_header`] to get the parameters the
    /// tile decodes under.
    ///
    /// They are *not* resolved into a whole header here, because a header is
    /// sized by the component count and there can be tens of thousands of tiles:
    /// materializing one per tile would cost `tiles × components`, which a header
    /// under 50 KB can drive into the gigabytes. Resolved one tile at a time, the
    /// cost is `tiles + components`.
    pub header: TileHeader,
    pub data: Cow<'a, [u8]>,
}

/// A parsed codestream: the main header plus every tile, in tile-index order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codestream<'a> {
    pub header: MainHeader,
    pub tiles: Vec<Tile<'a>>,
}

impl Codestream<'_> {
    /// The parameters tile `index` decodes under: the main header with that
    /// tile's tile-part-header markers resolved over it (A.6.1).
    ///
    /// Every stage past the codestream takes one of these, so nothing downstream
    /// has to know that a tile header exists — a tile is just a header, a
    /// rectangle, and some bytes.
    pub fn tile_header(&self, index: usize) -> Result<MainHeader> {
        let tile = self
            .tiles
            .get(index)
            .ok_or_else(|| Error::Inconsistent(format!("no tile {index}")))?;
        resolve_tile_header(&self.header, &tile.header)
    }
}

/// The JP2 signature box that opens every JP2 file (ISO/IEC 15444-1 Annex I.5.1):
/// a 12-byte box of type `jP  ` whose contents are the fixed `0D 0A 87 0A`.
const JP2_SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// Parse a raw codestream (must start with SOC, end with EOC).
///
/// Rejects the JP2 box wrapper (callers pass the bare codestream) and anything
/// outside the single-component subset with [`Error::Unsupported`].
///
/// [`Error::Unsupported`]: crate::Error::Unsupported
pub fn parse(bytes: &[u8]) -> Result<Codestream<'_>> {
    // A JP2 file is valid JPEG 2000, just not the bare codestream this decoder
    // reads, so say which it is rather than complain about a missing SOC.
    if bytes.starts_with(&JP2_SIGNATURE) {
        return Err(Error::Unsupported(
            "JP2 file format wrapper; pass the contained codestream (the `jp2c` box contents)"
                .into(),
        ));
    }

    let (header, sot_offset) = parse_main_header(bytes)?;
    let tiles = walk_tiles(bytes, sot_offset, &header)?;
    let codestream = Codestream { header, tiles };

    // Resolve every tile's header once, here, so a malformed tile-part header is
    // a parse error rather than a surprise partway through a decode. Each one is
    // dropped again immediately: they are sized by the component count, and a
    // codestream can declare tens of thousands of tiles.
    for index in 0..codestream.tiles.len() {
        let tile = codestream.tile_header(index)?;

        // Every tile writes into one buffer per component, and that buffer's
        // arithmetic follows the component's wavelet: 5/3 reconstructs exact
        // integers, 9/7 reals. A tile-part COD may legally give a component a
        // different wavelet than the main header does, which would ask one buffer
        // to hold both. Nothing in Part 4 encodes that and no real encoder emits
        // it, so it is rejected rather than given a widened container.
        let pairs = tile.components.iter().zip(&codestream.header.components);
        for (component, (in_tile, in_main)) in pairs.enumerate() {
            if in_tile.coding.transform != in_main.coding.transform {
                return Err(Error::Unsupported(format!(
                    "tile {index} codes component {component} with the {:?} wavelet where the \
                     main header uses {:?}; tiles that disagree on a component's wavelet are \
                     not decoded",
                    in_tile.coding.transform, in_main.coding.transform,
                )));
            }
        }
    }

    Ok(codestream)
}

/// A tile-part header's own markers (A.4.2): the tile-level COD/QCD defaults and
/// the per-component COC/QCC/RGN overrides that layer over them. Only the *first*
/// tile-part of a tile may carry any of these, so one per tile is enough however
/// many parts the tile is split into.
///
/// The per-component overrides are **sparse** — only the components a marker
/// actually names. Almost every tile carries none at all, and a dense vector per
/// tile would cost `tiles × components` for nothing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TileHeader {
    cod: Option<Cod>,
    qcd: Option<Qcd>,
    coc: Vec<(usize, Coding)>,
    qcc: Vec<(usize, Qcd)>,
    rgn: Vec<(usize, u8)>,
}

/// What has been seen of one tile so far while the tile-parts are walked.
#[derive(Debug)]
struct TileParts<'a> {
    /// Each tile-part's packet data, in `TPsot` order (which is the order they
    /// must appear in, enforced below).
    parts: Vec<&'a [u8]>,
    /// `TNsot` from whichever tile-part stated it; `None` while every part so
    /// far left it 0 ("not yet known", A.4.2).
    declared_parts: Option<u8>,
    /// The tile's own header, from its first tile-part.
    header: TileHeader,
}

/// Walk every tile-part from the first `SOT` to the closing `EOC` (A.4.2,
/// A.4.4) and gather them into one [`Tile`] per tile index.
///
/// Tile-parts of different tiles may interleave freely, so this accumulates by
/// `Isot` and joins each tile's parts at the end. Within one tile they must
/// arrive in `TPsot` order, and every tile of the grid must be present: a
/// codestream that skips one describes an image it does not carry, which is a
/// truncation, not a feature to decode around.
///
/// A `Psot` overrun, an out-of-range or out-of-order `TPsot`, a `TNsot` that
/// disagrees with the parts that arrive, a missing tile, or a missing `EOC` all
/// reject with [`Error::Codestream`].
fn walk_tiles<'a>(bytes: &'a [u8], sot_offset: usize, main: &MainHeader) -> Result<Vec<Tile<'a>>> {
    let tile_count = main.siz.num_tiles() as usize;
    let mut tiles: Vec<TileParts<'a>> = (0..tile_count)
        .map(|_| TileParts {
            parts: Vec::new(),
            declared_parts: None,
            header: TileHeader::default(),
        })
        .collect();

    let mut part_offset = sot_offset;
    loop {
        let mut cur = Cursor::at(bytes, part_offset);
        // `parse_main_header` stopped on the first SOT and the loop below only
        // continues when it has just read one, so this is always present.
        if cur.u16()? != marker::SOT {
            return Err(Error::Codestream(
                "tile-part does not start with SOT".into(),
            ));
        }
        let sot = decode_sot(segment(&mut cur)?)?;

        let index = usize::from(sot.tile_index);
        let tile = tiles.get_mut(index).ok_or_else(|| {
            Error::Codestream(format!(
                "tile-part names tile {index}, but the grid holds {tile_count} tiles"
            ))
        })?;

        // A tile's parts are numbered from 0 and shall appear in order (A.4.2),
        // so TPsot is exactly how many of this tile's parts have already been
        // seen. That single equality catches a duplicate, a gap, and a
        // reordering at once.
        if usize::from(sot.tile_part_index) != tile.parts.len() {
            return Err(Error::Codestream(format!(
                "tile {index} tile-part index is {}, but {} of its tile-parts have been read",
                sot.tile_part_index,
                tile.parts.len(),
            )));
        }
        // TNsot is 0 in a tile-part that does not yet know the count; any part
        // that does state one must agree with every other that does.
        if sot.num_tile_parts != 0 {
            if sot.tile_part_index >= sot.num_tile_parts {
                return Err(Error::Codestream(format!(
                    "tile {index} tile-part {} of a declared {} tile-parts",
                    sot.tile_part_index, sot.num_tile_parts,
                )));
            }
            match tile.declared_parts {
                Some(declared) if declared != sot.num_tile_parts => {
                    return Err(Error::Codestream(format!(
                        "tile {index} declares {} tile-parts in one SOT and {declared} in another",
                        sot.num_tile_parts,
                    )));
                }
                _ => tile.declared_parts = Some(sot.num_tile_parts),
            }
        }

        let first_part = sot.tile_part_index == 0;
        let tile_header = read_tile_part_header(&mut cur, &main.siz, first_part)?;
        if first_part {
            tile.header = tile_header;
        }
        let data_start = cur.pos;

        // Psot counts from the SOT marker's first byte to the end of the
        // tile-part. Psot == 0 marks the *last* tile-part: it runs to the
        // closing EOC (A.4.2), anchored to the last two bytes of the buffer —
        // everything after SOD is the tile, which is also OpenJPEG's reading.
        // Scanning for the first FF D9 instead would be unsound: bit stuffing
        // keeps packet data and headers clear of it, but an SOP segment's Nsop
        // counter is two raw bytes and can spell FF D9 (packet 65497, or a 0xFF
        // low byte abutting 0xD9), which would silently truncate a valid tile.
        // The asymmetry with the Psot != 0 arm (which ignores bytes after its
        // EOC) is therefore deliberate: with no declared length there is nothing
        // else to anchor to.
        let data_end = if sot.psot == 0 {
            bytes
                .len()
                .checked_sub(2)
                .filter(|&end| end >= data_start && read_u16(bytes, end) == Some(marker::EOC))
                .ok_or_else(|| {
                    Error::Codestream("Psot=0 tile-part is not terminated by EOC".into())
                })?
        } else {
            let end = part_offset
                .checked_add(sot.psot as usize)
                .filter(|&end| end <= bytes.len())
                .ok_or_else(|| Error::Codestream("Psot overruns the codestream".into()))?;
            if end < data_start {
                return Err(Error::Codestream(
                    "Psot is shorter than the tile-part header".into(),
                ));
            }
            end
        };
        tiles[index].parts.push(&bytes[data_start..data_end]);

        // Another SOT continues the walk; EOC ends it. Anything else — or
        // nothing at all — means the tile-part chain does not close.
        match read_u16(bytes, data_end) {
            Some(marker::EOC) => break,
            Some(marker::SOT) => part_offset = data_end,
            Some(other) => {
                return Err(Error::Codestream(format!(
                    "expected SOT or EOC after a tile-part, found {other:#06X}"
                )));
            }
            None => return Err(Error::Codestream("missing EOC after the tile-part".into())),
        }
    }

    tiles
        .into_iter()
        .enumerate()
        .map(|(index, tile)| {
            if tile.parts.is_empty() {
                return Err(Error::Codestream(format!(
                    "the codestream carries no tile-part for tile {index}"
                )));
            }
            if let Some(declared) = tile.declared_parts {
                if usize::from(declared) != tile.parts.len() {
                    return Err(Error::Codestream(format!(
                        "tile {index} declares {declared} tile-parts but carries {}",
                        tile.parts.len()
                    )));
                }
            }
            // One part: borrow it. Several: join them, which is exact — a
            // tile-part holds a whole number of packets (B.9), so nothing
            // straddles the seam.
            let data = match <[&[u8]; 1]>::try_from(tile.parts.as_slice()) {
                Ok([only]) => Cow::Borrowed(only),
                Err(_) => Cow::Owned(tile.parts.concat()),
            };
            Ok(Tile {
                index: index as u32,
                header: tile.header,
                data,
            })
        })
        .collect()
}

/// Read one tile-part header: the markers between its `SOT` segment and its
/// `SOD` (A.4.2). The cursor is left on the first byte of the packet data.
///
/// Only the *first* tile-part of a tile may carry the coding-parameter overrides
/// (COD/COC/QCD/QCC/RGN): they say how to read the tile's packets, and a
/// later part cannot restate what the earlier parts were already decoded under.
/// A conformant encoder never emits one; a codestream that does is malformed,
/// not merely unsupported.
fn read_tile_part_header(cur: &mut Cursor<'_>, siz: &Siz, first_part: bool) -> Result<TileHeader> {
    let mut tile = TileHeader::default();
    // A.6 allows one of each per header. The tile-part tally is tracked apart
    // from the main header's: a tile-part marker over a main-header one for the
    // same component is the legal override, not a duplicate.
    let overrides_here = |code: u16| -> Result<()> {
        if first_part {
            Ok(())
        } else {
            Err(Error::Codestream(format!(
                "marker {code:#06X} appears in a tile-part header other than the first; \
                 the coding-parameter overrides belong to the first tile-part of a tile"
            )))
        }
    };

    loop {
        let m = cur.u16()?;
        match m {
            marker::SOD => break,
            marker::COM => {
                segment(cur)?;
            }
            marker::COD => {
                overrides_here(m)?;
                if tile.cod.replace(decode_cod(segment(cur)?)?).is_some() {
                    return Err(Error::Codestream(
                        "duplicate COD marker in the tile-part header".into(),
                    ));
                }
            }
            marker::QCD => {
                overrides_here(m)?;
                if tile
                    .qcd
                    .replace(decode_quant(segment(cur)?, "QCD")?)
                    .is_some()
                {
                    return Err(Error::Codestream(
                        "duplicate QCD marker in the tile-part header".into(),
                    ));
                }
            }
            marker::COC => {
                overrides_here(m)?;
                let (index, coding) = decode_coc(segment(cur)?, siz)?;
                if tile.coc.iter().any(|&(i, _)| i == index) {
                    return Err(Error::Codestream(format!(
                        "duplicate COC marker for component {index} in the tile-part header"
                    )));
                }
                tile.coc.push((index, coding));
            }
            marker::QCC => {
                overrides_here(m)?;
                let (index, quant) = decode_qcc(segment(cur)?, siz)?;
                if tile.qcc.iter().any(|&(i, _)| i == index) {
                    return Err(Error::Codestream(format!(
                        "duplicate QCC marker for component {index} in the tile-part header"
                    )));
                }
                tile.qcc.push((index, quant));
            }
            marker::RGN => {
                overrides_here(m)?;
                let (index, shift) = decode_rgn(segment(cur)?, siz)?;
                if tile.rgn.iter().any(|&(i, _)| i == index) {
                    return Err(Error::Codestream(format!(
                        "duplicate RGN marker for component {index} in the tile-part header"
                    )));
                }
                tile.rgn.push((index, shift));
            }
            // Packet lengths (A.7.3): informational, so the structure is
            // validated and the lengths discarded — the packets are read from
            // the data itself, and decoding must not depend on the hint.
            marker::PLT => {
                decode_plt(segment(cur)?)?;
            }
            // Valid here, but not yet decoded: both relocate or reorder what the
            // packet data means, so skipping one would silently decode the wrong
            // image.
            marker::POC | marker::PPT => {
                return Err(Error::Unsupported(format!(
                    "tile-part header marker {} is outside the decoded subset",
                    marker::describe(m)
                )));
            }
            // These are legal markers, but not here: TLM and PLM are
            // main-header-only (A.7.1, A.7.2), and SOP/EPH delimit packets
            // after SOD. In a tile-part header they are a malformed
            // codestream, not a missing feature.
            marker::TLM | marker::PLM | marker::SOP | marker::EPH => {
                return Err(Error::Codestream(format!(
                    "marker {m:#06X} is not permitted in a tile-part header"
                )));
            }
            other => {
                return Err(Error::Codestream(format!(
                    "unexpected marker {other:#06X} in tile-part header"
                )));
            }
        }
    }
    Ok(tile)
}

/// Resolve one tile's effective parameters: the main header with the tile-part
/// header's markers laid over it, in the standard's precedence order (A.6.1).
///
/// For each component the winner is the first of
///
/// ```text
/// tile COC  >  tile COD  >  main COC  >  main COD
/// ```
///
/// and likewise QCC > QCD down the same two levels. The middle two are the trap:
/// a **tile COD outranks a main-header COC**. So it is not enough to lay the
/// tile's markers over the main header's resolved components — the main COC
/// would win a contest it lost, and the tile would decode on a pyramid no marker
/// described.
///
/// The ladder falls out instead: the main header's resolved components already
/// encode `main COC > main COD`, so a tile COD overwrites *every* component's
/// coding (beating any main COC), and a tile COC then overwrites the one
/// component it names (beating the tile COD). An RGN in a tile-part header
/// likewise *replaces* the main header's for that component (A.6.3).
fn resolve_tile_header(main: &MainHeader, tile: &TileHeader) -> Result<MainHeader> {
    let mut header = main.clone();

    // A tile COD/QCD is the tile's new default: it displaces the main header's
    // COD/QCD *and* any main-header COC/QCC, so it lands on every component.
    if let Some(cod) = &tile.cod {
        header.cod = cod.clone();
        let coding = header.cod.coding();
        for params in &mut header.components {
            params.coding = coding.clone();
        }
    }
    if let Some(qcd) = &tile.qcd {
        header.qcd = qcd.clone();
        for params in &mut header.components {
            params.quant = qcd.clone();
        }
    }

    // Then the tile's own per-component markers, which outrank its COD/QCD.
    // `decode_coc`/`decode_qcc`/`decode_rgn` already bounded each index by the
    // SIZ component count, so these cannot land outside the vector.
    for (index, coding) in &tile.coc {
        header.components[*index].coding = coding.clone();
    }
    for (index, quant) in &tile.qcc {
        header.components[*index].quant = quant.clone();
    }
    for (index, shift) in &tile.rgn {
        header.components[*index].roi_shift = *shift;
    }

    // The tile's resolved parameters must hold every invariant the main header's
    // do: a tile COD/QCD pair can break one the main header kept.
    validate_resolved(&header)?;
    Ok(header)
}

/// SOT fields (A.4.2): the tile index, tile-part length, part index, and part
/// count. The packet-data extent is derived from `psot` by the caller.
struct Sot {
    tile_index: u16,
    psot: u32,
    tile_part_index: u8,
    num_tile_parts: u8,
}

/// Decode the SOT marker segment body (everything after `Lsot`): Isot, Psot,
/// TPsot, TNsot. `expect_consumed` enforces the fixed `Lsot == 10` layout.
fn decode_sot(mut b: Cursor<'_>) -> Result<Sot> {
    let tile_index = b.u16()?;
    let psot = b.u32()?;
    let tile_part_index = b.u8()?;
    let num_tile_parts = b.u8()?;
    b.expect_consumed("SOT")?;
    Ok(Sot {
        tile_index,
        psot,
        tile_part_index,
        num_tile_parts,
    })
}

/// Read a big-endian `u16` marker at `pos`, or `None` if it would run past the
/// end. Used to peek the marker that follows a tile-part without disturbing the
/// segment cursor.
fn read_u16(bytes: &[u8], pos: usize) -> Option<u16> {
    let hi = *bytes.get(pos)?;
    let lo = *bytes.get(pos + 1)?;
    Some(u16::from_be_bytes([hi, lo]))
}

/// One marker segment located by the structural walk: its code and its body
/// (empty for the segment-less markers).
struct Segment<'a> {
    code: u16,
    body: &'a [u8],
}

/// Upper bound on the marker segments a main header may carry.
///
/// A segment-less marker costs two input bytes but one `Segment` record, so a
/// run of them would grow the located-segment list several times faster than the
/// input it came from. The largest conformant header is far smaller than this:
/// `Csiz` tops out at 16384, and even a COC *and* a QCC per component is 32768
/// segments, so this bound cannot reject a real codestream.
const MAX_MAIN_HEADER_SEGMENTS: usize = 1 << 16;

/// Walk the main header's marker segments from just after `SOC` to the first
/// `SOT`, without judging any of them (A.6).
///
/// This is deliberately subset-agnostic: it establishes *where every segment
/// is*, and leaves "can we decode this?" to the caller. Running the whole walk
/// before any segment is interpreted is what lets a codestream carrying an
/// out-of-subset feature still be traversed end to end — which is the only way
/// a marker sitting *after* the offending one gets exercised at all.
///
/// Two shapes a naive walker gets wrong, both present in the corpus:
///
/// - the reserved range `0xFF30..=0xFF3F` carries **no** length field; reading
///   one consumes the bytes that follow (`p0_02` has `0xFF30` after its `COM`);
/// - a marker this decoder does not recognise still has a length, so the walk
///   can step over it. Whether the *decoder* may proceed past it is a different
///   question, and `parse_main_header` answers it with `Unsupported`.
///
/// Returns the segments in codestream order and the offset of the `SOT` that
/// ended the header. Truncation, a non-marker where a marker must be, and
/// `SOD`/`EOC` before any tile-part are [`Error::Codestream`].
fn walk_main_header(bytes: &[u8]) -> Result<(Vec<Segment<'_>>, usize)> {
    let mut cur = Cursor::new(bytes);

    if cur.u16()? != marker::SOC {
        return Err(Error::Codestream(
            "does not start with the SOC marker".into(),
        ));
    }

    let mut segments = Vec::new();
    let sot_offset = loop {
        let code = cur.u16()?;
        match code {
            // The first SOT ends the main header; point the offset back at it.
            marker::SOT => break cur.pos - 2,

            // A second SOC, or a delimiter that belongs to a tile-part, means
            // the header is malformed rather than merely unfamiliar.
            marker::SOC | marker::SOD | marker::EOC => {
                return Err(Error::Codestream(format!(
                    "unexpected marker {code:#06X} before any tile-part"
                )));
            }

            _ if !marker::is_marker(code) => {
                return Err(Error::Codestream(format!(
                    "expected a marker in the main header, found {code:#06X}"
                )));
            }

            // Everything else is located, not interpreted.
            _ => {
                // A segment-less marker advances only two bytes, so a run of
                // them would grow `segments` at ~12x the input. Bound it: no
                // conformant main header comes close, and the cap keeps a hostile
                // input's allocation proportional to nothing but this constant.
                if segments.len() == MAX_MAIN_HEADER_SEGMENTS {
                    return Err(Error::Codestream(format!(
                        "main header has more than {MAX_MAIN_HEADER_SEGMENTS} marker segments"
                    )));
                }
                let body = if marker::has_segment(code) {
                    segment(&mut cur)?.buf
                } else {
                    &[][..]
                };
                segments.push(Segment { code, body });
            }
        }
    };

    Ok((segments, sot_offset))
}

/// Parse the main header up to (but not into) the first `SOT`.
///
/// Returns the decoded [`MainHeader`] and the byte offset of that `SOT` marker,
/// which is where tile-part walking (issue #6) begins. Stops before any
/// entropy-coded data, so it never touches packet bytes.
///
/// The header is walked in full by [`walk_main_header`] first, then each
/// located segment is interpreted in codestream order.
fn parse_main_header(bytes: &[u8]) -> Result<(MainHeader, usize)> {
    let (segments, sot_offset) = walk_main_header(bytes)?;

    // SIZ shall be the first marker segment after SOC (A.6).
    let Some(first) = segments.first() else {
        return Err(Error::Codestream(
            "main header has no marker segments".into(),
        ));
    };
    if first.code != marker::SIZ {
        return Err(Error::Codestream(
            "SIZ must be the first marker after SOC".into(),
        ));
    }
    let siz = decode_siz(Cursor::new(first.body))?;

    // Judge the image before its coding parameters, so a codestream outside the
    // subset reports the *image* feature that blocks it (an origin, a tile grid)
    // rather than whichever coding-style bit COD happens to trip on first. Both
    // are `Unsupported`; this is about which reason is useful to read.
    validate_geometry(&siz)?;
    check_sample_budget(&siz)?;

    let mut cod = None;
    let mut qcd = None;
    let mut coc: Vec<Option<Coding>> = vec![None; siz.components.len()];
    let mut qcc: Vec<Option<Qcd>> = vec![None; siz.components.len()];
    let mut rgn: Vec<Option<u8>> = vec![None; siz.components.len()];
    let mut tlm: Vec<TlmEntry> = Vec::new();

    for seg in &segments[1..] {
        let body = || Cursor::new(seg.body);
        match seg.code {
            marker::SIZ => {
                return Err(Error::Codestream("duplicate SIZ marker".into()));
            }
            marker::COD => {
                if cod.is_some() {
                    return Err(Error::Codestream("duplicate COD marker".into()));
                }
                cod = Some(decode_cod(body())?);
            }
            marker::QCD => {
                if qcd.is_some() {
                    return Err(Error::Codestream("duplicate QCD marker".into()));
                }
                qcd = Some(decode_quant(body(), "QCD")?);
            }
            // A.6.2 and A.6.5 allow at most one COC and one QCC per component in
            // a header. A second one is not a later-wins override; it is a
            // malformed header, and guessing which wins would decode an image
            // the encoder never described. OpenJPEG is silently last-wins here;
            // rejecting is stricter, and no conformant codestream carries two.
            marker::COC => {
                let (index, coding) = decode_coc(body(), &siz)?;
                if coc[index].replace(coding).is_some() {
                    return Err(Error::Codestream(format!(
                        "duplicate COC marker for component {index}"
                    )));
                }
            }
            marker::QCC => {
                let (index, quant) = decode_qcc(body(), &siz)?;
                if qcc[index].replace(quant).is_some() {
                    return Err(Error::Codestream(format!(
                        "duplicate QCC marker for component {index}"
                    )));
                }
            }
            // A.6.3 allows one RGN per component per header, like COC and QCC.
            marker::RGN => {
                let (index, shift) = decode_rgn(body(), &siz)?;
                if rgn[index].replace(shift).is_some() {
                    return Err(Error::Codestream(format!(
                        "duplicate RGN marker for component {index}"
                    )));
                }
            }
            // Comment: recognised, carries nothing the decoder needs.
            marker::COM => {}

            // Length markers (A.7): pointers into the codestream, not decode
            // parameters — the packet and tile-part structures they describe
            // are read from the codestream itself, so decoding is identical
            // with them present or absent. TLM's entries are validated and
            // recorded; PLM's packet lengths are skipped as OpenJPEG skips
            // them (`opj_j2k_read_plm` reads nothing past `Zplm`).
            marker::TLM => decode_tlm(body(), &siz, &mut tlm)?,
            marker::PLM => decode_plm(body())?,

            // PLT is the tile-part-header form of PLM (A.7.3); in the main
            // header it is a malformed codestream, not a missing feature.
            marker::PLT => {
                return Err(Error::Codestream(
                    "PLT is a tile-part-header marker; it is not permitted in the main header"
                        .into(),
                ));
            }

            // Valid markers the decoded subset does not yet cover. Rejected
            // rather than skipped: each one changes how the codestream is
            // interpreted, so ignoring it would silently decode the wrong
            // image. PPM/PPT relocate packet headers; CRG is informational but
            // travels with features we do not decode. CAP announces
            // capabilities beyond Part 1 (an HTJ2K codestream carries one),
            // whose code-blocks this Tier-1 would misread as Part 1.
            marker::CAP
            | marker::POC
            | marker::PPM
            | marker::PPT
            | marker::CRG
            | marker::SOP
            | marker::EPH => {
                return Err(Error::Unsupported(format!(
                    "marker {} is outside the decoded subset",
                    marker::describe(seg.code)
                )));
            }

            // The reserved segment-less range is *defined* to carry nothing, so
            // it is the one thing that can be passed over safely.
            code if marker::RESERVED_NO_SEGMENT.contains(&code) => {}

            // Any other marker this decoder does not know. The walk stepped over
            // its segment — that is what lets the header be traversed at all —
            // but we will not decode past it: every marker code is allocated by
            // some part of the standard, and an unknown one may well change what
            // the packet data means (Part 2's MCT/MCC/MCO, CBD, NLT; Part 15's
            // HTJ2K block coder). Guessing that it is ignorable would trade a
            // clean rejection for a silently wrong image.
            other => {
                return Err(Error::Unsupported(format!(
                    "unrecognized marker {other:#06X} in the main header"
                )));
            }
        }
    }

    let cod = cod.ok_or_else(|| Error::Codestream("missing required COD marker".into()))?;
    let qcd = qcd.ok_or_else(|| Error::Codestream("missing required QCD marker".into()))?;

    // Start from COD/QCD's defaults, then lay each override over its component.
    // That is the whole of A.6.2's and A.6.5's resolution rule at the main
    // header's level; `resolve_tile_header` continues the ladder for a tile.
    let mut header = MainHeader::new(siz, cod, qcd);
    header.tlm = tlm;
    for (params, ((coding, quant), shift)) in header
        .components
        .iter_mut()
        .zip(coc.into_iter().zip(qcc).zip(rgn))
    {
        if let Some(coding) = coding {
            params.coding = coding;
        }
        if let Some(quant) = quant {
            params.quant = quant;
        }
        if let Some(shift) = shift {
            params.roi_shift = shift;
        }
    }

    validate_resolved(&header)?;
    Ok((header, sot_offset))
}

/// Check the invariants that only hold once a header's per-component parameters
/// are resolved — whichever header level resolved them. A tile-part COD/QCD pair
/// can break one the main header kept, so this runs for every tile header too,
/// not just the main one.
fn validate_resolved(header: &MainHeader) -> Result<()> {
    // A quantization default or override must cover every subband its
    // component decomposes into: 3·NL + 1 entries for the none/expounded
    // styles (derived's single entry is enforced in `decode_quant`, and excess
    // entries are ignored, as OpenJPEG does). NL is only known here, after the
    // overrides are resolved; discovering the shortfall per subband at decode
    // time would misreport a malformed marker as a geometry inconsistency.
    for (index, params) in header.components.iter().enumerate() {
        let needed = 3 * usize::from(params.coding.decomposition_levels) + 1;
        if params.quant.style != QuantStyle::ScalarDerived && params.quant.steps.len() < needed {
            return Err(Error::Marker(format!(
                "component {index} carries {} quantization step entries but its {} \
                 decomposition levels need {needed}",
                params.quant.steps.len(),
                params.coding.decomposition_levels
            )));
        }
        // The wavelet constrains the quantization style (E.1 ties reversible
        // to no quantization, irreversible to scalar). OpenJPEG decodes both
        // non-conformant pairings: style-0 entries still carry exponents, so
        // it derives 9/7 steps with mantissa 0 and reconstructs. This crate
        // accepts the 5/3-with-scalar direction to match the oracle (the
        // reversible path ignores the mantissas — same output) but rejects
        // 9/7-with-none rather than adopt the oracle's zero-mantissa
        // interpretation of a stream the standard forbids — a documented
        // stricter-than-oracle call (docs/correctness.md), relaxed if a real
        // file ever trips it.
        if params.coding.transform == Transform::Irreversible97
            && params.quant.style == QuantStyle::None
        {
            return Err(Error::Marker(format!(
                "component {index} pairs the 9/7 wavelet with the no-quantization style"
            )));
        }
    }

    // The colour transform is signalled by COD but constrains SIZ, so it can
    // only be checked once both are read.
    if header.cod.multiple_component_transform {
        crate::mct::check_geometry(&header.siz.components)?;
        // Which colour transform applies follows from the wavelet, and the
        // wavelet is per component once COC is in play. RCT is defined over
        // three components reconstructed as integers, so all three must be 5/3;
        // a COC that moves one of them to 9/7 describes a transform that does
        // not exist.
        for (index, params) in header.components.iter().take(3).enumerate() {
            if params.coding.transform != Transform::Reversible53 {
                return Err(Error::Unsupported(format!(
                    "COD signals the colour transform but component {index} uses the 9/7 \
                     wavelet; the irreversible colour transform (ICT) is not decoded"
                )));
            }
        }
    }

    Ok(())
}

/// Upper bound on the declared image area (`Xsiz * Ysiz`), a robustness guard
/// against a malformed SIZ steering the per-subband and DWT buffers into an
/// overflowing or out-of-memory allocation. Sized for the GRIB2 grids decoded
/// today: 2^26 samples is 256 MiB as `i32`, well above operational grids (HRRR
/// ~1.9M, MRMS ~24.5M) and below anything that threatens the decode. Not a
/// format limit — raise it as larger imagery comes into scope.
const MAX_IMAGE_SAMPLES: u64 = 1 << 26;

/// Bound the samples a codestream can ask the decoder to allocate.
///
/// Every component is reconstructed into its own buffer, so the cost is the
/// *sum* of the component areas, not the image area. `Csiz` reaches 16384, so a
/// large image declared with many components would otherwise multiply the
/// allocation far past any single-component guard.
fn check_sample_budget(siz: &Siz) -> Result<()> {
    let mut total: u64 = 0;
    for index in 0..siz.components.len() {
        let (width, height) = siz
            .component_extent(index)
            .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {index}")))?;
        total += u64::from(width) * u64::from(height);
        if total > MAX_IMAGE_SAMPLES {
            return Err(Error::Unsupported(format!(
                "components total over {MAX_IMAGE_SAMPLES} samples, above the decode guard"
            )));
        }
    }
    Ok(())
}

/// Enforce the decoded geometry subset on the SIZ fields: a tile grid anchored
/// at the canvas origin, bounded in area and tile count. A nonzero image origin
/// is valid JPEG 2000 but not yet decoded, so reject it cleanly here rather than
/// let an out-of-subset origin reach the reconstruction, and reject an unbounded
/// area or tile count before either reaches the buffer allocations.
fn validate_geometry(siz: &Siz) -> Result<()> {
    if siz.x_size == 0 || siz.y_size == 0 {
        return Err(Error::Marker("SIZ declares a zero-size image".into()));
    }
    if siz.x_offset != 0 || siz.y_offset != 0 {
        return Err(Error::Unsupported(format!(
            "image offset ({}, {}); the decoded subset is canvas-origin only",
            siz.x_offset, siz.y_offset
        )));
    }
    // The standard requires XTOsiz <= XOsiz (Table A-9), so with the image
    // pinned at the origin the tile grid is pinned there too. A nonzero tile
    // offset here is therefore a malformed SIZ, not an undecoded feature —
    // but it only becomes one once the image offset above is decoded, so it
    // stays `Unsupported` and moves with #96.
    if siz.tile_x_offset != 0 || siz.tile_y_offset != 0 {
        return Err(Error::Unsupported(format!(
            "tile offset ({}, {}); the decoded subset is canvas-origin only",
            siz.tile_x_offset, siz.tile_y_offset
        )));
    }
    if siz.tile_width == 0 || siz.tile_height == 0 {
        return Err(Error::Marker("SIZ declares a zero-size tile".into()));
    }
    if siz.x_size as u64 * siz.y_size as u64 > MAX_IMAGE_SAMPLES {
        return Err(Error::Unsupported(format!(
            "image area {}×{} exceeds the decode guard of {MAX_IMAGE_SAMPLES} samples",
            siz.x_size, siz.y_size
        )));
    }
    // `Isot` is a `u16` running 0..=65534 (Table A-10), so a grid finer than
    // that has tiles no tile-part can name. The bound also caps the per-tile
    // bookkeeping — one resolved header and one geometry walk each — that the
    // sample budget alone does not: a 1×1 tile costs a header but almost no
    // samples.
    const MAX_TILES: u32 = 65535;
    let tiles = siz.num_tiles();
    if tiles > MAX_TILES {
        return Err(Error::Marker(format!(
            "tile grid is {}×{} = {tiles} tiles, past the {MAX_TILES}-tile limit on Isot",
            siz.tile_grid().0,
            siz.tile_grid().1,
        )));
    }
    Ok(())
}

/// Decode SIZ — image and tile geometry plus every component's depth, sign, and
/// sub-sampling (A.5.1).
///
/// Parses all `Csiz` components. Whether the decoder can *reconstruct* them is
/// a separate question, enforced by [`validate_geometry`] and
/// [`check_sample_budget`] once the whole main header is read, so a
/// multi-component codestream still parses cleanly here.
///
/// `Ssiz` is a depth-minus-one in the low 7 bits with the sign in bit 7, so the
/// declared depth is `1..=128`; the standard caps it at 38 (Table A-11) and
/// anything above that is a malformed field, not an unsupported feature.
fn decode_siz(mut b: Cursor<'_>) -> Result<Siz> {
    let _rsiz = b.u16()?; // capabilities / profile — not needed by the decoder
    let x_size = b.u32()?;
    let y_size = b.u32()?;
    let x_offset = b.u32()?;
    let y_offset = b.u32()?;
    let tile_width = b.u32()?;
    let tile_height = b.u32()?;
    let tile_x_offset = b.u32()?;
    let tile_y_offset = b.u32()?;
    let csiz = b.u16()?;

    if csiz == 0 {
        return Err(Error::Marker("SIZ declares zero components".into()));
    }
    if csiz > markers::MAX_COMPONENTS {
        return Err(Error::Marker(format!(
            "SIZ declares {csiz} components, above the limit of {}",
            markers::MAX_COMPONENTS
        )));
    }
    // Each component record is 3 bytes. Check the segment can actually hold them
    // before reserving, so a lying `Csiz` cannot steer the allocation.
    if b.remaining() != 3 * csiz as usize {
        return Err(Error::Codestream(format!(
            "SIZ has {} bytes for {csiz} component records, expected {}",
            b.remaining(),
            3 * csiz as usize,
        )));
    }

    let mut components = Vec::with_capacity(csiz as usize);
    for index in 0..csiz {
        let ssiz = b.u8()?;
        let x_sampling = b.u8()?;
        let y_sampling = b.u8()?;
        let bit_depth = (ssiz & 0x7F) + 1;
        if bit_depth > 38 {
            return Err(Error::Marker(format!(
                "component {index} declares bit depth {bit_depth}, above the limit of 38"
            )));
        }
        if x_sampling == 0 || y_sampling == 0 {
            return Err(Error::Marker(format!(
                "component {index} declares a zero sub-sampling factor ({x_sampling}, {y_sampling})"
            )));
        }
        components.push(SizComponent {
            bit_depth,
            signed: ssiz & 0x80 != 0,
            x_sampling,
            y_sampling,
        });
    }
    b.expect_consumed("SIZ")?;

    let siz = Siz {
        x_size,
        y_size,
        x_offset,
        y_offset,
        tile_width,
        tile_height,
        tile_x_offset,
        tile_y_offset,
        components,
    };
    // Geometry legality (`validate_geometry`) and the sample budget
    // (`check_sample_budget`) are the caller's to apply, in that order, so
    // this function stays a pure reader of the marker segment.
    Ok(siz)
}

/// Decode COD — default coding style (A.6.1): transform, decomposition depth,
/// progression, layers, code-block size/style. Rejects explicit precincts, the
/// non-default code-block styles, Part 2's array MCT, and ICT.
fn decode_cod(mut b: Cursor<'_>) -> Result<Cod> {
    // Scod bit 0: user-defined precincts in SPcod. Bit 1: SOP may precede each
    // packet. Bit 2: EPH follows each packet header. Bits 3-7 are reserved.
    let scod = b.u8()?;
    if scod & 0x01 != 0 {
        return Err(Error::Unsupported(
            "explicit precinct partition; the subset uses maximal precincts".into(),
        ));
    }
    if scod & 0xF8 != 0 {
        return Err(Error::Marker(format!(
            "COD sets reserved Scod bits {:#04X}",
            scod & 0xF8
        )));
    }
    let use_sop = scod & 0x02 != 0;
    let use_eph = scod & 0x04 != 0;

    let progression = match b.u8()? {
        0 => Progression::Lrcp,
        1 => Progression::Rlcp,
        2 => Progression::Rpcl,
        3 => Progression::Pcrl,
        4 => Progression::Cprl,
        other => {
            return Err(Error::Marker(format!("reserved progression order {other}")));
        }
    };

    // Every packet costs at least one byte, so a layer count is bounded by the
    // tile-part in practice; a declared zero is malformed (ISO Table A-16).
    let layers = b.u16()?;
    if layers == 0 {
        return Err(Error::Marker("COD declares zero quality layers".into()));
    }

    // SGcod multiple-component transform: 0 = none, 1 = the Part 1 colour
    // transform over the first three components. 2 selects Part 2's array MCT,
    // which is a different feature entirely.
    let mct = b.u8()?;
    let multiple_component_transform = match mct {
        0 => false,
        1 => true,
        other => {
            return Err(Error::Unsupported(format!(
                "multiple-component transform type {other}; only the Part 1 colour transform is decoded"
            )));
        }
    };

    let coding = decode_coding(&mut b, "COD")?;
    let Coding {
        decomposition_levels,
        code_block_width,
        code_block_height,
        code_block_style,
        transform,
        precinct_sizes,
    } = coding;
    b.expect_consumed("COD")?;

    // The wavelet picks the colour transform: 5/3 pairs with the reversible RCT
    // (G.2), 9/7 with the irreversible ICT (G.3). Only RCT is decoded.
    if multiple_component_transform && transform == Transform::Irreversible97 {
        return Err(Error::Unsupported(
            "irreversible colour transform (ICT) on the 9/7 path".into(),
        ));
    }

    Ok(Cod {
        progression,
        layers,
        decomposition_levels,
        code_block_width,
        code_block_height,
        code_block_style,
        use_sop,
        use_eph,
        multiple_component_transform,
        transform,
        // Maximal precincts (PPx=PPy=15) when Scod bit 0 is clear, signalled by
        // an empty list; explicit precincts were rejected above.
        precinct_sizes,
    })
}

/// Decode the `SPcod`/`SPcoc` tail shared by COD and COC (Tables A-12, A-14):
/// decomposition levels, the two code-block exponents, the style byte, and the
/// wavelet. `origin` names the marker for error messages.
///
/// Explicit precinct sizes are rejected by the caller before this runs (they are
/// signalled in `Scod`/`Scoc`, not here), so `precinct_sizes` always comes back
/// empty and the caller must have consumed the whole segment.
fn decode_coding(b: &mut Cursor<'_>, origin: &str) -> Result<Coding> {
    let decomposition_levels = b.u8()?;
    // Table A-15: 0–32 decomposition levels; 33–255 are reserved. A reserved
    // field encoding rejects at parse, like the wavelet and style bytes below.
    if decomposition_levels > 32 {
        return Err(Error::Marker(format!(
            "{origin} sets reserved decomposition level count {decomposition_levels} (max 32)"
        )));
    }
    let code_block_width = b.u8()?;
    let code_block_height = b.u8()?;
    let code_block_style = b.u8()?;
    // Each style flag changes how Tier-1 reads a code-block's coded segments.
    // Ignoring one does not decode a slightly different image, it decodes the
    // wrong one, so reject rather than half-decode until each lands. `restart`,
    // `predictable termination` and `segmentation symbols` are decoded; the rest
    // are not.
    use markers::code_block_style::{PTERM, SEGSYM, TERMALL};
    let undecoded = code_block_style & !(TERMALL | PTERM | SEGSYM);
    if undecoded != 0 {
        return Err(Error::Unsupported(format!(
            "{origin} code-block style ({}) is outside the decoded subset",
            markers::code_block_style::describe(undecoded),
        )));
    }
    let transform = match b.u8()? {
        0 => Transform::Irreversible97,
        1 => Transform::Reversible53,
        other => {
            return Err(Error::Marker(format!(
                "{origin} sets reserved wavelet transform {other}"
            )));
        }
    };
    Ok(Coding {
        decomposition_levels,
        code_block_width,
        code_block_height,
        code_block_style,
        transform,
        precinct_sizes: Vec::new(),
    })
}

/// The component index a COC or QCC applies to (`Ccoc`, `Cqcc`). One byte while
/// `Csiz < 257`, two bytes otherwise (A.6.2, A.6.5) — the width depends on the
/// component count, so SIZ must already be parsed.
fn component_index(b: &mut Cursor<'_>, siz: &Siz, origin: &str) -> Result<usize> {
    let count = siz.components.len();
    let index = if count < 257 {
        usize::from(b.u8()?)
    } else {
        usize::from(b.u16()?)
    };
    if index >= count {
        return Err(Error::Marker(format!(
            "{origin} targets component {index}, but SIZ declares {count}"
        )));
    }
    Ok(index)
}

/// Decode COC — a coding-style override for one component (A.6.2).
///
/// Returns the component index and the coding parameters that replace COD's for
/// it. `Scoc` carries only the precinct flag; every other bit is reserved.
fn decode_coc(mut b: Cursor<'_>, siz: &Siz) -> Result<(usize, Coding)> {
    let index = component_index(&mut b, siz, "COC")?;
    let scoc = b.u8()?;
    if scoc & 0x01 != 0 {
        return Err(Error::Unsupported(
            "explicit precinct partition; the subset uses maximal precincts".into(),
        ));
    }
    // Only bit 0 is defined in `Scoc` (Table A-21). SOP and EPH are signalled
    // once for the tile, in COD's `Scod`, and have no per-component form — so
    // unlike `Scod` there is nothing else here to read. OpenJPEG stores the byte
    // without validating it and consults only bit 0.
    if scoc & 0xFE != 0 {
        return Err(Error::Marker(format!(
            "COC sets reserved Scoc bits {:#04X}",
            scoc & 0xFE
        )));
    }
    let coding = decode_coding(&mut b, "COC")?;
    b.expect_consumed("COC")?;
    Ok((index, coding))
}

/// Decode QCC — a quantization override for one component (A.6.5). The body
/// after `Cqcc` is a `QCD` body, byte for byte.
fn decode_qcc(mut b: Cursor<'_>, siz: &Siz) -> Result<(usize, Qcd)> {
    let index = component_index(&mut b, siz, "QCC")?;
    let quant = decode_quant(b, "QCC")?;
    Ok((index, quant))
}

/// Decode RGN — a region of interest for one component (A.6.3).
///
/// Returns the component index and its maxshift. `Srgn` selects the ROI style,
/// and `0` (implicit — the maxshift of Annex H) is the only style Part 1
/// defines. OpenJPEG reads the byte and never looks at it, so a codestream using
/// a Part 2 style decodes there as though it were maxshift.
fn decode_rgn(mut b: Cursor<'_>, siz: &Siz) -> Result<(usize, u8)> {
    let index = component_index(&mut b, siz, "RGN")?;
    let srgn = b.u8()?;
    if srgn != 0 {
        return Err(Error::Unsupported(format!(
            "region-of-interest style {srgn}; only the implicit maxshift (Srgn = 0) is decoded"
        )));
    }
    let shift = b.u8()?;
    b.expect_consumed("RGN")?;
    Ok((index, shift))
}

/// Decode TLM — tile-part lengths (A.7.1) — appending onto `entries`.
///
/// `Stlm` packs two field widths: `ST` (bits 4–5) is the tile-index width in
/// bytes, where 0 means the index is implied by entry order across all TLM
/// markers, and `SP` (bit 6) selects 16- or 32-bit lengths. `ST = 3` and the
/// reserved bits (0–3 and 7) are illegal encodings; OpenJPEG warns and ignores
/// the whole TLM chain on `ST = 3` or a bad entry size and never looks at the
/// reserved bits, but as with `Scoc` rejecting is stricter and no conformant
/// codestream carries them.
///
/// An entry may only name a tile the grid actually holds. OpenJPEG makes the
/// same range check against its grid.
fn decode_tlm(mut b: Cursor<'_>, siz: &Siz, entries: &mut Vec<TlmEntry>) -> Result<()> {
    b.u8()?; // Ztlm: this marker's position among the TLMs
    let stlm = b.u8()?;
    if stlm & 0x8F != 0 {
        return Err(Error::Marker(format!(
            "TLM sets reserved Stlm bits {:#04X}",
            stlm & 0x8F
        )));
    }
    let st = (stlm >> 4) & 0x3;
    if st == 3 {
        return Err(Error::Marker(
            "TLM tile-index width ST = 3 is not defined".into(),
        ));
    }
    let sp = (stlm >> 6) & 0x1;

    let entry_size = usize::from(st) + 2 * (usize::from(sp) + 1);
    if !b.remaining().is_multiple_of(entry_size) {
        return Err(Error::Codestream(format!(
            "TLM carries {} entry bytes, not a multiple of its {entry_size}-byte entries",
            b.remaining()
        )));
    }

    while b.remaining() > 0 {
        let tile_index = match st {
            0 => entries.len() as u32,
            1 => u32::from(b.u8()?),
            _ => u32::from(b.u16()?),
        };
        if tile_index >= siz.num_tiles() {
            return Err(Error::Marker(format!(
                "TLM names tile {tile_index}, but the grid holds {} tiles",
                siz.num_tiles()
            )));
        }
        let length = match sp {
            0 => u32::from(b.u16()?),
            _ => b.u32()?,
        };
        entries.push(TlmEntry { tile_index, length });
    }
    Ok(())
}

/// Decode PLM — main-header packet lengths (A.7.2).
///
/// Validated for presence and otherwise skipped, exactly as OpenJPEG's
/// `opj_j2k_read_plm` does: an `Nplm`/`Iplm` chain may split across PLM
/// markers mid-entry, so per-marker structural validation would reject
/// codestreams the oracle accepts. The packets themselves are read from the
/// codestream, so nothing here is needed to decode.
fn decode_plm(mut b: Cursor<'_>) -> Result<()> {
    b.u8()?; // Zplm: this marker's position among the PLMs
    Ok(())
}

/// Decode PLT — tile-part packet lengths (A.7.3).
///
/// Each packet length is a base-128 chain of bytes, high bit meaning "another
/// byte follows". The lengths are informational and discarded; what is checked
/// is that the last chain is whole — a final byte with its continuation bit
/// set is a truncated entry. OpenJPEG rejects the same way unless every
/// payload bit of the dangling chain is zero (`opj_j2k_read_plt` tests the
/// accumulated *value* at segment end, and `0 << 7` stays 0); rejecting the
/// all-zero dangle too is stricter, the same documented stance as the TLM
/// encodings above.
fn decode_plt(mut b: Cursor<'_>) -> Result<()> {
    b.u8()?; // Zplt: this marker's position among the tile-part's PLTs
    // The chains are self-delimiting, so whole-ness is one property: the last
    // byte must not claim a continuation.
    let chains = b.take(b.remaining())?;
    if chains.last().is_some_and(|&last| last & 0x80 != 0) {
        return Err(Error::Codestream(
            "PLT ends in the middle of a packet-length entry".into(),
        ));
    }
    Ok(())
}

/// Decode the quantization body shared by QCD (A.6.4) and QCC (A.6.5): style,
/// guard bits, and the per-subband (exponent, mantissa) step parameters. QCC's
/// body after `Cqcc` is a QCD body byte for byte, so `origin` only names the
/// marker for error messages — a malformed override must not report as a fault
/// in the codestream-wide default.
fn decode_quant(mut b: Cursor<'_>, origin: &str) -> Result<Qcd> {
    let sqcd = b.u8()?;
    let guard_bits = sqcd >> 5;
    let style = match sqcd & 0x1F {
        0 => QuantStyle::None,
        1 => QuantStyle::ScalarDerived,
        2 => QuantStyle::ScalarExpounded,
        other => {
            return Err(Error::Marker(format!(
                "{origin} sets reserved quantization style {other}"
            )));
        }
    };

    // A step table covers at most the 3·NL + 1 subbands of a 32-level
    // decomposition (A.6.4, with NL ≤ 32 per Table A-15), so 97 entries.
    // Entries past that are kept out of the table — but still consumed — the
    // way OpenJPEG caps at J2K_MAXBANDS and decodes (verified empirically:
    // a 120-entry QCD decodes without a warning). Dropping rather than
    // rejecting also keeps a hostile 65535-byte Lqcd from inflating the
    // per-component `Qcd` clones in [`MainHeader::new`].
    const MAX_STEP_ENTRIES: usize = 3 * 32 + 1;

    let mut steps = Vec::new();
    match style {
        // No quantization (reversible): one byte per subband, high 5 bits are
        // the exponent, mantissa is 0.
        QuantStyle::None => {
            if b.remaining() == 0 {
                return Err(Error::Codestream(format!(
                    "{origin} carries no step entries"
                )));
            }
            while b.remaining() > 0 {
                let v = b.u8()?;
                if steps.len() < MAX_STEP_ENTRIES {
                    steps.push((v >> 3, 0));
                }
            }
        }
        // Scalar: 16-bit per entry, high 5 bits exponent, low 11 bits mantissa.
        // Derived signals one entry (LL); expounded one per subband.
        QuantStyle::ScalarDerived | QuantStyle::ScalarExpounded => {
            if b.remaining() == 0 || !b.remaining().is_multiple_of(2) {
                return Err(Error::Codestream(format!(
                    "{origin} step table is truncated"
                )));
            }
            if style == QuantStyle::ScalarDerived && b.remaining() != 2 {
                return Err(Error::Codestream(format!(
                    "derived {origin} must carry exactly one step entry"
                )));
            }
            while b.remaining() > 0 {
                let v = b.u16()?;
                if steps.len() < MAX_STEP_ENTRIES {
                    steps.push((((v >> 11) & 0x1F) as u8, v & 0x07FF));
                }
            }
        }
    }

    Ok(Qcd {
        style,
        guard_bits,
        steps,
    })
}

/// Read a marker segment's length field and return a [`Cursor`] over its body.
///
/// `Lmarker` (A.4) counts the two length bytes but not the two marker bytes, so
/// the body is `Lmarker - 2` bytes. A length below 2 or past the buffer end is a
/// malformed codestream.
fn segment<'a>(cur: &mut Cursor<'a>) -> Result<Cursor<'a>> {
    let len = cur.u16()? as usize;
    if len < 2 {
        return Err(Error::Codestream("marker segment length below 2".into()));
    }
    let body = cur.take(len - 2)?;
    Ok(Cursor::new(body))
}

/// Bounds-checked big-endian byte cursor. Every read maps an overrun to
/// [`Error::Codestream`] so truncation is a typed error, never a panic.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// A cursor over `buf` starting at an absolute offset, for resuming a walk
    /// (e.g. tile-parts) from a position the main-header pass returned.
    fn at(buf: &'a [u8], pos: usize) -> Self {
        Self { buf, pos }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::Codestream("truncated marker segment".into()));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Assert the whole segment body was consumed; a trailing remainder means
    /// the declared length and the field layout disagree.
    fn expect_consumed(&self, marker: &str) -> Result<()> {
        if self.remaining() != 0 {
            return Err(Error::Codestream(format!(
                "{marker} segment has {} unexpected trailing byte(s)",
                self.remaining()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
