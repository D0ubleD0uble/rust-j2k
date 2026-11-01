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

/// The JP2 Header box's contents: `ihdr` parsed, every other child kept as its
/// raw contents in file order. Repeats are kept too, so the code that reads a
/// box can apply its own rule (the first `colr` wins; a second `pclr` is an
/// error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Jp2Header<'a> {
    pub(crate) ihdr: Ihdr,
    /// Bits Per Component boxes (I.5.3.2).
    pub(crate) bpcc: Vec<&'a [u8]>,
    /// Colour Specification boxes (I.5.3.3).
    pub(crate) colr: Vec<&'a [u8]>,
    /// Palette boxes (I.5.3.4).
    pub(crate) pclr: Vec<&'a [u8]>,
    /// Component Mapping boxes (I.5.3.5).
    pub(crate) cmap: Vec<&'a [u8]>,
    /// Channel Definition boxes (I.5.3.6).
    pub(crate) cdef: Vec<&'a [u8]>,
    /// Resolution superboxes (I.5.3.7), contents not yet descended into.
    pub(crate) res: Vec<&'a [u8]>,
    /// Boxes this reader does not know, skipped (I.4: a reader ignores them).
    pub(crate) unknown: Vec<Jp2Box<'a>>,
}

impl<'a> Jp2Header<'a> {
    /// Walk a `jp2h` box's children. `ihdr` must come first (I.5.3.1); a later
    /// `ihdr` is ignored, as OpenJPEG does.
    pub(crate) fn parse(payload: &'a [u8]) -> Result<Self> {
        let mut children = Boxes::children(payload, "the `jp2h` box");
        let ihdr = match children.next() {
            None => {
                return Err(Error::FileFormat(
                    "the `jp2h` box is empty; it must start with `ihdr`".into(),
                ));
            }
            Some(first) => {
                let first = first?;
                if &first.kind != b"ihdr" {
                    return Err(Error::FileFormat(format!(
                        "the `jp2h` box must start with `ihdr`, not `{}`",
                        first.name()
                    )));
                }
                Ihdr::parse(first.payload)?
            }
        };

        let mut header = Jp2Header {
            ihdr,
            bpcc: Vec::new(),
            colr: Vec::new(),
            pclr: Vec::new(),
            cmap: Vec::new(),
            cdef: Vec::new(),
            res: Vec::new(),
            unknown: Vec::new(),
        };
        for child in children {
            let child = child?;
            match &child.kind {
                b"ihdr" => {}
                b"bpcc" => header.bpcc.push(child.payload),
                b"colr" => header.colr.push(child.payload),
                b"pclr" => header.pclr.push(child.payload),
                b"cmap" => header.cmap.push(child.payload),
                b"cdef" => header.cdef.push(child.payload),
                b"res " => header.res.push(child.payload),
                _ => header.unknown.push(child),
            }
        }
        Ok(header)
    }
}
