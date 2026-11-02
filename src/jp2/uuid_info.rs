//! The UUID info box (`uinf`, ISO/IEC 15444-1 I.7.3): a superbox holding a
//! UUID list box (`ulst`, I.7.3.1) and a data entry URL box (`url `,
//! I.7.3.2), in that order (Figure I.20).
//!
//! OpenJPEG skips `uinf`, so the standard is the reference. The checks:
//!
//! - exactly one `ulst` and exactly one `url `, the `ulst` first; any other
//!   box inside is skipped (I.8);
//! - `ulst` holds its 2-byte count `NU` and exactly `NU` 16-byte UUIDs
//!   ("The number of UUIDi fields shall be the same as the value of the NU
//!   field");
//! - `url ` holds at least its 1-byte `VERS` and 3-byte `FLAG`.
//!
//! The standard also requires `VERS` and `FLAG` to be 0 and `LOC` to be a
//! NUL-terminated UTF-8 string. Those are reported as stored, not checked.
//! Each fault is an [`Error::FileFormat`]; `docs/correctness.md` records the
//! choices.

use super::UuidInfo;
use super::boxes::{Boxes, Jp2Box};
use crate::{Error, Result};

/// Read a `uinf` box's contents.
pub(crate) fn parse(payload: &[u8]) -> Result<UuidInfo> {
    let mut ulst: Option<Jp2Box<'_>> = None;
    let mut url: Option<Jp2Box<'_>> = None;
    for child in Boxes::children(payload, "a `uinf` box") {
        let child = child?;
        match &child.kind {
            b"ulst" if ulst.is_some() => {
                return Err(fault("holds more than one `ulst` box"));
            }
            b"ulst" if url.is_some() => {
                return Err(fault("has its `url ` box before its `ulst` box"));
            }
            b"ulst" => ulst = Some(child),
            b"url " if url.is_some() => {
                return Err(fault("holds more than one `url ` box"));
            }
            b"url " => url = Some(child),
            _ => {}
        }
    }
    let Some(ulst) = ulst else {
        return Err(fault("has no `ulst` box"));
    };
    let Some(url) = url else {
        return Err(fault("has no `url ` box"));
    };
    let ids = parse_ulst(ulst.payload)?;
    let [version, f0, f1, f2, location @ ..] = url.payload else {
        return Err(fault(&format!(
            "has a `url ` box of {} bytes; it needs at least 4 (VERS and FLAG)",
            url.payload.len()
        )));
    };
    Ok(UuidInfo {
        ids,
        version: *version,
        flags: [*f0, *f1, *f2],
        location: location.to_vec(),
    })
}

/// Read a `ulst` box's contents: `NU`, then exactly `NU` UUIDs.
fn parse_ulst(payload: &[u8]) -> Result<Vec<[u8; 16]>> {
    let Some((count, ids)) = payload.split_first_chunk::<2>() else {
        return Err(fault(&format!(
            "has a `ulst` box of {} bytes, too short for its 2-byte count",
            payload.len()
        )));
    };
    let count = usize::from(u16::from_be_bytes(*count));
    // `count * 16` is at most 2^20, so the product cannot overflow.
    if ids.len() != count * 16 {
        return Err(fault(&format!(
            "has a `ulst` box that counts {count} UUIDs ({} bytes) but holds {} bytes of them",
            count * 16,
            ids.len()
        )));
    }
    // The count was checked against the bytes, so the list is bounded by the
    // input.
    Ok(ids
        .chunks_exact(16)
        .map(|id| {
            let mut out = [0; 16];
            out.copy_from_slice(id);
            out
        })
        .collect())
}

/// A malformed `uinf` box.
fn fault(what: &str) -> Error {
    Error::FileFormat(format!("a `uinf` box {what}"))
}
