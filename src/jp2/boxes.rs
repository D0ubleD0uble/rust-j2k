//! The JP2 box reader (ISO/IEC 15444-1 I.4): a box is `LBox` (4 bytes),
//! `TBox` (4 bytes), an optional 8-byte `XLBox`, then its contents.
//!
//! The reader walks a byte slice, so a box's contents are a sub-slice of the
//! input and nothing is allocated from a length field. Every length is checked
//! against the bytes its parent actually holds before it is used:
//!
//! - `LBox == 1`: the length is the 64-bit `XLBox`, which must cover its own
//!   16-byte header.
//! - `LBox == 0`: the box runs to the end of the file. That is only possible
//!   for the last box of the file, so it is accepted at the top level and
//!   rejected inside a superbox (OpenJPEG rejects it there too).
//! - `LBox` of 2..=7 is shorter than the box's own header.
//! - A box longer than the bytes left in its parent is truncated, except a
//!   top-level `jp2c`: OpenJPEG ignores the codestream box's length and reads
//!   the codestream to its end, so a `jp2c` that runs past the end of the file
//!   is clamped to it. The codestream parser still rejects a codestream that
//!   is actually cut short.
//!
//! Each fault is an [`Error::FileFormat`], and the reader stops after the
//! first one. [`check_after_codestream`] applies OpenJPEG's looser rule to the
//! bytes after the codestream.

use crate::{Error, Result};

/// One box: its type and its contents (the bytes after the header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Jp2Box<'a> {
    /// `TBox`, the four-character box type.
    pub(crate) kind: [u8; 4],
    /// The box contents, header excluded.
    pub(crate) payload: &'a [u8],
}

impl Jp2Box<'_> {
    /// The box type as text, for error messages.
    pub(crate) fn name(&self) -> String {
        type_name(self.kind)
    }
}

/// A four-character box type as printable text: `jp2h`, `xml `, or escaped
/// bytes for a type that is not ASCII.
pub(crate) fn type_name(kind: [u8; 4]) -> String {
    kind.iter()
        .flat_map(|&b| std::ascii::escape_default(b))
        .map(char::from)
        .collect()
}

/// Iterates the boxes laid end to end in one byte slice: the whole file, or
/// one superbox's contents.
pub(crate) struct Boxes<'a> {
    data: &'a [u8],
    pos: usize,
    /// Whether `LBox == 0` ("to the end of the file") is allowed here. Only the
    /// top level reaches the end of the file.
    open_ended: bool,
    /// What `data` is, for error messages ("the file", "the `jp2h` box").
    parent: &'static str,
    /// Set once an error has been returned, so the iterator then ends.
    done: bool,
    /// Whether the last box read had `LBox == 0`.
    last_open_ended: bool,
}

impl<'a> Boxes<'a> {
    /// The top-level boxes of a file, starting at `start`.
    pub(crate) fn top_level(file: &'a [u8], start: usize) -> Self {
        Boxes {
            data: file,
            pos: start,
            open_ended: true,
            parent: "the file",
            done: false,
            last_open_ended: false,
        }
    }

    /// The child boxes of a superbox, given its contents.
    pub(crate) fn children(payload: &'a [u8], parent: &'static str) -> Self {
        Boxes {
            data: payload,
            pos: 0,
            open_ended: false,
            parent,
            done: false,
            last_open_ended: false,
        }
    }

