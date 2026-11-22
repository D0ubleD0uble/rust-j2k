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
//! A top-level `jp2c` is the exception to the last two rules: OpenJPEG ignores
//! the codestream box's length and reads the codestream to its end, so a `jp2c`
//! whose `LBox` is 2..=7 or whose `XLBox` is below 16 runs to the end of the
//! file, and one that runs past the end of the file is clamped to it. Where
//! the codestream actually ends is the caller's to find (see
//! [`super::after`]); the codestream parser still rejects one that is cut
//! short.
//!
//! Each fault is an [`Error::FileFormat`], and the reader stops after the
//! first one. At the top level, where the input may still be arriving, the
//! file ending inside a box is [`Error::Incomplete`] instead: inside its
//! 8-byte header, its `XLBox`, or its contents (a length damaged to reach
//! past the end of the file looks the same). Inside a superbox, whose
//! contents are whole, a box that runs past them stays `FileFormat`.
//! [`walk_after_codestream`] applies OpenJPEG's looser rule to the bytes
//! after the codestream.

use crate::{Error, Result};

/// One box: its type and its contents (the bytes after the header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Jp2Box<'a> {
    /// `TBox`, the four-character box type.
    pub(crate) kind: [u8; 4],
    /// The box contents, header excluded.
    pub(crate) payload: &'a [u8],
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
    /// The type of the box being read when the input ended inside it, once
    /// its type field was read.
    cut_kind: Option<[u8; 4]>,
    /// Whether the input ended inside that box's `XLBox` field while it can
    /// still be below 16.
    cut_in_length: bool,
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
            cut_kind: None,
            cut_in_length: false,
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
            cut_kind: None,
            cut_in_length: false,
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

    /// The type of the top-level box the input ended inside, when the last
    /// read was [`Error::Incomplete`] after the box's `TBox` field. A check
    /// that needs only the type can still be made on it.
    pub(crate) fn cut_kind(&self) -> Option<[u8; 4]> {
        self.cut_kind
    }

    /// Whether the input ended inside the `XLBox` field of the box
    /// [`cut_kind`](Self::cut_kind) names while it can still be below 16: its
    /// length, which can still be at fault, is not in yet.
    pub(crate) fn cut_in_length(&self) -> bool {
        self.cut_in_length
    }

    /// Read the box at `self.pos` and step past it.
    fn read(&mut self) -> Result<Jp2Box<'a>> {
        let rest = self.data.get(self.pos..).unwrap_or_default();
        let (lbox, kind) = match rest {
            [a, b, c, d, t0, t1, t2, t3, ..] => {
                (u32::from_be_bytes([*a, *b, *c, *d]), [*t0, *t1, *t2, *t3])
            }
            _ if self.open_ended => {
                return Err(Error::incomplete(format!(
                    "the input ends inside a box header, after {} of its 8 bytes",
                    rest.len()
                )));
            }
            _ => {
                return Err(Error::file_format(format!(
                    "a box header needs 8 bytes but {} remain in {}",
                    rest.len(),
                    self.parent
                )));
            }
        };
        self.cut_kind = Some(kind);
        // An `XLBox` below 16 is a fault, which bytes still to come can make
        // only while those in are all 0.
        self.cut_in_length = lbox == 1
            && rest.len() < 16
            && rest.get(8..).is_some_and(|xl| xl.iter().all(|&b| b == 0));
        let name = type_name(kind);
        self.last_open_ended = lbox == 0;
        // The top-level codestream box, whose length OpenJPEG does not use.
        let codestream = self.open_ended && &kind == b"jp2c";

        // The box's total length (header included) and its header length. A
        // codestream box too short for its own header runs to the end of the
        // file, like one of length 0.
        let (length, header): (u64, usize) = match lbox {
            0 if self.open_ended => (rest.len() as u64, 8),
            0 => {
                return Err(Error::file_format(format!(
                    "box `{name}` in {} has length 0 (to the end of the file), which only the \
                     last box of the file may use",
                    self.parent
                )));
            }
            1 => {
                let Some(xl) = rest.get(8..16) else {
                    if self.open_ended {
                        return Err(Error::incomplete(format!(
                            "the input ends inside the 16-byte header of box `{name}`"
                        )));
                    }
                    return Err(Error::file_format(format!(
                        "box `{name}` has a 64-bit length but its `XLBox` field is truncated"
                    )));
                };
                let mut be = [0u8; 8];
                be.copy_from_slice(xl);
                let xlbox = u64::from_be_bytes(be);
                match xlbox {
                    0..16 if codestream => (rest.len() as u64, 16),
                    0..16 => {
                        return Err(Error::file_format(format!(
                            "box `{name}` has `XLBox` {xlbox}, shorter than its 16-byte header"
                        )));
                    }
                    _ => (xlbox, 16),
                }
            }
            2..=7 if codestream => (rest.len() as u64, 8),
            2..=7 => {
                return Err(Error::file_format(format!(
                    "box `{name}` has length {lbox}, shorter than its 8-byte header"
                )));
            }
            n => (u64::from(n), 8),
        };

        // OpenJPEG reads a codestream to its end whatever its box says, so a
        // top-level `jp2c` that runs past the end of the file is clamped.
        let length = if codestream {
            length.min(rest.len() as u64)
        } else {
            length
        };
        if length > rest.len() as u64 && self.open_ended {
            return Err(Error::incomplete(format!(
                "the input ends inside box `{name}`, which is {length} bytes, after {} of them",
                rest.len()
            )));
        }
        if length > rest.len() as u64 {
            return Err(Error::file_format(format!(
                "box `{name}` is {length} bytes but only {} remain in {}",
                rest.len(),
                self.parent
            )));
        }
        // `length <= rest.len()`, so it fits a usize and the slices below are
        // in bounds; `header <= length` was checked per form above.
        let length = length as usize;
        let payload = rest.get(header..length).ok_or_else(|| {
            Error::inconsistent(format!("box `{name}` bounds checked but not sliceable"))
        })?;
        self.pos = self
            .pos
            .checked_add(length)
            .ok_or_else(|| Error::inconsistent("box offset overflow"))?;
        self.cut_kind = None;
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

