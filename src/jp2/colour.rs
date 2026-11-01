//! The Colour Specification box (I.5.3.3) and the Bits Per Component box
//! (I.5.3.2).
//!
//! Both are read the way OpenJPEG 2.5.0 reads them (`opj_jp2_read_colr`,
//! `opj_jp2_read_bpcc`), on accept and reject as well as on value:
//!
//! - Every `colr` must hold at least 3 bytes, including the ones that are
//!   ignored, because the size check runs before the "already have a `colr`"
//!   check.
//! - The first `colr` with `METH` 1 or 2 is the one that counts. A box with
//!   any other `METH` is ignored whole and does not count as the first.
//! - `METH` 1 needs 7 bytes (`EnumCS` is 4); extra bytes are ignored.
//!   `METH` 2 keeps everything after the 3 header bytes as the profile, and an
//!   empty profile is refused.
//! - Every `bpcc` must hold exactly `NC` bytes, whatever `ihdr.BPC` says. The
//!   depths themselves are not used: SIZ is the truth for depth and sign.

use super::header::Jp2Header;
use super::metadata::ColourSpace;
use crate::{Error, Result};

/// `METH` for an enumerated colour space.
const ENUMERATED: u8 = 1;
/// `METH` for a restricted ICC profile.
const RESTRICTED_ICC: u8 = 2;

/// Check every `bpcc` against `ihdr.NC`.
pub(crate) fn check_bpcc(header: &Jp2Header<'_>) -> Result<()> {
    let components = usize::from(header.ihdr.components);
    for bpcc in &header.bpcc {
        if bpcc.len() != components {
            return Err(Error::FileFormat(format!(
                "a `bpcc` box holds {} bytes; it needs one per component ({components})",
                bpcc.len()
            )));
        }
    }
    Ok(())
}

/// The colour space the file declares: the first usable `colr`, or
/// [`ColourSpace::Unknown`] if there is none or `ihdr.UnkC` is 1. Every `colr`
/// is still held to its size rules, so a malformed box is refused even when it
/// would not have been used.
pub(crate) fn read(header: &Jp2Header<'_>) -> Result<ColourSpace> {
    let mut first = None;
    for payload in &header.colr {
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
    if header.ihdr.unknown_colourspace == 1 {
        return Ok(ColourSpace::Unknown);
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
