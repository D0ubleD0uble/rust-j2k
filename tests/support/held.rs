//! Whether the input an incremental decoder holds an error on can still
//! change it. The one-shot decode parses every tile-part before it decodes
//! a tile, so an error met after that parse gives way to a fault in a later
//! tile-part. `refine` waits on such an error (`Waiting::TileError`) and
//! `header` gives the header, until the input closes: a codestream whose
//! chain of `SOT`s steps by nonzero `Psot`s to an `EOC` the input holds. A
//! JP2 file never closes, as boxes can follow the codestream and the decode
//! checks them first.
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

/// Whether `bytes` close: a codestream (it starts with `SOC`) whose chain of
/// `SOT`s, from the first, steps by nonzero `Psot`s to an `EOC` the input
/// holds. Anything else, a JP2 file included, does not.
pub fn closes(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xFF, 0x4F]) {
        return false;
    }
    // The main header's segments, up to the first `SOT`.
    let mut at = 2usize;
    loop {
        match u16_at(bytes, at) {
            Some(0xFF90) => break,
            Some(_) => match u16_at(bytes, at + 2) {
                Some(length) => at += 2 + usize::from(length),
                None => return false,
            },
            None => return false,
        }
    }
    loop {
        if u16_at(bytes, at) != Some(0xFF90) {
            return false;
        }
        match u32_at(bytes, at + 6) {
            Some(0) | None => return false,
            Some(psot) => at = at.saturating_add(psot as usize),
        }
        match u16_at(bytes, at) {
            Some(0xFFD9) => return true,
            Some(0xFF90) => {}
            _ => return false,
        }
    }
}
