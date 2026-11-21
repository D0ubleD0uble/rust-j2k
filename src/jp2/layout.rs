//! The layout of the palette and channel definition boxes (`pclr`, `cmap`
//! and `cdef`), read without the codestream: how many there are, their order
//! and their payloads. The checks run in one pass over the header boxes in
//! file order, holding what each box has set, as OpenJPEG's box handlers do
//! while it reads `jp2h` (`opj_jp2_read_pclr`, `opj_jp2_read_cmap`,
//! `opj_jp2_read_cdef`), so the first fault in file order is the one
//! reported. What the boxes say about the components is checked later, by
//! the plans.

use super::channels::{self, Entry};
use super::header::Jp2Header;
use super::palette::{self, Palette, PaletteBoxes};
use crate::{Error, Result};

/// What the palette and channel definition boxes hold.
pub(super) struct BoxLayout {
    /// The palette, and its `cmap` entries when the file has a `cmap`.
    pub(super) palette: Option<PaletteBoxes>,
    /// The `cdef` entries, when the file has a `cdef`.
    pub(super) cdef: Option<Vec<Entry>>,
}

/// Read the boxes' layout, refusing its first fault in file order. `jph`
/// says whether the file is a JPH file, which the `cdef` entries record.
pub(super) fn read(header: &Jp2Header<'_>, jph: bool) -> Result<BoxLayout> {
    let mut palette: Option<Palette> = None;
    let mut cmap = None;
    let mut cdef = None;
    for child in &header.children {
        match &child.kind {
            b"pclr" => {
                if palette.is_some() {
                    return Err(Error::file_format("the file has more than one `pclr` box"));
                }
                palette = Some(Palette::parse(child.payload)?);
            }
            b"cmap" => {
                // OpenJPEG needs the palette first, to know how many entries
                // to read, and checks that before it looks for a second
                // `cmap`.
                let Some(columns) = palette.as_ref().map(|p| p.columns.len()) else {
                    return Err(Error::file_format(if header.pclr().is_empty() {
                        "the file has a `cmap` box but no `pclr` box"
                    } else {
                        "the `cmap` box comes before the `pclr` box it maps"
                    }));
                };
                if cmap.is_some() {
                    return Err(Error::file_format("the file has more than one `cmap` box"));
                }
                cmap = Some(palette::parse_cmap(child.payload, columns)?);
            }
            b"cdef" => {
                if cdef.is_some() {
                    return Err(Error::file_format("the file has more than one `cdef` box"));
                }
                cdef = Some(channels::parse(child.payload, jph)?);
            }
            _ => {}
        }
    }
    Ok(BoxLayout {
        palette: palette.map(|palette| (palette, cmap)),
        cdef,
    })
}
