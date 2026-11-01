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
//! - A box longer than the bytes left in its parent is truncated.
//!
//! Each fault is an [`Error::FileFormat`], and the reader stops after the
//! first one.

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
        }
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
