//! Decoding part of an image: a window on the reference grid, one tile, or
//! some of the components ([`DecodeOptions::region`],
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
/// [`Region::Window`]. The bounds are half-open: `x1` and `y1` are one
/// past the last column and row.
///
/// The reference grid is the one SIZ declares, so the image area is
/// `XOsiz..Xsiz` by `YOsiz..Ysiz`, and `(0, 0)` is the grid's origin, not the
/// image's top-left when the image has an offset. This is the rectangle
/// OpenJPEG's `opj_set_decode_area` and `opj_decompress -d x0,y0,x1,y1` take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// The area of the image a decode returns, for [`DecodeOptions::region`].
///
/// It selects the decode area. It is not the JPEG 2000 region of interest
/// (`RGN`), which the decoder applies on its own.
///
/// One value holds the choice, so a window and a tile cannot be asked for
/// together. The enum is `#[non_exhaustive]`, so a new kind of area is not a
/// breaking change.
///
/// ```
/// use rust_j2k::{DecodeOptions, Region, Window};
///
/// assert_eq!(Region::default(), Region::Whole);
/// let options = DecodeOptions::default().with_region(Region::Window(Window::new(0, 0, 8, 8)));
/// assert_eq!(options.region(), &Region::Window(Window::new(0, 0, 8, 8)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
#[doc(alias = "decode_area")]
pub enum Region {
    /// The whole image. This is the default.
    #[default]
    Whole,
    /// A rectangle of the reference grid, cut to the image area.
    Window(Window),
    /// One tile, by its index in raster order from 0. The header's
    /// [`tiles`](crate::Header::tiles) gives the grid, and its
    /// [`count`](crate::TileGrid::count) how many tiles there are.
    Tile(u32),
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

    /// The part of the image the tiles this decode reads cover, on the
    /// reference grid: the area grown out to the edges of the tiles it
    /// reaches, which [`tiles`](Self::tiles) flags. Those tiles are a block
    /// of whole rows and columns of the grid, so their union is a rectangle.
    /// It is the image area when the decode returns the whole image, or when
    /// the area is not inside the tile grid, which SIZ's checks rule out.
    pub(crate) fn read_rect(&self, siz: &Siz) -> Rect {
        let image = siz.image_rect();
        let area = self.area.image_rect();
        let (across, _) = siz.tile_grid();
        if area.is_empty() || area == image || siz.tile_width == 0 || siz.tile_height == 0 {
            return image;
        }
        // The grid column of `x`, which is at or past the tile origin (A-9).
        let column = |x: u32| (x.saturating_sub(siz.tile_x_offset)) / siz.tile_width;
        let row = |y: u32| (y.saturating_sub(siz.tile_y_offset)) / siz.tile_height;
        let index = |x: u32, y: u32| u64::from(row(y)) * u64::from(across) + u64::from(column(x));
        let first = u32::try_from(index(area.x0, area.y0)).ok();
        let last = u32::try_from(index(area.x1 - 1, area.y1 - 1)).ok();
        match (
            first.and_then(|i| siz.tile_rect(i)),
            last.and_then(|i| siz.tile_rect(i)),
        ) {
            (Some(first), Some(last)) => Rect {
                x0: first.x0,
                y0: first.y0,
                x1: last.x1,
                y1: last.y1,
            },
            _ => image,
        }
    }
}

/// The checks of the three options that need no input.
pub(crate) fn check_options(options: &DecodeOptions) -> Result<()> {
    if let Region::Window(w) = options.region {
        if w.x0 >= w.x1 || w.y0 >= w.y1 {
            return Err(Error::invalid_options(format!(
                "the window {}..{} by {}..{} is empty",
                w.x0, w.x1, w.y0, w.y1
            )));
        }
    }
    if let Some(components) = &options.components {
        if components.is_empty() {
            return Err(Error::invalid_options(
                "`DecodeOptions::components` is `Some` of no component; a decode returns \
                 at least one",
            ));
        }
        let mut sorted = components.clone();
        sorted.sort_unstable();
        if let Some(pair) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(Error::invalid_options(format!(
                "`DecodeOptions::components` names component {} more than once",
                pair[0]
            )));
        }
        if options.color == ColorOutput::Srgb {
            return Err(Error::invalid_options(
                "`DecodeOptions::components` returns the components as coded, so it cannot \
                 be combined with `ColorOutput::Srgb`",
            ));
        }
    }
    Ok(())
}

