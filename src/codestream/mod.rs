//! Stage 1 — codestream parsing (ISO/IEC 15444-1 Annex A).
//!
//! Walks the marker segments of a raw J2K codestream: the main header
//! (SIZ / COD / QCD, plus optional COC / QCC / RGN / POC / COM), then the
//! tile-parts (SOT … SOD … data). Produces a [`MainHeader`] of decode
//! parameters and the byte ranges of each tile's packet data — everything the
//! later stages need, with no interpretation of the entropy-coded bytes yet.
//!
//! The main header is located before it is judged: [`walk_main_header`] finds
//! every marker segment without caring what the decoder supports, and
//! [`parse_main_header`] then interprets them. That split is what lets a
//! codestream be traversed past a feature we cannot decode, and is what makes
//! the reserved segment-less markers and unknown segments handleable at all.

pub mod markers;

use crate::{Error, Result};
use markers::{Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform, marker};

/// Parsed main-header decode parameters. COC/QCC/RGN component overrides will
/// live here too once needed; for the single-component subset the defaults
/// usually suffice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHeader {
    pub siz: Siz,
    pub cod: Cod,
    pub qcd: Qcd,
}

/// One tile-part: its tile index and the slice of packet data between SOD and
/// the next marker. Multiple tile-parts can carry one tile; the GRIB2 common
/// case is a single tile in a single part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilePart<'a> {
    pub tile_index: u16,
    pub data: &'a [u8],
}

/// A parsed codestream: main header plus the tile-part data ranges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codestream<'a> {
    pub header: MainHeader,
    pub tile_parts: Vec<TilePart<'a>>,
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
    let tile_parts = walk_tile_parts(bytes, sot_offset)?;
    Ok(Codestream { header, tile_parts })
}

