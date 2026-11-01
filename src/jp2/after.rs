//! Where the codestream ends, and the boxes after it.
//!
//! OpenJPEG 2.5.0 does not use the `jp2c` box's length. It decodes the
//! codestream from the start of the box, and when the decode is done it walks
//! the boxes that follow from wherever the decoder stopped: just after `EOC`
//! when every tile-part has a length (`Psot`), the end of the file when the
//! last one runs to the end (`Psot = 0`). On that walk it runs its header box
//! readers again (`opj_jp2_end_decompress`), so a malformed header box after
//! the codestream fails the decode. [`codestream_end`] finds the same place
//! from the tile-part lengths, and [`check`] runs the same readers, in the
//! same state:
//!
//! - `ihdr` is ignored, whatever its length.
//! - `jP  ` and `ftyp` are errors: each must come first or second.
//! - A `jp2h` must hold an `ihdr`, and its children are read like the boxes
//!   below.
//! - A `colr` under 3 bytes is an error. Once a `colr` with `METH` 1 or 2 has
//!   been read (in the header or after the codestream), later ones are
//!   ignored; before that, a `METH` 1 box under 7 bytes or a `METH` 2 box
//!   with no profile is an error. A `colr` read here never changes the
//!   decoded image's colour space: OpenJPEG has set it already.
//! - A `bpcc` must hold `ihdr.NC` bytes.
//! - A `pclr` is an error if a palette is still held, and is otherwise
//!   checked like one in the header. OpenJPEG drops a header palette with no
//!   `cmap` when it decodes, and keeps one that was applied or skipped.
//! - A `cmap` needs a held palette with no `cmap` yet, and `4 * NPC` bytes.
//! - A `cdef` is an error if one is still held, and is otherwise checked like
//!   one in the header (its layout only). OpenJPEG drops the header's `cdef`
//!   once it is applied, and keeps it when it skips it (the raw channels).
//!
//! Nothing read here is applied to the image.

use super::Jp2File;
use super::boxes::{Boxes, Jp2Box};
use super::{channels, colour, palette};
use crate::codestream::markers::marker::{EOC, SOC, SOT};
use crate::{Error, Result};

/// The fewest bytes a tile-part can span: `SOT` and its 10-byte segment,
/// then `SOD`.
const MIN_TILE_PART: u32 = 14;

/// Where a codestream ends, as OpenJPEG's decoder leaves its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// Just after the `EOC` marker at this offset minus 2: every tile-part
    /// has a length, and the last is followed by `EOC`.
    At(usize),
    /// A tile-part with `Psot = 0` runs to the end of the file.
    OpenLastPart,
    /// The main header or the tile-part chain is broken or cut short: the
    /// codestream parser rejects it, or `read_header` reads only its header.
    Unknown,
}

