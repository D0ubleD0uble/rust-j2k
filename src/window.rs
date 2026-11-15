//! Decoding part of an image: a window on the reference grid, one tile, or
//! some of the components ([`DecodeOptions::window`], [`DecodeOptions::tile`],
//! [`DecodeOptions::components`]).
//!
//! The tiles outside the window are not decoded, and the components left out
//! are not decoded past the packet walk. Inside a tile the window needs, only
//! the code-blocks whose wavelet support reaches the window are decoded, and
//! the wavelet runs over only the samples the window needs
//! ([`crate::dwt::Footprint`]).

use crate::codestream::markers::{Rect, Siz};
use crate::{ColorOutput, DecodeOptions, Error, ReportEntry, Result};

/// A rectangle on the image's reference grid, `x0..x1` by `y0..y1`, for
/// [`DecodeOptions::window`]. The bounds are half-open: `x1` and `y1` are one
/// past the last column and row.
///
/// The reference grid is the one SIZ declares, so the image area is
/// `XOsiz..Xsiz` by `YOsiz..Ysiz`, and `(0, 0)` is the grid's origin, not the
/// image's top-left when the image has an offset. This is the rectangle
/// OpenJPEG's `opj_set_decode_area` and `opj_decompress -d x0,y0,x1,y1` take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[doc(alias = "region")]
#[doc(alias = "decode_area")]
pub struct Window {
    /// The left edge.
    pub x0: u32,
    /// The top edge.
    pub y0: u32,
    /// One past the right edge.
    pub x1: u32,
    /// One past the bottom edge.
    pub y1: u32,
}

impl Window {
    /// The window `x0..x1` by `y0..y1`.
    ///
    /// ```
    /// use rust_j2k::Window;
    ///
    /// let window = Window::new(16, 8, 48, 40);
    /// assert_eq!((window.x1 - window.x0, window.y1 - window.y0), (32, 32));
    /// ```
    #[must_use]
    pub const fn new(x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Window { x0, y0, x1, y1 }
    }

    fn rect(self) -> Rect {
        Rect {
            x0: self.x0,
            y0: self.y0,
            x1: self.x1,
            y1: self.y1,
        }
    }
}

/// The part of an image a decode returns: its area on the reference grid and
/// the components, in output order.
#[derive(Debug, Clone)]
pub(crate) struct Selection {
    /// The codestream's SIZ with its image area cut to what the decode
    /// returns ([`Siz::windowed`]). Equal to the codestream's when the decode
    /// returns the whole image.
    pub(crate) area: Siz,
    /// The SIZ components the decode returns, in output order.
    pub(crate) order: Vec<usize>,
    /// Whether [`DecodeOptions::components`] chose the components.
    pub(crate) subset: bool,
}

impl Selection {
    /// Whether tile `index` of `siz` has any of the reference grid this
    /// decode returns. A tile that touches the area only where a component
    /// has no sample is still decoded, as OpenJPEG picks its tiles on the
    /// reference grid too.
    pub(crate) fn needs_tile(&self, siz: &Siz, index: u32) -> bool {
        siz.tile_rect(index)
            .is_some_and(|tile| !tile.intersect(&self.area.image_rect()).is_empty())
    }

    /// The tiles of `siz` this decode reads, one flag per tile index.
    pub(crate) fn tiles(&self, siz: &Siz) -> Vec<bool> {
        (0..siz.num_tiles())
            .map(|index| self.needs_tile(siz, index))
            .collect()
    }
}

/// The checks of the three options that need no input.
pub(crate) fn check_options(options: &DecodeOptions) -> Result<()> {
    if options.window.is_some() && options.tile.is_some() {
        return Err(Error::InvalidOptions(
            "`DecodeOptions::window` and `DecodeOptions::tile` are both set; a decode \
             returns one area"
                .into(),
        ));
    }
    if let Some(w) = options.window.filter(|w| w.x0 >= w.x1 || w.y0 >= w.y1) {
        return Err(Error::InvalidOptions(format!(
            "the window {}..{} by {}..{} is empty",
            w.x0, w.x1, w.y0, w.y1
        )));
    }
    if let Some(components) = &options.components {
        if components.is_empty() {
            return Err(Error::InvalidOptions(
                "`DecodeOptions::components` is `Some` of no component; a decode returns \
                 at least one"
                    .into(),
            ));
        }
        let mut sorted = components.clone();
        sorted.sort_unstable();
        if let Some(pair) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(Error::InvalidOptions(format!(
                "`DecodeOptions::components` names component {} more than once",
                pair[0]
            )));
        }
        if options.color == ColorOutput::Srgb {
            return Err(Error::InvalidOptions(
                "`DecodeOptions::components` returns the components as coded, so it cannot \
                 be combined with `ColorOutput::Srgb`"
                    .into(),
            ));
        }
    }
    Ok(())
}