    /// The bytes after the last box read, not yet walked.
    pub(crate) fn rest(&self) -> &'a [u8] {
        self.data.get(self.pos..).unwrap_or_default()
    }

    /// Whether the last box read had length 0, running to the end of the file.
    pub(crate) fn last_was_open_ended(&self) -> bool {
        self.last_open_ended
    }

    /// Read the box at `self.pos` and step past it.
    fn read(&mut self) -> Result<Jp2Box<'a>> {
        let rest = self.data.get(self.pos..).unwrap_or_default();
        let (lbox, kind) = match rest {
            [a, b, c, d, t0, t1, t2, t3, ..] => {
                (u32::from_be_bytes([*a, *b, *c, *d]), [*t0, *t1, *t2, *t3])
            }
            _ => {
                return Err(Error::FileFormat(format!(
                    "a box header needs 8 bytes but {} remain in {}",
                    rest.len(),
                    self.parent
                )));
            }
        };
        let name = type_name(kind);
        self.last_open_ended = lbox == 0;

        // The box's total length (header included) and its header length.
        let (length, header): (u64, usize) = match lbox {
            0 if self.open_ended => (rest.len() as u64, 8),
            0 => {
                return Err(Error::FileFormat(format!(
                    "box `{name}` in {} has length 0 (to the end of the file), which only the \
                     last box of the file may use",
                    self.parent
                )));
            }
            1 => {
                let Some(xl) = rest.get(8..16) else {
                    return Err(Error::FileFormat(format!(
                        "box `{name}` has a 64-bit length but its XLBox field is truncated"
                    )));
                };
                let mut be = [0u8; 8];
                be.copy_from_slice(xl);
                let xlbox = u64::from_be_bytes(be);
                if xlbox < 16 {
                    return Err(Error::FileFormat(format!(
                        "box `{name}` has XLBox {xlbox}, shorter than its 16-byte header"
                    )));
                }
                (xlbox, 16)
            }
            2..=7 => {
                return Err(Error::FileFormat(format!(
                    "box `{name}` has length {lbox}, shorter than its 8-byte header"
                )));
            }
            n => (u64::from(n), 8),
        };

        // OpenJPEG reads a codestream to its end whatever its box says, so a
        // top-level `jp2c` that runs past the end of the file is clamped.
        let length = if self.open_ended && &kind == b"jp2c" {
            length.min(rest.len() as u64)
        } else {
            length
        };
        if length > rest.len() as u64 {
            return Err(Error::FileFormat(format!(
                "box `{name}` is {length} bytes but only {} remain in {}",
                rest.len(),
                self.parent
            )));
        }
        // `length <= rest.len()`, so it fits a usize and the slices below are
        // in bounds; `header <= length` was checked per form above.
        let length = length as usize;
        let payload = rest.get(header..length).ok_or_else(|| {
            Error::Inconsistent(format!("box `{name}` bounds checked but not sliceable"))
        })?;
        self.pos = self
            .pos
            .checked_add(length)
            .ok_or_else(|| Error::Inconsistent("box offset overflow".into()))?;
        Ok(Jp2Box { kind, payload })
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Result<Jp2Box<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pos >= self.data.len() {
            return None;
        }
        let item = self.read();
        self.done = item.is_err();
        Some(item)
    }
}

/// Check the bytes after the first `jp2c` box the way OpenJPEG does when it
/// walks them after decoding: only a box whose `LBox` is 2..=7, or whose
/// `XLBox` is below 16, is an error. The walk stops quietly at anything it
/// cannot follow (fewer than 8 bytes left, `LBox` 0, a truncated `XLBox`, a box
/// past the end of the file) and at the next `jp2c`, as OpenJPEG's does. Box
/// contents are never read.
pub(crate) fn check_after_codestream(mut rest: &[u8]) -> Result<()> {
    while let [a, b, c, d, t0, t1, t2, t3, ..] = *rest {
        let lbox = u32::from_be_bytes([a, b, c, d]);
        let kind = [t0, t1, t2, t3];
        let length = match lbox {
            0 => return Ok(()),
            1 => {
                let Some(xl) = rest.get(8..16) else {
                    return Ok(());
                };
                let mut be = [0u8; 8];
                be.copy_from_slice(xl);
                let xlbox = u64::from_be_bytes(be);
                if xlbox < 16 {
                    return Err(Error::FileFormat(format!(
                        "box `{}` after the codestream has XLBox {xlbox}, shorter than its \
                         16-byte header",
                        type_name(kind)
                    )));
                }
                xlbox
            }
            2..=7 => {
                return Err(Error::FileFormat(format!(
                    "box `{}` after the codestream has length {lbox}, shorter than its \
                     8-byte header",
                    type_name(kind)
                )));
            }
            n => u64::from(n),
        };
        if &kind == b"jp2c" || length > rest.len() as u64 {
            return Ok(());
        }
        // `8 <= length <= rest.len()` by the arms above, so every step moves at
        // least 8 bytes and the walk ends. Checked rather than assumed, so a
        // future arm that let a shorter length through fails loudly instead of
        // looping.
        let step = usize::try_from(length)
            .ok()
            .filter(|&step| step >= 8)
            .ok_or_else(|| {
                Error::Inconsistent(format!(
                    "box walk step of {length} bytes after the codestream"
                ))
            })?;
        rest = rest.get(step..).unwrap_or_default();
    }
    Ok(())
}