/// Find where the codestream at the start of `bytes` ends by stepping over the
/// main header's marker segments and then the tile-parts by their `Psot`. This
/// reads only marker and segment lengths; the codestream parser checks the
/// rest. Each step moves at least 4 bytes, so the walk is linear in the input.
pub(crate) fn codestream_end(bytes: &[u8]) -> End {
    let marker = |at: usize| {
        bytes
            .get(at..at.checked_add(2)?)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    if marker(0) != Some(SOC) {
        return End::Unknown;
    }
    let mut at = 2usize;
    loop {
        match marker(at) {
            Some(SOT) => break,
            Some(code) if code >> 8 == 0xFF => {
                let Some(length) = marker(at + 2).filter(|&length| length >= 2) else {
                    return End::Unknown;
                };
                at += 2 + usize::from(length);
            }
            _ => return End::Unknown,
        }
    }
    loop {
        let Some(&[p0, p1, p2, p3]) = at.checked_add(6).and_then(|from| bytes.get(from..from + 4))
        else {
            return End::Unknown;
        };
        let psot = u32::from_be_bytes([p0, p1, p2, p3]);
        if psot == 0 {
            return End::OpenLastPart;
        }
        if psot < MIN_TILE_PART {
            return End::Unknown;
        }
        let Some(end) = usize::try_from(psot)
            .ok()
            .and_then(|psot| at.checked_add(psot))
        else {
            return End::Unknown;
        };
        match marker(end) {
            Some(EOC) => return End::At(end + 2),
            Some(SOT) => at = end,
            _ => return End::Unknown,
        }
    }
}

/// What OpenJPEG still holds when it walks the boxes after the codestream,
/// updated as the walk reads them.
struct State {
    /// `ihdr.NC`, which a `bpcc` must match.
    components: usize,
    /// Whether a `colr` with `METH` 1 or 2 has been read.
    has_colour: bool,
    /// The held palette's column count (`NPC`), if one is held.
    palette: Option<usize>,
    /// Whether the held palette has its `cmap`.
    mapped: bool,
    /// Whether a `cdef` is held.
    channel_definitions: bool,
}

impl State {
    /// The state after the decode, from the header boxes and whether the
    /// palette and channel definitions were skipped (`raw_channels`).
    fn after_decode(file: &Jp2File<'_>, raw_channels: bool) -> Self {
        let header = &file.header;
        let mapped = !header.cmap().is_empty();
        // The header's boxes have passed their checks, so the first `pclr`
        // parses; a palette with no `cmap` is dropped unless it was skipped.
        let palette = header
            .pclr()
            .first()
            .and_then(|pclr| pclr.get(2))
            .filter(|_| mapped || raw_channels)
            .map(|&npc| usize::from(npc));
        State {
            components: usize::from(header.ihdr.components),
            has_colour: header
                .colr()
                .iter()
                .any(|colr| matches!(colr.first(), Some(1 | 2))),
            mapped: palette.is_some() && mapped,
            palette,
            channel_definitions: raw_channels && !header.cdef().is_empty(),
        }
    }

    /// Read one box as OpenJPEG's handler for its type does; other types are
    /// ignored.
    fn read(&mut self, item: &Jp2Box<'_>) -> Result<()> {
        let payload = item.payload;
        match &item.kind {
            b"colr" => {
                if payload.len() < 3 {
                    return Err(fault(format!(
                        "a `colr` box holds {} bytes; it needs at least 3",
                        payload.len()
                    )));
                }
                if !self.has_colour {
                    self.has_colour = colour::parse_usable(payload).map_err(context)?.is_some();
                }
            }
            b"bpcc" => {
                if payload.len() != self.components {
                    return Err(fault(format!(
                        "a `bpcc` box holds {} bytes; it needs one per component ({})",
                        payload.len(),
                        self.components
                    )));
                }
            }
            b"pclr" => {
                if self.palette.is_some() {
                    return Err(fault("a second `pclr` box".into()));
                }
                let columns = palette::Palette::parse(payload).map_err(context)?.columns();
                self.palette = Some(columns);
                self.mapped = false;
            }
            b"cmap" => {
                let Some(columns) = self.palette else {
                    return Err(fault("a `cmap` box with no `pclr` box before it".into()));
                };
                if self.mapped {
                    return Err(fault("a second `cmap` box".into()));
                }
                if payload.len() < columns * 4 {
                    return Err(fault(format!(
                        "the `cmap` box holds {} bytes; mapping {columns} palette columns needs {}",
                        payload.len(),
                        columns * 4
                    )));
                }
                self.mapped = true;
            }
            b"cdef" => {
                if self.channel_definitions {
                    return Err(fault("a second `cdef` box".into()));
                }
                channels::check_layout(payload).map_err(context)?;
                self.channel_definitions = true;
            }
            _ => {}
        }
        Ok(())
    }
}

/// A box fault after the codestream.
fn fault(message: String) -> Error {
    Error::FileFormat(format!("after the codestream, {message}"))
}

/// Mark a box reader's error as found after the codestream.
fn context(error: Error) -> Error {
    match error {
        Error::FileFormat(message) => fault(message),
        other => other,
    }
}

/// Read the boxes after the codestream as OpenJPEG does once it has decoded
/// it, if its walk reaches them ([`Jp2File::after_read`]). `raw_channels` is
/// whether the palette and channel definitions were skipped, which decides
/// what OpenJPEG still holds. The walk's length fault, if any, comes after
/// every box it walked.
pub(crate) fn check(file: &Jp2File<'_>, raw_channels: bool) -> Result<()> {
    if !file.after_read {
        return Ok(());
    }
    let mut state = State::after_decode(file, raw_channels);
    for item in &file.after_codestream {
        match &item.kind {
            b"jP  " => return Err(fault("a signature box, which must be the first box".into())),
            b"ftyp" => return Err(fault("an `ftyp` box, which must be the second box".into())),
            b"ihdr" => {}
            b"jp2h" => {
                let mut has_ihdr = false;
                for child in Boxes::children(item.payload, "a `jp2h` box after the codestream") {
                    let child = child?;
                    if &child.kind == b"ihdr" {
                        has_ihdr = true;
                    } else {
                        state.read(&child)?;
                    }
                }
                if !has_ihdr {
                    return Err(fault("a `jp2h` box has no `ihdr` box".into()));
                }
            }
            _ => state.read(item)?,
        }
    }
    match &file.after_fault {
        Some(error) => Err(error.clone()),
        None => Ok(()),
    }
}
