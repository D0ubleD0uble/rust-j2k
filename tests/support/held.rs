//! Whether the input an incremental decoder holds an error on can still
//! change it. The one-shot decode parses every tile-part before it decodes
//! a tile, so an error met after that parse gives way to a fault in a later
//! tile-part. `refine` waits on such an error (`Waiting::Held`) and
//! `header` gives the header, until the input closes: a codestream whose
//! chain of `SOT`s steps by nonzero `Psot`s to an `EOC` the input holds.
//!
//! In a JP2 file, boxes can follow the codestream and the decode checks
//! them first, so the file closes only once its codestream closes and no
//! box after it can be read, whatever bytes follow. This file finds that
//! from the bytes, as OpenJPEG 2.5.0 reads them, and not through the
//! decoder: OpenJPEG decodes the codestream from the start of the first
//! `jp2c` box's payload, whatever that box's length, and then walks the
//! boxes from just after the `EOC`. That walk ends for good at a `jp2c`
//! box, or at an `XLBox` of 2^32 or more. Anything else it can still read
//! more of: fewer than 8 bytes, a box the input ends inside, or a box of
//! length 0, which runs to the end of the file.
//!
//! This file has no dependency but `rust_j2k`. It does not say which errors
//! are held: an oracle allows a held error only where the input has not
//! closed, and checks it against the error the one-shot call gives.

#![allow(dead_code)]

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The JP2 signature box, which a JP2 file starts with.
const JP2_SIGNATURE: [u8; 12] = [0, 0, 0, 12, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// Whether `bytes` close: a codestream (it starts with `SOC`) whose chain of
/// `SOT`s, from the first, steps by nonzero `Psot`s to an `EOC` the input
/// holds, or a JP2 file whose codestream closes so and after which no box
/// can be read ([`jp2_closes`]). Anything else does not.
pub fn closes(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0x4F]) {
        return codestream_end(bytes).is_some();
    }
    jp2_closes(bytes)
}

/// Where the codestream at the start of `bytes` closes, just past its
/// `EOC`: its chain of `SOT`s, from the first, steps by nonzero `Psot`s to
/// an `EOC` the input holds.
fn codestream_end(bytes: &[u8]) -> Option<usize> {
    if !bytes.starts_with(&[0xFF, 0x4F]) {
        return None;
    }
    // The main header's markers, up to the first `SOT`. Each has a length
    // and a segment, but those ISO/IEC 15444-1 gives none (A.1.3): the
    // reserved `0xFF30` to `0xFF3F` and `EPH`, two bytes each. `SOC`, `SOD`
    // and `EOC` have none either, and no main header holds them.
    let mut at = 2usize;
    loop {
        match u16_at(bytes, at)? {
            0xFF90 => break,
            0xFF4F | 0xFF93 | 0xFFD9 => return None,
            0xFF30..=0xFF3F | 0xFF92 => at += 2,
            _ => at += 2 + usize::from(u16_at(bytes, at + 2)?),
        }
    }
    loop {
        if u16_at(bytes, at) != Some(0xFF90) {
            return None;
        }
        match u32_at(bytes, at + 6)? {
            0 => return None,
            psot => at = at.saturating_add(psot as usize),
        }
        match u16_at(bytes, at)? {
            0xFFD9 => return Some(at + 2),
            0xFF90 => {}
            _ => return None,
        }
    }
}

/// Whether the JP2 file `bytes` closes: its codestream, from the payload of
/// its first top-level `jp2c` box, closes ([`codestream_end`]), and the
/// walk of the boxes after it stops for good on bytes the input holds.
fn jp2_closes(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&JP2_SIGNATURE) {
        return false;
    }
    let Some(start) = jp2c_payload(bytes) else {
        return false;
    };
    let Some(end) = codestream_end(&bytes[start..]) else {
        return false;
    };
    walk_ends(&bytes[start + end..])
}

/// Where the payload of the first top-level `jp2c` box of `bytes` starts,
/// if the input holds every box before it and that box's header.
fn jp2c_payload(bytes: &[u8]) -> Option<usize> {
    let mut at = 0usize;
    loop {
        let lbox = u32_at(bytes, at)?;
        // OpenJPEG reads a `jp2c` box from its payload whatever its length,
        // 0 (to the end of the file) included.
        if bytes.get(at + 4..at + 8)? == b"jp2c" {
            let header = if lbox == 1 { 16 } else { 8 };
            return Some(at + header).filter(|&start| start <= bytes.len());
        }
        let (length, header) = match lbox {
            // A box of length 0 runs to the end of the file: nothing follows.
            0 => return None,
            1 => match (u32_at(bytes, at + 8)?, u32_at(bytes, at + 12)?) {
                (0, low) => (low as usize, 16),
                _ => return None,
            },
            n => (n as usize, 8),
        };
        if length < header {
            return None;
        }
        at = at.checked_add(length)?;
    }
}

/// Whether OpenJPEG's walk of the boxes in `rest`, the bytes after the
/// codestream, stops for good, whatever bytes follow: at a `jp2c` box's
/// header, or at an `XLBox` whose high half holds a nonzero byte. It stops
/// too at a box shorter than its own header, a fault, but then more
/// bytes cannot change it, and the decode gives that fault before the
/// tile-parts' errors; that is left as not closed.
fn walk_ends(mut rest: &[u8]) -> bool {
    loop {
        let Some(lbox) = u32_at(rest, 0) else {
            return false;
        };
        let Some(kind) = rest.get(4..8) else {
            return false;
        };
        if kind == b"jp2c" {
            return true;
        }
        let (length, header) = match lbox {
            0 => return false,
            1 => {
                let high = rest.get(8..rest.len().min(12)).unwrap_or_default();
                if high.iter().any(|&b| b != 0) {
                    return true;
                }
                match u32_at(rest, 12) {
                    Some(low) => (low as usize, 16),
                    None => return false,
                }
            }
            n => (n as usize, 8),
        };
        if length < header {
            return false;
        }
        match rest.get(length..) {
            Some(after) => rest = after,
            None => return false,
        }
    }
}
