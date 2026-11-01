//! Stage 0 — the JP2 file format (ISO/IEC 15444-1 Annex I).
//!
//! A JP2 file is a sequence of boxes around one codestream:
//!
//! ```text
//! jP   signature (12 bytes, first)
//! ftyp file type (second): `jp2 ` as brand or in the compatibility list
//! jp2h header superbox: ihdr, colr, bpcc, pclr, cmap, cdef, res
//! jp2c the codestream (the first one; later ones are ignored, I.5.4)
//! ```
//!
//! Other boxes are skipped. [`parse`] walks the boxes up to the first `jp2c`
//! and hands back the codestream slice with the header boxes; the codestream
//! stage decodes the slice as it would a raw codestream. [`check_against_siz`]
//! then holds `ihdr` to the codestream's SIZ, [`check_supported`] refuses the
//! header boxes not applied yet, and [`present`] builds the [`Jp2Metadata`]
//! that rides on the [`Image`](crate::Image).
//!
//! For these ways a file can bend Annex I, the walk does what OpenJPEG 2.5.0
//! does: `ihdr` may sit anywhere in `jp2h`, a second `jp2h` before the
//! codestream is merged into the first, a header box misplaced at the top
//! level after `jp2h` is applied as if it were inside, and a `jp2c` that runs
//! past the end of the file is clamped to it. The walk stops at the first
//! `jp2c`; after it, only the two length faults OpenJPEG rejects there are
//! checked ([`boxes::check_after_codestream`]). The two decoders still differ
//! on some files, in both directions; `docs/correctness.md` and the reject
//! matrix in `docs/roadmap-phase3.md` list where.
//!
//! Every length is checked before use and nothing is allocated from one (see
//! [`boxes`]). Box-layer faults are [`Error::FileFormat`]; a file this decoder
//! does not read (no `jp2 ` compatibility, or a box it cannot yet apply) is
//! [`Error::Unsupported`].

mod boxes;
mod colour;
mod header;
mod metadata;

#[cfg(test)]
mod colour_tests;
#[cfg(test)]
mod tests;

pub use metadata::{Association, ChannelDefinition, ChannelType, ColourSpace, Jp2Metadata};

use crate::codestream::markers::Siz;
use crate::{Component, Error, Result};
use boxes::{Boxes, Jp2Box, check_after_codestream};
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
    /// The `jp2h` contents, merged over every `jp2h` box and every header box
    /// misplaced at the top level after the first `jp2h`.
    pub(crate) header: Jp2Header<'a>,
    /// The colour space the header declares (see [`colour::read`]).
    pub(crate) colour: ColourSpace,
    /// The first `jp2c` box's contents: the codestream.
    pub(crate) codestream: &'a [u8],
    /// The other top-level boxes before the codestream (`xml `, `uuid`, `rreq`
    /// and the like, and header boxes misplaced before `jp2h`, which OpenJPEG
    /// ignores), in file order.
    pub(crate) other: Vec<Jp2Box<'a>>,
    /// Everything after the first `jp2c` box. Only checked for the length
    /// faults OpenJPEG rejects there; its contents are not read.
    pub(crate) after_codestream: &'a [u8],
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

    let mut header: Option<Jp2Header<'_>> = None;
    let mut other = Vec::new();
    // Whether a box with length 0 ("to the end of the file") was read, for the
    // message when it swallowed the boxes meant to follow it.
    let mut open_ended = false;
    while let Some(item) = boxes.next() {
        let item = item?;
        open_ended = boxes.last_was_open_ended();
        match &item.kind {
            b"jp2h" => {
                header = Some(match header {
                    None => Jp2Header::parse(item.payload)?,
                    Some(first) => first.merge(item.payload)?,
                });
            }
            b"jp2c" => {
                let Some(header) = header else {
                    return Err(Error::FileFormat(
                        "the file has no `jp2h` header box before its `jp2c` codestream box".into(),
                    ));
                };
                let after_codestream = boxes.rest();
                check_after_codestream(after_codestream)?;
                colour::check_bpcc(&header)?;
                let colour = colour::read(&header)?;
                return Ok(Jp2File {
                    header,
                    colour,
                    codestream: item.payload,
                    other,
                    after_codestream,
                });
            }
            _ => {
                let taken = header
                    .as_mut()
                    .is_some_and(|header| header.add_misplaced(item));
                if !taken {
                    other.push(item);
                }
            }
        }
    }
    let missing = match header {
        None => "the file has no `jp2h` header box and no `jp2c` codestream box",
        Some(_) => "the file has no `jp2c` codestream box",
    };
    Err(Error::FileFormat(if open_ended {
        format!("{missing}: a box of length 0 (to the end of the file) swallowed the rest")
    } else {
        missing.into()
    }))
}

/// Reject a file with a header box the decoder cannot yet apply, so a file is
/// never decoded into channels it would present wrongly. `header` holds the
/// boxes inside every `jp2h` and those misplaced at the top level after it, so
/// this one check covers both places OpenJPEG applies them from.
pub(crate) fn check_supported(header: &Jp2Header<'_>) -> Result<()> {
    let unsupported = |name: &str| {
        Err(Error::Unsupported(format!(
            "the file has a `{name}` box, which this decoder does not apply"
        )))
    };
    if !header.pclr().is_empty() {
        return unsupported("pclr");
    }
    if !header.cmap().is_empty() {
        return unsupported("cmap");
    }
    if !header.cdef().is_empty() {
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
/// describe them. The channels are the components as coded: no palette and
/// every channel colour `i + 1`.
pub(crate) fn present(
    file: &Jp2File<'_>,
    components: Vec<Component>,
) -> Result<(Vec<Component>, Jp2Metadata)> {
    let colour = file.colour.clone();
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