/// The box types OpenJPEG has a reader for: the top-level `jP  `, `ftyp` and
/// `jp2h`, and the `jp2h` children it also reads at the top level. After the
/// codestream, a box of one of these types that runs past the end of the file
/// is an error; any other such box ends the walk.
pub(crate) const READ_BY_OPENJPEG: [&[u8; 4]; 9] = [
    b"jP  ", b"ftyp", b"jp2h", b"ihdr", b"colr", b"bpcc", b"pclr", b"cmap", b"cdef",
];

/// What [`walk_after_codestream`] found: the whole boxes it stepped over, in
/// file order, and the length fault that ended it, if one did.
#[derive(Debug, Clone)]
pub(crate) struct AfterCodestream<'a> {
    /// The boxes walked, in file order.
    pub(crate) boxes: Vec<Jp2Box<'a>>,
    /// The fault that ended the walk: a box whose `LBox` is 2..=7 or whose
    /// `XLBox` is below 16, or a box OpenJPEG reads that runs past the end of
    /// the file. It comes after every box in `boxes`.
    pub(crate) fault: Option<Error>,
    /// Whether the input ends inside the box the walk ended on, so that more
    /// bytes can still change that box: the fault is a box that runs past the
    /// end of the input, or, with no fault, the last box in `boxes` has
    /// `LBox` 0 and runs to the end.
    pub(crate) open: bool,
}

/// Walk the bytes after the codestream the way OpenJPEG does after decoding
/// (`opj_jp2_read_header_procedure`, run again by `opj_jp2_end_decompress`).
///
/// - Fewer than 8 bytes left, a truncated `XLBox`, or an `XLBox` of 2^32 or
///   more ends the walk quietly, as does a `jp2c`.
/// - `LBox` 0 is a box that runs to the end of the file.
/// - A box shorter than its own header is a fault.
/// - A box that runs past the end of the file is a fault if OpenJPEG reads
///   its type ([`READ_BY_OPENJPEG`]); any other ends the walk quietly.
///
/// Nothing here reads a box's contents; [`super::after`] runs OpenJPEG's box
/// readers over them, and the caller picks out the `xml ` and `uuid` boxes it
/// passes through.
pub(crate) fn walk_after_codestream(mut rest: &[u8]) -> AfterCodestream<'_> {
    let mut boxes = Vec::new();
    let mut open = false;
    let fault = loop {
        let [a, b, c, d, t0, t1, t2, t3, ..] = *rest else {
            break None;
        };
        let lbox = u32::from_be_bytes([a, b, c, d]);
        let kind = [t0, t1, t2, t3];
        let (length, header) = match lbox {
            // OpenJPEG cannot hold a box over 2^32 bytes; nothing that long
            // fits in memory here either.
            0 => match u32::try_from(rest.len()) {
                Ok(length) => (length, 8),
                Err(_) => break None,
            },
            1 => match rest.get(8..16) {
                Some(&[0, 0, 0, 0, l0, l1, l2, l3]) => (u32::from_be_bytes([l0, l1, l2, l3]), 16),
                _ => break None,
            },
            n => (n, 8),
        };
        if &kind == b"jp2c" {
            break None;
        }
        if (length as usize) < header {
            let field = if lbox == 1 { "`XLBox`" } else { "length" };
            break Some(Error::file_format(format!(
                "box `{}` after the codestream has {field} {length}, shorter than its \
                 {header}-byte header",
                type_name(kind)
            )));
        }
        let Some(payload) = rest.get(header..length as usize) else {
            if READ_BY_OPENJPEG.contains(&&kind) {
                open = true;
                break Some(Error::file_format(format!(
                    "box `{}` after the codestream is {length} bytes but only {} remain in \
                     the file",
                    type_name(kind),
                    rest.len()
                )));
            }
            break None;
        };
        boxes.push(Jp2Box { kind, payload });
        // A box of length 0 takes the rest of the input, so it is the last.
        open = lbox == 0;
        // `length >= header >= 8`, so every step moves at least 8 bytes and
        // the walk ends.
        rest = rest.get(length as usize..).unwrap_or_default();
    };
    AfterCodestream { boxes, fault, open }
}
