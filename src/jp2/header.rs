//! The File Type box (I.5.2) and the JP2 Header superbox (I.5.3) with its
//! Image Header box (I.5.3.1).
//!
//! `ihdr` is parsed here because the codestream cannot be checked against the
//! file without it. The other `jp2h` children are kept as raw slices, in file
//! order, for the code that interprets them.

use super::boxes::{Boxes, Jp2Box, type_name};
use crate::{Error, Result};

/// The brand and compatibility code of a JP2 file.
const JP2_BRAND: [u8; 4] = *b"jp2 ";

/// Check a File Type box: `BR` (4 bytes), `MinV` (4 bytes), then any number of
/// 4-byte compatibility codes. A file is read only if `jp2 ` is its brand or
/// in its compatibility list (I.5.2); anything else is a JPX, JPM or MJ2 file
/// that does not claim JP2 compatibility, and is [`Error::Unsupported`].
pub(crate) fn check_file_type(ftyp: &Jp2Box<'_>) -> Result<()> {
    let payload = ftyp.payload;
    if payload.len() < 8 || !payload.len().is_multiple_of(4) {
        return Err(Error::FileFormat(format!(
            "the `ftyp` box holds {} bytes; it needs a brand, a version, and whole \
             4-byte compatibility codes",
            payload.len()
        )));
    }
    let brand = [payload[0], payload[1], payload[2], payload[3]];
    let compatible = brand == JP2_BRAND
        || payload[8..]
            .chunks_exact(4)
            .any(|code| code == JP2_BRAND.as_slice());
    if !compatible {
        return Err(Error::Unsupported(format!(
            "file brand `{}` without `jp2 ` compatibility (a JPX, JPM or MJ2 file that \
             does not claim to be readable as JP2)",
            type_name(brand)
        )));
    }
    Ok(())
}

/// The Image Header box (I.5.3.1), 14 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ihdr {
    /// `HEIGHT`: the image area height.
    pub(crate) height: u32,
    /// `WIDTH`: the image area width.
    pub(crate) width: u32,
    /// `NC`: the number of components.
    pub(crate) components: u16,
    /// `BPC`: bit depth minus one and sign, or 255 when `bpcc` gives the
    /// depths per component. Read, not used: SIZ is the truth for depth.
    pub(crate) bits_per_component: u8,
    /// `C`: the compression type, 7 for JPEG 2000.
    pub(crate) compression: u8,
    /// `UnkC`: 1 when the colour space is unknown.
    pub(crate) unknown_colourspace: u8,
    /// `IPR`: 1 when the file holds an intellectual property box.
    pub(crate) ipr: u8,
}

impl Ihdr {
    fn parse(payload: &[u8]) -> Result<Self> {
        let &[h0, h1, h2, h3, w0, w1, w2, w3, n0, n1, bpc, c, unk_c, ipr] = payload else {
            return Err(Error::FileFormat(format!(
                "the `ihdr` box holds {} bytes, not 14",
                payload.len()
            )));
        };
        Ok(Ihdr {
            height: u32::from_be_bytes([h0, h1, h2, h3]),
            width: u32::from_be_bytes([w0, w1, w2, w3]),
            components: u16::from_be_bytes([n0, n1]),
            bits_per_component: bpc,
            compression: c,
            unknown_colourspace: unk_c,
            ipr,
        })
    }
}

/// The JP2 Header box's contents: `ihdr` parsed, every other child kept in one
/// list in the order OpenJPEG reads them. Repeats are kept too, so the code
/// that reads a box can apply its own rule (the first `colr` wins; a second
/// `pclr` is an error).
///
/// The order matters because OpenJPEG runs a box's handler as it meets the
/// box, and a handler can depend on the ones before it (`bpcc` is checked
/// against the component count `ihdr` sets). The list holds the children of
/// every `jp2h` box before the codestream in file order, then the header boxes
/// found misplaced at the top level after the first `jp2h` (see
/// [`Jp2Header::add_misplaced`]) as they come. [`Jp2Header::ihdr_at`] says
/// where the first `ihdr` sat in it. The per-type accessors ([`Jp2Header::colr`]
/// and the like) give the payloads of one type, in that order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Jp2Header<'a> {
    /// The first `ihdr` read; later ones are ignored.
    pub(crate) ihdr: Ihdr,
    /// How many entries of `children` come before the first `ihdr` in read
    /// order.
    pub(crate) ihdr_at: usize,
    /// Every child other than `ihdr`, in read order.
    pub(crate) children: Vec<Jp2Box<'a>>,
}