/// What a decode of a codestream of `siz` returns under `options`, at
/// `options.resolution_reduction`. A window is cut to the image area; one
/// with no part inside it, a tile index past the grid, a component past
/// SIZ's, and an area that holds no sample of a returned component at the
/// reduction are [`Error::InvalidOptions`], as OpenJPEG refuses them.
pub(crate) fn select(siz: &Siz, options: &DecodeOptions) -> Result<Selection> {
    let image = siz.image_rect();
    let count = siz.components.len();
    let order: Vec<usize> = match &options.components {
        None => (0..count).collect(),
        Some(list) => {
            if let Some(&past) = list.iter().find(|&&c| usize::from(c) >= count) {
                return Err(Error::InvalidOptions(format!(
                    "`DecodeOptions::components` names component {past}, but the image has \
                     {count}"
                )));
            }
            list.iter().map(|&c| usize::from(c)).collect()
        }
    };
    let area = match (options.window, options.tile) {
        (Some(window), _) => {
            let area = window.rect().intersect(&image);
            if area.is_empty() {
                return Err(Error::InvalidOptions(format!(
                    "the window {}..{} by {}..{} is outside the image area {}..{} by {}..{}",
                    window.x0,
                    window.x1,
                    window.y0,
                    window.y1,
                    image.x0,
                    image.x1,
                    image.y0,
                    image.y1
                )));
            }
            Some(area)
        }
        (None, Some(tile)) => Some(siz.tile_rect(tile).ok_or_else(|| {
            Error::InvalidOptions(format!(
                "`DecodeOptions::tile` is {tile}, but the image has {} tiles",
                siz.num_tiles()
            ))
        })?),
        (None, None) => None,
    };
    let Some(area) = area else {
        return Ok(Selection {
            area: siz.clone(),
            order,
            subset: options.components.is_some(),
        });
    };
    let area = siz.windowed(area);
    let reduction = options.resolution_reduction;
    for &comp in &order {
        let (width, height) = area.component_extent_at(comp, reduction).unwrap_or((0, 0));
        if width == 0 || height == 0 {
            return Err(Error::InvalidOptions(format!(
                "the area {}..{} by {}..{} holds no sample of component {comp} at resolution \
                 reduction {reduction}",
                area.x_offset, area.x_size, area.y_offset, area.y_size
            )));
        }
    }
    Ok(Selection {
        area,
        order,
        subset: options.components.is_some(),
    })
}

/// Keep the report entries about the tiles a decode reads (`tiles`, one flag
/// per tile index), so a tile outside the window does not make the image
/// look incomplete. A run of missing tiles is narrowed to the first and last
/// tile of it the decode reads; the tiles between them are missing too, even
/// those outside the window.
pub(crate) fn keep_entries(entries: Vec<ReportEntry>, tiles: &[bool]) -> Vec<ReportEntry> {
    let reads = |tile: u32| tiles.get(tile as usize).copied().unwrap_or(false);
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            ReportEntry::MissingTiles { tiles: run } => {
                let first = run.clone().find(|&t| reads(t))?;
                let last = run.clone().rev().find(|&t| reads(t))?;
                Some(ReportEntry::MissingTiles {
                    tiles: first..last + 1,
                })
            }
            ReportEntry::MissingPackets { tile, .. }
            | ReportEntry::TruncatedTilePart { tile, .. }
            | ReportEntry::TilePartCountMismatch { tile, .. }
            | ReportEntry::LeftoverBytes { tile, .. } => reads(tile).then_some(entry),
            other => Some(other),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_of_missing_tiles_is_narrowed_to_the_tiles_read() {
        let tiles = [false, true, false, true, false, false];
        let entries = vec![
            ReportEntry::MissingTiles { tiles: 0..5 },
            ReportEntry::MissingTiles { tiles: 4..6 },
            ReportEntry::TilePartCountMismatch {
                tile: 2,
                declared: 2,
                found: 1,
            },
            ReportEntry::TilePartCountMismatch {
                tile: 3,
                declared: 2,
                found: 1,
            },
            ReportEntry::MissingEoc,
        ];
        assert_eq!(
            keep_entries(entries, &tiles),
            vec![
                ReportEntry::MissingTiles { tiles: 1..4 },
                ReportEntry::TilePartCountMismatch {
                    tile: 3,
                    declared: 2,
                    found: 1,
                },
                ReportEntry::MissingEoc,
            ]
        );
    }
}
