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
//! Other boxes are skipped without being descended into: `uinf`, `jp2i`, the
//! JPX boxes (`rreq`, `jpch`, `jplh`, `asoc`, `lbl `, `ftbl`) and anything
//! unknown. Only `jp2h` and `res ` are entered. The top-level `xml ` and
//! `uuid` boxes are passed through verbatim, including those after the
//! codestream.
//!
//! [`parse`] walks the boxes up to the first `jp2c` and hands back the
//! codestream slice with the header boxes. The rest runs before any tile data
//! is read, so `read_header` shares it with `decode`: the codestream's main
//! header is parsed, [`check_against_siz`] holds `ihdr` to its SIZ,
//! [`ChannelPlans::plan`] plans the output channels (the palette, see
//! [`palette`], then the channel definitions, see [`channels`]), and
//! [`metadata()`] builds the [`Jp2Metadata`] that rides on the
//! [`Image`](crate::Image). When the caller asks for sRGB output,
//! [`plan_srgb`] plans that conversion too (see [`srgb`]). Only then are the
//! tiles decoded and the plans applied to their samples.
//!
//! For these ways a file can bend Annex I, the walk does what OpenJPEG 2.5.0
//! does: `ihdr` may sit anywhere in `jp2h`, a second `jp2h` before the
//! codestream is merged into the first, a header box misplaced at the top
//! level after `jp2h` is applied as if it were inside, and a `jp2c` that runs
//! past the end of the file is clamped to it. The walk stops at the first
//! `jp2c`. OpenJPEG does not use that box's length: the codestream ends where
//! its tile-parts end ([`after::codestream_end`]), and the boxes from there on
//! are walked and read as OpenJPEG reads them once it has decoded the
//! codestream ([`boxes::walk_after_codestream`], [`after::check`]). The two
//! decoders still differ on some files, in both directions;
//! `docs/correctness.md` and the reject matrix in `docs/roadmap-phase3.md`
//! list where.
//!
//! Every length is checked before use and nothing is allocated from one (see
//! [`boxes`]). Box-layer faults are [`Error::FileFormat`]; a file this decoder
//! does not read (no `jp2 ` compatibility, or a palette mapping or column depth
//! it does not decode) is [`Error::Unsupported`].

mod after;
mod boxes;
mod channels;
mod colour;
mod header;
mod lcms;
mod metadata;
mod palette;
mod resolution;
mod srgb;

#[cfg(test)]
mod colour_tests;
#[cfg(test)]
mod srgb_tests;
#[cfg(test)]
mod tests;

pub(crate) use after::check as check_after_codestream;
pub use metadata::{
    Association, ChannelDefinition, ChannelType, ColourSpace, Jp2Metadata, Ratio, Res, Resolution,
    UuidBox,
};

use crate::codestream::markers::Siz;
use crate::{Component, ComponentInfo, Error, Result};
use after::End;
use boxes::{Boxes, Jp2Box, walk_after_codestream};
use channels::ChannelPlan;
use header::{Jp2Header, check_file_type};
use palette::PalettePlan;
pub(crate) use srgb::SrgbPlan;

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
    /// The codestream: from the start of the first `jp2c` box's contents to
    /// the end of its last tile-part, or the box's contents when that end
    /// cannot be found from the tile-part lengths (see [`after::codestream_end`]).
    pub(crate) codestream: &'a [u8],
    /// The other top-level boxes before the codestream (`xml `, `uuid`, `rreq`
    /// and the like, and header boxes misplaced before `jp2h`, which OpenJPEG
    /// ignores), in file order.
    pub(crate) other: Vec<Jp2Box<'a>>,
    /// The whole boxes after the codestream, from just after its last
    /// tile-part, as far as OpenJPEG's walk there follows them (see
    /// [`walk_after_codestream`]). When [`Jp2File::after_read`] is set,
    /// [`after::check`] reads the header boxes among them.
    pub(crate) after_codestream: Vec<Jp2Box<'a>>,
    /// The whole boxes after both the codestream and the `jp2c` box, walked
    /// the same way. Their `xml ` and `uuid` boxes are passed through. This
    /// differs from `after_codestream` when the box is longer than its
    /// codestream: padding inside the box is skipped by its length here.
    pub(crate) after_box: Vec<Jp2Box<'a>>,
    /// Whether OpenJPEG reads the boxes after the codestream: true when the
    /// codestream's end was found from its tile-part lengths. When the last
    /// tile-part runs to the end of the file, OpenJPEG's walk starts at the
    /// end and finds nothing; when the end cannot be found, the codestream
    /// does not decode.
    pub(crate) after_read: bool,
    /// The length fault that ended the walk after the codestream, reported by
    /// [`after::check`] after the boxes before it.
    pub(crate) after_fault: Option<Error>,
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
                colour::check_bpcc(&header)?;
                let colour = colour::read(&header)?;
                let (codestream, rest, after_read) =
                    locate_codestream(bytes, item.payload, boxes.rest());
                let after = walk_after_codestream(rest);
                // Both are ends of the file; the shorter starts later.
                let past_box = boxes.rest();
                let after_box = if past_box.len() < rest.len() {
                    walk_after_codestream(past_box).boxes
                } else {
                    after.boxes.clone()
                };
                return Ok(Jp2File {
                    header,
                    colour,
                    codestream,
                    other,
                    after_codestream: after.boxes,
                    after_box,
                    after_read,
                    after_fault: after.fault,
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

/// Find the codestream that starts with the `jp2c` box's contents `payload`,
/// the box being followed by `rest` in `file`. Returns the codestream, the
/// bytes after it, and whether OpenJPEG reads those bytes as boxes.
///
/// OpenJPEG reads the codestream from the start of the box to wherever its
/// tile-parts end, whatever the box's length says. When every tile-part has a
/// length, that is just after `EOC`, and the boxes from there on are walked.
/// A last tile-part of `Psot = 0` runs to the end of the file and nothing
/// after it is walked. The box's contents are kept when they end in `EOC`:
/// anything after them would be tile data past the last packet, which
/// OpenJPEG ignores. Otherwise the codestream runs to the end of the file.
/// When the tile-part chain is broken, the box's contents go to the
/// codestream parser to be rejected (or to `read_header`, which reads only
/// the main header).
fn locate_codestream<'a>(
    file: &'a [u8],
    payload: &'a [u8],
    rest: &'a [u8],
) -> (&'a [u8], &'a [u8], bool) {
    // `payload` and `rest` are the end of `file`, back to back.
    let tail = file
        .len()
        .checked_sub(rest.len() + payload.len())
        .and_then(|start| file.get(start..));
    let Some(tail) = tail else {
        return (payload, rest, false);
    };
    match after::codestream_end(tail) {
        End::At(end) => (&tail[..end], &tail[end..], true),
        End::OpenLastPart if !payload.ends_with(&[0xFF, 0xD9]) => (tail, &[], false),
        End::OpenLastPart | End::Unknown => (payload, rest, false),
    }
}

/// Hold `ihdr` to the codestream: `HEIGHT` and `WIDTH` must equal the SIZ image
/// area (I.5.3.1), as OpenJPEG requires. `NC` is not held to `Csiz`: OpenJPEG
/// lets the codestream decide the image, and the palette's `cmap` and the
/// channel definitions are checked against the codestream's components.
pub(crate) fn check_against_siz(header: &Jp2Header<'_>, siz: &Siz) -> Result<()> {
    let ihdr = &header.ihdr;
    let (width, height) = siz.image_extent_at(0);
    if (ihdr.width, ihdr.height) != (width, height) {
        return Err(Error::FileFormat(format!(
            "`ihdr` declares a {}x{} image but the codestream's SIZ declares {width}x{height}",
            ihdr.width, ihdr.height
        )));
    }
    Ok(())
}

/// What the header boxes do to a file's channels, worked out from the boxes
/// and the codestream's main header before any tile is decoded: the palette,
/// then the channel definitions over the palette's output channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChannelPlans {
    palette: PalettePlan,
    channels: ChannelPlan,
}

