//! Stage 1 — codestream parsing (ISO/IEC 15444-1 Annex A).
//!
//! Walks the marker segments of a raw J2K codestream: the main header
//! (SIZ / COD / QCD, plus optional COC / QCC / RGN / POC / COM), then the
//! tile-parts (SOT … SOD … data). Produces a [`MainHeader`] of decode
//! parameters and the byte ranges of each tile's packet data — everything the
//! later stages need, with no interpretation of the entropy-coded bytes yet.

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

/// Parse a raw codestream (must start with SOC, end with EOC).
///
/// Rejects the JP2 box wrapper (callers pass the bare codestream) and anything
/// outside the single-component subset with [`Error::Unsupported`].
///
/// [`Error::Unsupported`]: crate::Error::Unsupported
pub fn parse(bytes: &[u8]) -> Result<Codestream<'_>> {
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
    // subset; tile-level coding/quant overrides are a later phase.
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
            | marker::SOP
            | marker::EPH => {
                return Err(Error::Unsupported(format!(
                    "tile-part header marker {m:#06X} is outside the Phase 1 subset"
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

/// Parse the main header up to (but not into) the first `SOT`.
///
/// Returns the decoded [`MainHeader`] and the byte offset of that `SOT` marker,
/// which is where tile-part walking (issue #6) begins. Stops before any
/// entropy-coded data, so it never touches packet bytes.
fn parse_main_header(bytes: &[u8]) -> Result<(MainHeader, usize)> {
    let mut cur = Cursor::new(bytes);

    if cur.u16()? != marker::SOC {
        return Err(Error::Codestream(
            "does not start with the SOC marker".into(),
        ));
    }
    // SIZ shall be the first marker segment after SOC (A.6).
    if cur.u16()? != marker::SIZ {
        return Err(Error::Codestream(
            "SIZ must be the first marker after SOC".into(),
        ));
    }
    let siz = decode_siz(segment(&mut cur)?)?;

    let mut cod = None;
    let mut qcd = None;

    let sot_offset = loop {
        let m = cur.u16()?;
        match m {
            // First SOT ends the main header; point the offset back at it.
            marker::SOT => break cur.pos - 2,

            marker::COD => {
                if cod.is_some() {
                    return Err(Error::Codestream("duplicate COD marker".into()));
                }
                cod = Some(decode_cod(segment(&mut cur)?)?);
            }
            marker::QCD => {
                if qcd.is_some() {
                    return Err(Error::Codestream("duplicate QCD marker".into()));
                }
                qcd = Some(decode_qcd(segment(&mut cur)?)?);
            }
            // Comment: recognised, length-skipped.
            marker::COM => {
                segment(&mut cur)?;
            }

            // Valid markers a later phase owns — reject cleanly, do not half-parse.
            marker::COC
            | marker::QCC
            | marker::RGN
            | marker::POC
            | marker::TLM
            | marker::PLT
            | marker::SOP
            | marker::EPH => {
                return Err(Error::Unsupported(format!(
                    "marker {m:#06X} is outside the Phase 1 subset"
                )));
            }

            // SOD/EOC have no place in a main header.
            marker::SOD | marker::EOC => {
                return Err(Error::Codestream(format!(
                    "unexpected marker {m:#06X} before any tile-part"
                )));
            }
            other => {
                return Err(Error::Codestream(format!(
                    "unknown marker {other:#06X} in main header"
                )));
            }
        }
    };

    let cod = cod.ok_or_else(|| Error::Codestream("missing required COD marker".into()))?;
    let qcd = qcd.ok_or_else(|| Error::Codestream("missing required QCD marker".into()))?;

    Ok((MainHeader { siz, cod, qcd }, sot_offset))
}

/// Upper bound on the declared image area (`Xsiz * Ysiz`), a robustness guard
/// against a malformed SIZ steering the per-subband and DWT buffers into an
/// overflowing or out-of-memory allocation. Sized for Phase 1 GRIB2 grids: 2^26
/// samples is 256 MiB as `i32`, well above operational grids (HRRR ~1.9M, MRMS
/// ~24.5M) and below anything that threatens the decode. Not a format limit —
/// raise it as later phases take larger imagery.
const MAX_IMAGE_SAMPLES: u64 = 1 << 26;

/// Enforce the Phase 1 geometry subset on the SIZ fields: a single tile at the
/// canvas origin, bounded in area. The general canvas (nonzero image/tile
/// offsets, a multi-tile grid) is valid JPEG 2000 but Phase 2+ work, so reject
/// it cleanly here rather than let an out-of-subset origin reach the DWT (whose
/// interleaving assumes even, canvas-anchored subband origins) or an unbounded
/// area reach the buffer allocations.
fn validate_geometry(siz: &Siz) -> Result<()> {
    if siz.x_size == 0 || siz.y_size == 0 {
        return Err(Error::Marker("SIZ declares a zero-size image".into()));
    }
    if siz.x_offset != 0 || siz.y_offset != 0 {
        return Err(Error::Unsupported(format!(
            "image offset ({}, {}); the Phase 1 subset is canvas-origin only",
            siz.x_offset, siz.y_offset
        )));
    }
    if siz.tile_x_offset != 0 || siz.tile_y_offset != 0 {
        return Err(Error::Unsupported(format!(
            "tile offset ({}, {}); the Phase 1 subset is canvas-origin only",
            siz.tile_x_offset, siz.tile_y_offset
        )));
    }
    if siz.tile_width == 0 || siz.tile_height == 0 {
        return Err(Error::Marker("SIZ declares a zero-size tile".into()));
    }
    // A single tile must span the whole image; a smaller tile means a multi-tile
    // grid, which is Phase 2.
    if (siz.tile_width as u64) < siz.x_size as u64 || (siz.tile_height as u64) < siz.y_size as u64 {
        return Err(Error::Unsupported(
            "tile smaller than the image (multi-tile grid); the Phase 1 subset is single-tile"
                .into(),
        ));
    }
    if siz.x_size as u64 * siz.y_size as u64 > MAX_IMAGE_SAMPLES {
        return Err(Error::Unsupported(format!(
            "image area {}×{} exceeds the Phase 1 decode guard of {MAX_IMAGE_SAMPLES} samples",
            siz.x_size, siz.y_size
        )));
    }
    Ok(())
}

/// Decode SIZ — image and tile geometry plus the per-component depth/sign
/// (A.5.1). Enforces the single-component subset.
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
    if csiz != 1 {
        return Err(Error::Unsupported(format!(
            "{csiz} components; the Phase 1 subset is single-component"
        )));
    }

    let mut components = Vec::with_capacity(csiz as usize);
    for _ in 0..csiz {
        let ssiz = b.u8()?;
        let x_sampling = b.u8()?;
        let y_sampling = b.u8()?;
        components.push(SizComponent {
            bit_depth: (ssiz & 0x7F) + 1,
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
    validate_geometry(&siz)?;
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
            "SOP/EPH error-resilience markers are out of the Phase 1 subset".into(),
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

    let mct = b.u8()?;
    if mct != 0 {
        return Err(Error::Unsupported(
            "multiple-component transform is out of the single-component subset".into(),
        ));
    }

    let decomposition_levels = b.u8()?;
    let code_block_width = b.u8()?;
    let code_block_height = b.u8()?;
    let code_block_style = b.u8()?;
    let transform = match b.u8()? {
        0 => Transform::Irreversible97,
        1 => Transform::Reversible53,
        other => return Err(Error::Marker(format!("reserved wavelet transform {other}"))),
    };
    b.expect_consumed("COD")?;

    Ok(Cod {
        progression,
        layers,
        decomposition_levels,
        code_block_width,
        code_block_height,
        code_block_style,
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
            if b.remaining() == 0 || b.remaining() % 2 != 0 {
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
