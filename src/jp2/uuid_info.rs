//! The UUID info box (`uinf`, ISO/IEC 15444-1 I.7.3): a superbox holding a
//! UUID list box (`ulst`, I.7.3.1) and a data entry URL box (`url `,
//! I.7.3.2).
//!
//! OpenJPEG skips `uinf`, so a malformed one never fails a decode there. Here
//! it is read when it is well formed and skipped as a whole when it is not:
//! no partial [`UuidInfo`] is reported. Well formed means:
//!
//! - its children are whole boxes filling it exactly (each length checked as
//!   inside any superbox, see [`Boxes`]);
//! - exactly one `ulst` and exactly one `url `, in either order; any other
//!   box inside is skipped (I.8);
//! - `ulst` holds its 2-byte count `NU` and exactly `NU` 16-byte UUIDs
//!   ("The number of UUIDi fields shall be the same as the value of the NU
//!   field");
//! - `url ` holds at least its 1-byte `VERS` and 3-byte `FLAG`.
//!
//! The standard also requires `VERS` and `FLAG` to be 0 and `LOC` to be a
//! NUL-terminated UTF-8 string. Those are reported as stored, not checked.
//! `docs/correctness.md` records the choices.

use super::UuidInfo;
use super::boxes::{Boxes, Jp2Box};

/// Read a `uinf` box's contents, or `None` when it is malformed.
pub(crate) fn parse(payload: &[u8]) -> Option<UuidInfo> {
    let mut ulst: Option<Jp2Box<'_>> = None;
    let mut url: Option<Jp2Box<'_>> = None;
    for child in Boxes::children(payload, "a `uinf` box") {
        let child = child.ok()?;
        let slot = match &child.kind {
            b"ulst" => &mut ulst,
            b"url " => &mut url,
            _ => continue,
        };
        if slot.replace(child).is_some() {
            return None;
        }
    }
    let ids = parse_ulst(ulst?.payload)?;
    let [version, f0, f1, f2, location @ ..] = url?.payload else {
        return None;
    };
    Some(UuidInfo {
        ids,
        version: *version,
        flags: [*f0, *f1, *f2],
        location: location.to_vec(),
    })
}

/// Read a `ulst` box's contents: `NU`, then exactly `NU` UUIDs.
fn parse_ulst(payload: &[u8]) -> Option<Vec<[u8; 16]>> {
    let (count, ids) = payload.split_first_chunk::<2>()?;
    // `NU * 16` is at most 2^20, so the product cannot overflow.
    if ids.len() != usize::from(u16::from_be_bytes(*count)) * 16 {
        return None;
    }
    // The count was checked against the bytes, so the list is bounded by the
    // input.
    Some(
        ids.chunks_exact(16)
            .map(|id| {
                let mut out = [0; 16];
                out.copy_from_slice(id);
                out
            })
            .collect(),
    )
}
