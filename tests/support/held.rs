//! When an incremental decoder holds a tile refusal: the one-shot decode or
//! header read with `strict` off refuses a tile, by the reduction or a tile
//! cap more bytes do not lift, and the input does not yet close its
//! tile-parts with an `EOC`. A later tile-part could still hold a fault the
//! one-shot call gives first, so `refine` waits on the refusal
//! (`Waiting::TileRefused`) and `header` gives the header.
//!
//! This file has no dependency but `rust_j2k`.

#![allow(dead_code)]

use rust_j2k::{Error, LimitKind, Raise};

/// Whether `error`, from a one-shot call with `strict` off on `prefix`, is a
/// refusal an incremental decoder fed `prefix` holds.
pub fn held(error: &Error, prefix: &[u8]) -> bool {
    tile_refusal(error) && !closes(prefix)
}

/// Whether `error` refuses a tile in a way more bytes do not lift: a
/// reduction past the tile's levels, or a tile cap with no room left (under
/// `AtMost`, or past the address space).
pub fn tile_refusal(error: &Error) -> bool {
    match error {
        Error::InvalidOptions(detail) => detail.message().contains(" in tile "),
        Error::Limit(limit) => {
            let lifts = matches!(
                limit.kind(),
                LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts
            ) && matches!(
                limit.retry_with(),
                Some(Raise::Raised(_) | Raise::Unlimited)
            );
            !lifts && limit.message().starts_with("tile ") && limit.message().contains(" holds ")
        }
        _ => false,
    }
}

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

/// The codestream of `bytes`: all of it when it starts with `SOC`, else the
/// payload of the first top-level `jp2c` box, as far as the input holds it.
fn codestream(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.starts_with(&[0xFF, 0x4F]) {
        return Some(bytes);
    }
    let mut at = 0usize;
    loop {
        let length = u32_at(bytes, at)?;
        let kind = bytes.get(at + 4..at + 8)?;
        let (header, end) = match length {
            0 => (8, bytes.len()),
            1 => {
                let long =
                    u64::from(u32_at(bytes, at + 8)?) << 32 | u64::from(u32_at(bytes, at + 12)?);
                (16, at.saturating_add(usize::try_from(long).ok()?))
            }
            n => (8, at.saturating_add(n as usize)),
        };
        if kind == b"jp2c" {
            return bytes.get(at + header..end.min(bytes.len()));
        }
        if end <= at || end >= bytes.len() {
            return None;
        }
        at = end;
    }
}

/// Whether the tile-parts of `bytes` close: the chain of `SOT`s, from the
/// first, steps by nonzero `Psot`s to an `EOC` the input holds.
pub fn closes(bytes: &[u8]) -> bool {
    let Some(cs) = codestream(bytes) else {
        return false;
    };
    // The main header's segments, up to the first `SOT`.
    let mut at = 2usize;
    loop {
        match u16_at(cs, at) {
            Some(0xFF90) => break,
            Some(_) => match u16_at(cs, at + 2) {
                Some(length) => at += 2 + usize::from(length),
                None => return false,
            },
            None => return false,
        }
    }
    loop {
        if u16_at(cs, at) != Some(0xFF90) {
            return false;
        }
        match u32_at(cs, at + 6) {
            Some(0) | None => return false,
            Some(psot) => at = at.saturating_add(psot as usize),
        }
        match u16_at(cs, at) {
            Some(0xFFD9) => return true,
            Some(0xFF90) => {}
            _ => return false,
        }
    }
}