/// What a decode of a codestream of `siz` returns under `options`, at
/// `options.resolution_reduction()`. A window is cut to the image area; one
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
                return Err(Error::invalid_options(format!(
                    "`DecodeOptions::components` names component {past}, but the image has \
                     {count}"
                )));
            }
            list.iter().map(|&c| usize::from(c)).collect()
        }
    };
    let area = match options.region {
        Region::Whole => None,
        Region::Window(window) => {
            let area = window.rect().intersect(&image);
            if area.is_empty() {
                return Err(Error::invalid_options(format!(
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
        Region::Tile(tile) => Some(siz.tile_rect(tile).ok_or_else(|| {
            let tiles = siz.num_tiles();
            Error::invalid_options(if tiles == 1 {
                format!("tile {tile} is past the image's 1 tile (tile 0)")
            } else {
                format!(
                    "tile {tile} is past the image's {tiles} tiles (0 to {})",
                    tiles.saturating_sub(1)
                )
            })
        })?),
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
            return Err(Error::invalid_options(format!(
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
/// look incomplete. A run of missing tiles is split into the runs of it the
/// decode reads.
///
/// A window reads a rectangle of tiles, so a run splits into at most one
/// piece per row of tiles it reads, each of at least one tile read: the
/// pieces are bounded by the tiles read, which the sample limit bounds.
pub(crate) fn keep_entries(entries: Vec<ReportEntry>, tiles: &[bool]) -> Vec<ReportEntry> {
    let reads = |tile: u32| tiles.get(tile as usize).copied().unwrap_or(false);
    let mut kept = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            ReportEntry::MissingTiles { tiles: run } => {
                let mut tile = run.start;
                while tile < run.end {
                    if !reads(tile) {
                        tile += 1;
                        continue;
                    }
                    let start = tile;
                    while tile < run.end && reads(tile) {
                        tile += 1;
                    }
                    kept.push(ReportEntry::MissingTiles { tiles: start..tile });
                }
            }
            ReportEntry::MissingPackets { tile, .. }
            | ReportEntry::TruncatedTilePart { tile, .. }
            | ReportEntry::TilePartCountMismatch { tile, .. }
            | ReportEntry::LeftoverBytes { tile, .. } => {
                if reads(tile) {
                    kept.push(entry);
                }
            }
            other => kept.push(other),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream::markers::SizComponent;

    /// `read_rect` is the union of the tiles `tiles` flags, for windows
    /// on and off the tile grid, clipped edge tiles, and an image and tile
    /// grid with origins of their own.
    #[test]
    fn the_read_rect_is_the_union_of_the_tiles_read() {
        // A 7x5 grid of 16x14 tiles from (2, 3); the image is 101x67 at (7, 5),
        // so the edge tiles are partial.
        let siz = Siz {
            x_size: 108,
            y_size: 72,
            x_offset: 7,
            y_offset: 5,
            tile_width: 16,
            tile_height: 14,
            tile_x_offset: 2,
            tile_y_offset: 3,
            components: vec![SizComponent {
                bit_depth: 8,
                signed: false,
                x_sampling: 1,
                y_sampling: 1,
            }],
        };
        let image = siz.image_rect();
        let mut checked = 0;
        for x0 in (0..115).step_by(3) {
            for x1 in (x0 + 1..120).step_by(7) {
                for (y0, y1) in [(0, 80), (5, 6), (16, 17), (17, 30), (30, 72), (70, 90)] {
                    let window = Rect { x0, y0, x1, y1 }.intersect(&image);
                    if window.is_empty() {
                        continue;
                    }
                    let selection = Selection {
                        area: siz.windowed(window),
                        order: vec![0],
                        subset: false,
                    };
                    let tiles: Vec<Rect> = selection
                        .tiles(&siz)
                        .iter()
                        .enumerate()
                        .filter(|(_, read)| **read)
                        .map(|(index, _)| siz.tile_rect(index as u32).unwrap())
                        .collect();
                    let union = tiles.iter().fold(tiles[0], |u, t| Rect {
                        x0: u.x0.min(t.x0),
                        y0: u.y0.min(t.y0),
                        x1: u.x1.max(t.x1),
                        y1: u.y1.max(t.y1),
                    });
                    let area: u64 = tiles
                        .iter()
                        .map(|t| u64::from(t.x1 - t.x0) * u64::from(t.y1 - t.y0))
                        .sum();
                    let read = selection.read_rect(&siz);
                    assert_eq!(read, union, "window {window:?}");
                    // No gap: the tiles read fill their union.
                    assert_eq!(
                        area,
                        u64::from(read.x1 - read.x0) * u64::from(read.y1 - read.y0),
                        "window {window:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 500, "{checked}");
    }

    #[test]
    fn a_run_of_missing_tiles_is_split_at_the_tiles_not_read() {
        let tiles = [false, true, false, true, false, false];
        let entries = vec![
            ReportEntry::MissingTiles { tiles: 0..5 },
            ReportEntry::MissingTiles { tiles: 4..6 },
            ReportEntry::TilePartCountMismatch {
                tile: 2,
                declared: 2,
                found: 1,
                corrected: false,
            },
            ReportEntry::TilePartCountMismatch {
                tile: 3,
                declared: 2,
                found: 1,
                corrected: true,
            },
            ReportEntry::MissingEoc,
        ];
        assert_eq!(
            keep_entries(entries, &tiles),
            vec![
                ReportEntry::MissingTiles { tiles: 1..2 },
                ReportEntry::MissingTiles { tiles: 3..4 },
                ReportEntry::TilePartCountMismatch {
                    tile: 3,
                    declared: 2,
                    found: 1,
                    corrected: true,
                },
                ReportEntry::MissingEoc,
            ]
        );
    }
}