impl ChannelPlans {
    /// Plan the channels for a decode at `reduction` resolution levels down.
    /// The palette is planned first, so a malformed palette reports its fault
    /// before a malformed `cdef` does, as OpenJPEG reads them.
    pub(crate) fn plan(
        header: &Jp2Header<'_>,
        siz: &Siz,
        reduction: u8,
        raw_channels: bool,
    ) -> Result<Self> {
        let palette = palette::plan(header, siz, reduction, raw_channels)?;
        let count = palette.output_channels(siz).len();
        let channels = channels::plan(header, count, raw_channels)?;
        Ok(ChannelPlans { palette, channels })
    }

    /// The output channels' formats, in output order: the palette's output,
    /// then placed by the channel definitions.
    pub(crate) fn outputs(&self, siz: &Siz) -> Result<Vec<ComponentInfo>> {
        self.channels.apply(self.palette.output_channels(siz))
    }

    /// Build the output channels from the decoded components.
    pub(crate) fn apply(&self, components: Vec<Component>) -> Result<Vec<Component>> {
        self.channels.apply(self.palette.apply(components)?)
    }
}

/// The file's metadata, from its boxes and the channel plans alone: no
/// sample is needed. Every channel starts as colour `i + 1`; the palette and
/// channel plans relabel them. `siz` is the codestream's.
pub(crate) fn metadata(file: &Jp2File<'_>, plans: &ChannelPlans, siz: &Siz) -> Result<Jp2Metadata> {
    let (xml, uuid) = passthrough(file);
    let mut metadata = Jp2Metadata {
        colour: file.colour.clone(),
        channels: colour_channels(plans.outputs(siz)?.len())?,
        palette_applied: false,
        resolution: resolution::read(&file.header),
        xml,
        uuid,
    };
    plans.palette.describe(&mut metadata);
    plans.channels.describe(&mut metadata);
    Ok(metadata)
}

/// The top-level `xml ` and `uuid` boxes in file order: those before the
/// codestream, then those after it. A `uuid` box too short for its 16-byte
/// identifier is skipped. Every copy is of bytes in the input, so together
/// they are bounded by its length.
fn passthrough(file: &Jp2File<'_>) -> (Vec<Vec<u8>>, Vec<UuidBox>) {
    let (mut xml, mut uuid) = (Vec::new(), Vec::new());
    for item in file.other.iter().chain(&file.after_box) {
        match &item.kind {
            b"xml " => xml.push(item.payload.to_vec()),
            b"uuid" => {
                if let Some((id, payload)) = item.payload.split_first_chunk::<16>() {
                    uuid.push(UuidBox {
                        id: *id,
                        payload: payload.to_vec(),
                    });
                }
            }
            _ => {}
        }
    }
    (xml, uuid)
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

/// Plan the sRGB output step ([`ColourOutput::Srgb`](crate::ColourOutput::Srgb))
/// from the file's colour space and the planned output channels, before any
/// tile is decoded.
pub(crate) fn plan_srgb(
    metadata: &Jp2Metadata,
    plans: &ChannelPlans,
    siz: &Siz,
    reduction: u8,
) -> Result<SrgbPlan> {
    srgb::plan(
        &metadata.colour,
        &metadata.channels,
        &plans.outputs(siz)?,
        siz,
        reduction,
    )
}