/// Walk the tile-parts from the first `SOT` to the closing `EOC` (A.4.2, A.4.4).
///
/// The GRIB2 subset is exactly one tile carried in one tile-part, so this reads
/// a single `SOT … SOD … packet-data` run and requires `EOC` to follow it.
/// Multiple tiles or tile-parts reject with [`Error::Unsupported`]; a `Psot`
/// overrun, a truncated `SOT`, or a missing `EOC` reject with
/// [`Error::Codestream`].
fn walk_tile_parts(bytes: &[u8], sot_offset: usize) -> Result<Vec<TilePart<'_>>> {
    let mut cur = Cursor::at(bytes, sot_offset);

    // `parse_main_header` stopped on this SOT, so it is present; re-read it here
    // so this function owns the whole tile-part structure.
    if cur.u16()? != marker::SOT {
        return Err(Error::Codestream(
            "tile-part does not start with SOT".into(),
        ));
    }
    let sot = decode_sot(segment(&mut cur)?)?;

    // Single tile, single tile-part. Isot is the tile index, TPsot the part
    // index within the tile, TNsot the part count (0 = "not stated").
    if sot.tile_index != 0 {
        return Err(Error::Unsupported(format!(
            "tile index {}; the subset is a single tile",
            sot.tile_index
        )));
    }
    if sot.tile_part_index != 0 || sot.num_tile_parts > 1 {
        return Err(Error::Unsupported(
            "multiple tile-parts; the subset is a single tile-part".into(),
        ));
    }

    // Tile-part header: only SOD (and a skippable COM) belong here in the
    // subset; tile-level coding/quant overrides are not yet decoded.
    loop {
        let m = cur.u16()?;
        match m {
            marker::SOD => break,
            marker::COM => {
                segment(&mut cur)?;
            }
            marker::COD
            | marker::COC
            | marker::QCD
            | marker::QCC
            | marker::RGN
            | marker::POC
            | marker::TLM
            | marker::PLT
            | marker::PPT
            | marker::SOP
            | marker::EPH => {
                return Err(Error::Unsupported(format!(
                    "tile-part header marker {m:#06X} is outside the decoded subset"
                )));
            }
            other => {
                return Err(Error::Codestream(format!(
                    "unexpected marker {other:#06X} in tile-part header"
                )));
            }
        }
    }
    let data_start = cur.pos;

    // Psot counts from the SOT marker's first byte to the end of the tile-part.
    // Psot == 0 marks the last tile-part: it runs to the closing EOC (A.4.2).
    let data_end = if sot.psot == 0 {
        bytes
            .len()
            .checked_sub(2)
            .filter(|&end| end >= data_start && read_u16(bytes, end) == Some(marker::EOC))
            .ok_or_else(|| Error::Codestream("Psot=0 tile-part is not terminated by EOC".into()))?
    } else {
        let end = sot_offset
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

    let data = &bytes[data_start..data_end];

    // A single tile-part must be followed by EOC. A second SOT means more than
    // one tile-part, which the subset does not decode.
    match read_u16(bytes, data_end) {
        Some(marker::EOC) => {}
        Some(marker::SOT) => {
            return Err(Error::Unsupported(
                "multiple tile-parts; the subset is a single tile-part".into(),
            ));
        }
        Some(other) => {
            return Err(Error::Codestream(format!(
                "expected EOC after the tile-part, found {other:#06X}"
            )));
        }
        None => return Err(Error::Codestream("missing EOC after the tile-part".into())),
    }

    Ok(vec![TilePart {
        tile_index: sot.tile_index,
        data,
    }])
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
                qcd = Some(decode_qcd(body())?);
            }
            // Comment: recognised, carries nothing the decoder needs.
            marker::COM => {}

            // Valid markers the decoded subset does not yet cover. Rejected
            // rather than skipped: each one changes how the codestream is
            // interpreted, so ignoring it would silently decode the wrong
            // image. PPM/PPT relocate packet headers; PLM/PLT/TLM/CRG are
            // informational but travel with features we do not decode. CAP
            // announces capabilities beyond Part 1 (an HTJ2K codestream carries
            // one), whose code-blocks this Tier-1 would misread as Part 1.
            marker::CAP
            | marker::COC
            | marker::QCC
            | marker::RGN
            | marker::POC
            | marker::TLM
            | marker::PLM
            | marker::PLT
            | marker::PPM
            | marker::PPT
            | marker::CRG
            | marker::SOP
            | marker::EPH => {
                return Err(Error::Unsupported(format!(
                    "marker {:#06X} is outside the decoded subset",
                    seg.code
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

    // The colour transform is signalled by COD but constrains SIZ, so it can
    // only be checked once both are read.
    if cod.multiple_component_transform {
        crate::mct::check_geometry(&siz.components)?;
    }

    Ok((MainHeader { siz, cod, qcd }, sot_offset))
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

/// Enforce the decoded geometry subset on the SIZ fields: a single tile at the
/// canvas origin, bounded in area. The general canvas (nonzero image/tile
/// offsets, a multi-tile grid) is valid JPEG 2000 but not yet decoded, so reject
/// it cleanly here rather than let an out-of-subset origin reach the DWT (whose
/// interleaving assumes even, canvas-anchored subband origins) or an unbounded
/// area reach the buffer allocations.
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
    if siz.tile_x_offset != 0 || siz.tile_y_offset != 0 {
        return Err(Error::Unsupported(format!(
            "tile offset ({}, {}); the decoded subset is canvas-origin only",
            siz.tile_x_offset, siz.tile_y_offset
        )));
    }
    if siz.tile_width == 0 || siz.tile_height == 0 {
        return Err(Error::Marker("SIZ declares a zero-size tile".into()));
    }
    // A single tile must span the whole image; a smaller tile means a multi-tile
    // grid, which is not yet decoded.
    if (siz.tile_width as u64) < siz.x_size as u64 || (siz.tile_height as u64) < siz.y_size as u64 {
        return Err(Error::Unsupported(
            "tile smaller than the image (multi-tile grid); the decoded subset is single-tile"
                .into(),
        ));
    }
    if siz.x_size as u64 * siz.y_size as u64 > MAX_IMAGE_SAMPLES {
        return Err(Error::Unsupported(format!(
            "image area {}×{} exceeds the decode guard of {MAX_IMAGE_SAMPLES} samples",
            siz.x_size, siz.y_size
        )));
    }
    Ok(())
}

/// Decode SIZ — image and tile geometry plus every component's depth, sign, and
/// sub-sampling (A.5.1).
///
/// Parses all `Csiz` components. Whether the decoder can *reconstruct* them is a
/// separate question, enforced by [`check_subset`] after the whole main header
/// is read, so a multi-component codestream still parses cleanly here.
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
    // Geometry legality (`validate_geometry`) and the decoded subset
    // (`check_subset`) are the caller's to apply, in that order, so this
    // function stays a pure reader of the marker segment.
    Ok(siz)
}

/// Decode COD — default coding style (A.6.1): transform, decomposition depth,
/// progression, layers, code-block size/style. Enforces LRCP, a single layer,
/// no precincts, no multi-component transform.
fn decode_cod(mut b: Cursor<'_>) -> Result<Cod> {
    let scod = b.u8()?;
    // Scod bit 0: user-defined precincts present in SPcod; bits 1/2: SOP/EPH.
    if scod & 0x01 != 0 {
        return Err(Error::Unsupported(
            "explicit precinct partition; the subset uses maximal precincts".into(),
        ));
    }
    if scod & 0x06 != 0 {
        return Err(Error::Unsupported(
            "SOP/EPH error-resilience markers are outside the decoded subset".into(),
        ));
    }

    let progression = match b.u8()? {
        0 => Progression::Lrcp,
        p @ 1..=4 => {
            return Err(Error::Unsupported(format!(
                "progression order {p}; the subset is LRCP only"
            )));
        }
        other => {
            return Err(Error::Marker(format!("reserved progression order {other}")));
        }
    };

    let layers = b.u16()?;
    if layers != 1 {
        return Err(Error::Unsupported(format!(
            "{layers} quality layers; the subset is single-layer"
        )));
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

    let decomposition_levels = b.u8()?;
    let code_block_width = b.u8()?;
    let code_block_height = b.u8()?;
    let code_block_style = b.u8()?;
    // Each style flag changes how Tier-1 reads a code-block's coded segments.
    // Ignoring one does not decode a slightly different image, it decodes the
    // wrong one, so reject rather than half-decode until each lands.
    if code_block_style != 0 {
        return Err(Error::Unsupported(format!(
            "code-block style ({}); the subset uses the default style",
            markers::code_block_style::describe(code_block_style),
        )));
    }
    let transform = match b.u8()? {
        0 => Transform::Irreversible97,
        1 => Transform::Reversible53,
        other => return Err(Error::Marker(format!("reserved wavelet transform {other}"))),
    };
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
        multiple_component_transform,
        transform,
        // Maximal precincts (PPx=PPy=15) when Scod bit 0 is clear, signalled by
        // an empty list; explicit precincts were rejected above.
        precinct_sizes: Vec::new(),
    })
}

/// Decode QCD — default quantization (A.6.4): style, guard bits, and the
/// per-subband (exponent, mantissa) step parameters.
fn decode_qcd(mut b: Cursor<'_>) -> Result<Qcd> {
    let sqcd = b.u8()?;
    let guard_bits = sqcd >> 5;
    let style = match sqcd & 0x1F {
        0 => QuantStyle::None,
        1 => QuantStyle::ScalarDerived,
        2 => QuantStyle::ScalarExpounded,
        other => {
            return Err(Error::Marker(format!(
                "reserved quantization style {other}"
            )));
        }
    };

    let mut steps = Vec::new();
    match style {
        // No quantization (reversible): one byte per subband, high 5 bits are
        // the exponent, mantissa is 0.
        QuantStyle::None => {
            if b.remaining() == 0 {
                return Err(Error::Codestream("QCD carries no step entries".into()));
            }
            while b.remaining() > 0 {
                let v = b.u8()?;
                steps.push((v >> 3, 0));
            }
        }
        // Scalar: 16-bit per entry, high 5 bits exponent, low 11 bits mantissa.
        // Derived signals one entry (LL); expounded one per subband.
        QuantStyle::ScalarDerived | QuantStyle::ScalarExpounded => {
            if b.remaining() == 0 || !b.remaining().is_multiple_of(2) {
                return Err(Error::Codestream("QCD step table is truncated".into()));
            }
            if style == QuantStyle::ScalarDerived && b.remaining() != 2 {
                return Err(Error::Codestream(
                    "derived QCD must carry exactly one step entry".into(),
                ));
            }
            while b.remaining() > 0 {
                let v = b.u16()?;
                steps.push((((v >> 11) & 0x1F) as u8, v & 0x07FF));
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