impl<'a> Jp2Header<'a> {
    /// Read the first `jp2h` box.
    pub(crate) fn parse(payload: &'a [u8]) -> Result<Self> {
        Self::read(payload, None)
    }

    /// Read a later `jp2h` box into this header. OpenJPEG merges a second
    /// `jp2h` this way: its `ihdr` is ignored and its other children are
    /// applied, so they are appended here and meet the same checks.
    pub(crate) fn merge(self, payload: &'a [u8]) -> Result<Self> {
        Self::read(payload, Some(self))
    }

    /// Take a header box found at the top level after the first `jp2h`.
    /// OpenJPEG reads such a box as though it were inside `jp2h` ("read
    /// anyway, we already have jp2h"), so it is recorded the same way; a
    /// misplaced `ihdr` is ignored like any later one. Returns `false` for a
    /// box that is not a `jp2h` child type, which the caller keeps as a
    /// top-level box.
    pub(crate) fn add_misplaced(&mut self, item: Jp2Box<'a>) -> bool {
        match &item.kind {
            b"ihdr" => true,
            b"bpcc" | b"colr" | b"pclr" | b"cmap" | b"cdef" => {
                self.children.push(item);
                true
            }
            _ => false,
        }
    }

    /// Walk one `jp2h` box's children. Every `jp2h` box must hold an `ihdr`
    /// of 14 bytes, anywhere among its children (OpenJPEG requires both and
    /// nothing more); only the first `ihdr` of the file is parsed.
    fn read(payload: &'a [u8], previous: Option<Self>) -> Result<Self> {
        let mut known = previous
            .as_ref()
            .map(|header| (header.ihdr, header.ihdr_at));
        let mut has_ihdr = false;
        let mut children = previous.map(|header| header.children).unwrap_or_default();
        for child in Boxes::children(payload, "the `jp2h` box") {
            let child = child?;
            if &child.kind == b"ihdr" {
                has_ihdr = true;
                if known.is_none() {
                    known = Some((Ihdr::parse(child.payload)?, children.len()));
                }
            } else {
                children.push(child);
            }
        }
        match known {
            Some((ihdr, ihdr_at)) if has_ihdr => Ok(Jp2Header {
                ihdr,
                ihdr_at,
                children,
            }),
            _ => Err(Error::FileFormat("a `jp2h` box has no `ihdr` box".into())),
        }
    }

    /// The payloads of the children of one type, in read order.
    fn payloads(&self, kind: &[u8; 4]) -> Vec<&'a [u8]> {
        self.children
            .iter()
            .filter(|child| &child.kind == kind)
            .map(|child| child.payload)
            .collect()
    }

    /// Colour Specification boxes (I.5.3.3).
    pub(crate) fn colr(&self) -> Vec<&'a [u8]> {
        self.payloads(b"colr")
    }

    /// Palette boxes (I.5.3.4).
    pub(crate) fn pclr(&self) -> Vec<&'a [u8]> {
        self.payloads(b"pclr")
    }

    /// Component Mapping boxes (I.5.3.5).
    pub(crate) fn cmap(&self) -> Vec<&'a [u8]> {
        self.payloads(b"cmap")
    }

    /// Channel Definition boxes (I.5.3.6).
    pub(crate) fn cdef(&self) -> Vec<&'a [u8]> {
        self.payloads(b"cdef")
    }

    /// Bits Per Component boxes (I.5.3.2).
    #[cfg(test)]
    pub(crate) fn bpcc(&self) -> Vec<&'a [u8]> {
        self.payloads(b"bpcc")
    }

    /// Resolution superboxes (I.5.3.7), contents not yet descended into.
    #[cfg(test)]
    pub(crate) fn res(&self) -> Vec<&'a [u8]> {
        self.payloads(b"res ")
    }

    /// Boxes this reader does not know, skipped (I.4: a reader ignores them).
    #[cfg(test)]
    pub(crate) fn unknown(&self) -> Vec<&Jp2Box<'a>> {
        self.children
            .iter()
            .filter(|child| {
                !matches!(
                    &child.kind,
                    b"bpcc" | b"colr" | b"pclr" | b"cmap" | b"cdef" | b"res "
                )
            })
            .collect()
    }
}
