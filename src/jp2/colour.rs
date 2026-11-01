//! The Colour Specification box (I.5.3.3) and the Bits Per Component box
//! (I.5.3.2).
//!
//! Both are read the way OpenJPEG 2.5.0 reads them (`opj_jp2_read_colr`,
//! `opj_jp2_read_bpcc`), on accept and reject as well as on value, for the
//! boxes before the codestream. OpenJPEG reads the header boxes again after
//! `jp2c` and can reject these shapes there; this decoder does not yet:
//!
//! - Every `colr` before the codestream must hold at least 3 bytes, including
//!   the ones that are ignored, because the size check runs before the "already have a `colr`"
//!   check.
//! - The first `colr` with `METH` 1 or 2 is the one that counts. A box with
//!   any other `METH` is ignored whole and does not count as the first.
//! - `METH` 1 needs 7 bytes (`EnumCS` is 4); extra bytes are ignored.
//!   `METH` 2 keeps everything after the 3 header bytes as the profile, and an
//!   empty profile is refused.
//! - Every `bpcc` before the codestream must hold exactly `NC` bytes, whatever `ihdr.BPC` says; a
//!   `bpcc` that comes before `ihdr` sees `NC` as 0. The depths themselves are
//!   not used: SIZ is the truth for depth and sign.

use super::header::Jp2Header;
use super::metadata::ColourSpace;
use crate::{Error, Result};

/// `METH` for an enumerated colour space.
const ENUMERATED: u8 = 1;
/// `METH` for a restricted ICC profile.
const RESTRICTED_ICC: u8 = 2;

/// Check every `bpcc` against the component count. OpenJPEG checks a `bpcc`
/// as it reads it, against the `NC` of the `ihdr` read so far, which is still
/// 0 for a `bpcc` that comes before `ihdr`: such a box must be empty.
pub(crate) fn check_bpcc(header: &Jp2Header<'_>) -> Result<()> {
    for (at, child) in header.children.iter().enumerate() {
        if &child.kind != b"bpcc" {
            continue;
        }
        let before_ihdr = at < header.ihdr_at;
        let components = if before_ihdr {
            0
        } else {
            usize::from(header.ihdr.components)
        };
        if child.payload.len() != components {
            return Err(Error::FileFormat(format!(
                "a `bpcc` box holds {} bytes; it needs one per component ({components}{})",
                child.payload.len(),
                if before_ihdr {
                    ", as `ihdr` has not been read yet"
                } else {
                    ""
                }
            )));
        }
    }
    Ok(())
}

/// The colour space the file declares: the first usable `colr`, or
/// [`ColourSpace::Unknown`] if there is none. `ihdr.UnkC` is not consulted, as
/// in OpenJPEG: Kakadu writes `UnkC` 1 beside a `colr` it means (I.5.3.1 says
/// to treat the image as if the methods reproduce it accurately). Every `colr`
/// is still held to its size rules, so a malformed box is refused even when it
/// would not have been used.
pub(crate) fn read(header: &Jp2Header<'_>) -> Result<ColourSpace> {
    let mut first = None;
    for payload in header.colr() {
        if payload.len() < 3 {
            return Err(Error::FileFormat(format!(
                "a `colr` box holds {} bytes; it needs at least 3 (`METH`, `PREC`, `APPROX`)",
                payload.len()
            )));
        }
        if first.is_none() {
            first = parse_usable(payload)?;
        }
    }
    Ok(first.unwrap_or(ColourSpace::Unknown))
}

/// Read one `colr` of at least 3 bytes. `None` for a `METH` that is neither 1
/// nor 2: the box is ignored and does not count as the first.
fn parse_usable(payload: &[u8]) -> Result<Option<ColourSpace>> {
    let method = payload[0];
    let rest = &payload[3..];
    match method {
        ENUMERATED => {
            let Some(&[a, b, c, d]) = rest.first_chunk::<4>() else {
                return Err(Error::FileFormat(format!(
                    "a `colr` box with `METH` 1 holds {} bytes; it needs 7 for the \
                     enumerated colour space",
                    payload.len()
                )));
            };
            Ok(Some(match u32::from_be_bytes([a, b, c, d]) {
                16 => ColourSpace::Srgb,
                17 => ColourSpace::Greyscale,
                18 => ColourSpace::Sycc,
                other => ColourSpace::Enumerated(other),
            }))
        }
        RESTRICTED_ICC => {
            if rest.is_empty() {
                return Err(Error::FileFormat(
                    "a `colr` box with `METH` 2 holds no ICC profile".into(),
                ));
            }
            Ok(Some(ColourSpace::RestrictedIcc(rest.to_vec())))
        }
        _ => Ok(None),
    }
}
