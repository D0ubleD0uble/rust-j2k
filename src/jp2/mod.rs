//! Stage 0 — the JP2 file format (ISO/IEC 15444-1 Annex I).
//!
//! A JP2 file is a sequence of boxes around one codestream:
//!
//! ```text
//! jP   signature (12 bytes, first)
//! ftyp file type (second): `jp2 ` as brand or in the compatibility list
//! jp2h header superbox: ihdr first, then colr, bpcc, pclr, cmap, cdef, res
//! jp2c the codestream (the first one; later ones are ignored, I.5.4)
//! ```
//!
//! Other boxes are skipped. [`parse`] walks the boxes up to the first `jp2c`
//! and hands back the codestream slice with the header boxes; the codestream
//! stage decodes the slice as it would a raw codestream. [`check_against_siz`]
//! then holds `ihdr` to the codestream's SIZ, and [`present`] builds the
//! [`Jp2Metadata`] that rides on the [`Image`](crate::Image).
//!
//! The walk stops at the first `jp2c`, as OpenJPEG's does, so nothing after
//! the codestream can fail a decode. Every length is checked before use and
//! nothing is allocated from one (see [`boxes`]). Box-layer faults are
//! [`Error::FileFormat`]; a file this decoder does not read (no `jp2 `
//! compatibility, or a box it cannot yet apply) is [`Error::Unsupported`].

mod boxes;
mod header;
mod metadata;

#[cfg(test)]
mod tests;

pub use metadata::{Association, ChannelDefinition, ChannelType, ColourSpace, Jp2Metadata};

use crate::codestream::markers::Siz;
use crate::{Component, Error, Result};
use boxes::{Boxes, Jp2Box};
use header::{Jp2Header, check_file_type};

/// The JP2 signature box (I.5.1): length 12, type `jP  `, contents
/// `0D 0A 87 0A`.
pub(crate) const SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// Whether `bytes` open with a box of the signature box's type. Such input is
/// read as a JP2 file, so a damaged signature box is reported as one
/// ([`Error::FileFormat`]) rather than as a codestream missing its `SOC`.
pub(crate) fn looks_like_jp2(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(&SIGNATURE[4..8])
}

/// A JP2 file's boxes up to and including its first `jp2c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Jp2File<'a> {
    /// The `jp2h` contents.
    pub(crate) header: Jp2Header<'a>,
    /// The first `jp2c` box's contents: the codestream.
    pub(crate) codestream: &'a [u8],
    /// The other top-level boxes before the codestream (`xml `, `uuid`, `rreq`
    /// and the like), in file order.
    pub(crate) other: Vec<Jp2Box<'a>>,
}

/// Walk a JP2 file's top-level boxes to the first `jp2c`.
pub(crate) fn parse(bytes: &[u8]) -> Result<Jp2File<'_>> {
    if !bytes.starts_with(&SIGNATURE) {
        return Err(Error::FileFormat(
            "the file must open with the 12-byte JP2 signature box (`jP  ` holding \
             0D 0A 87 0A)"
                .into(),
        ));
    }

    let mut boxes = Boxes::top_level(bytes, SIGNATURE.len());
    match boxes.next().transpose()? {
        Some(ftyp) if &ftyp.kind == b"ftyp" => check_file_type(&ftyp)?,
        Some(other) => {
            return Err(Error::FileFormat(format!(
                "the `ftyp` box must follow the signature, not `{}`",
                other.name()
            )));
        }
        None => {
            return Err(Error::FileFormat(
                "the file ends after the signature box".into(),
            ));
        }
    }

    let mut header = None;
    let mut other = Vec::new();
    for item in boxes {
        let item = item?;
        match &item.kind {
            b"jp2h" if header.is_some() => {
                return Err(Error::FileFormat(
                    "the file has a second `jp2h` box; it must have exactly one".into(),
                ));
            }
            b"jp2h" => header = Some(Jp2Header::parse(item.payload)?),
            b"jp2c" => {
                let Some(header) = header else {
                    return Err(Error::FileFormat(
                        "the `jp2c` codestream box comes before the `jp2h` header box".into(),
                    ));
                };
                return Ok(Jp2File {
                    header,
                    codestream: item.payload,
                    other,
                });
            }
            _ => other.push(item),
        }
    }
    Err(Error::FileFormat(match header {
        None => "the file has no `jp2h` header box and no `jp2c` codestream box".into(),
        Some(_) => "the file has no `jp2c` codestream box".into(),
    }))
}

/// Reject a file whose header holds a box the decoder cannot yet apply, so a
/// file is never decoded into channels it would present wrongly.
pub(crate) fn check_supported(header: &Jp2Header<'_>) -> Result<()> {
    let unsupported = |name: &str| {
        Err(Error::Unsupported(format!(
            "the `{name}` box in the JP2 header is not applied yet"
        )))
    };
    if !header.pclr.is_empty() {
        return unsupported("pclr");
    }
    if !header.cmap.is_empty() {
        return unsupported("cmap");
    }
    if !header.cdef.is_empty() {
        return unsupported("cdef");
    }
    Ok(())
}

/// Hold `ihdr` to the codestream: `HEIGHT` and `WIDTH` must equal the SIZ image
/// area and `NC` must equal `Csiz` (I.5.3.1). OpenJPEG checks the area but not
/// `NC`; palette and channel definitions index components by `NC`, so a file
/// whose header disagrees with its codestream is refused.
pub(crate) fn check_against_siz(header: &Jp2Header<'_>, siz: &Siz) -> Result<()> {
    let ihdr = &header.ihdr;
    let (width, height) = siz.image_extent_at(0);
    if (ihdr.width, ihdr.height) != (width, height) {
        return Err(Error::FileFormat(format!(
            "`ihdr` declares a {}x{} image but the codestream's SIZ declares {width}x{height}",
            ihdr.width, ihdr.height
        )));
    }
    if usize::from(ihdr.components) != siz.components.len() {
        return Err(Error::FileFormat(format!(
            "`ihdr` declares {} components but the codestream's SIZ declares {}",
            ihdr.components,
            siz.components.len()
        )));
    }
    Ok(())
}

/// Turn the decoded codestream components into the file's output channels and
/// describe them. Today the channels are the components as coded: no palette,
/// every channel colour `i + 1`, and the colour space not yet read.
pub(crate) fn present(
    _file: &Jp2File<'_>,
    components: Vec<Component>,
) -> Result<(Vec<Component>, Jp2Metadata)> {
    let colour = ColourSpace::Unknown;
    let channels = colour_channels(components.len())?;
    let metadata = Jp2Metadata {
        colour,
        channels,
        palette_applied: false,
    };
    Ok((components, metadata))
}

/// The channel definitions of a file without `cdef`: channel `i` is colour
/// `i + 1`.
fn colour_channels(count: usize) -> Result<Vec<ChannelDefinition>> {
    (0..count)
        .map(|i| {
            let colour = u16::try_from(i + 1).map_err(|_| {
                Error::Inconsistent(format!("{count} channels exceed the `u16` colour index"))
            })?;
            Ok(ChannelDefinition {
                kind: ChannelType::Colour,
                association: Association::Colour(colour),
            })
        })
        .collect()
}
