//! What [`read_header`](crate::read_header) returns: an image's shape and
//! metadata, read without decoding any tile data.

use crate::Window;
use crate::codestream::markers::Siz;
use crate::jp2::Jp2Metadata;

/// An image's header: what [`decode`](crate::decode) would return, without
/// the samples.
///
/// The fields describe two things. [`width`](Self::width),
/// [`height`](Self::height), [`components`](Self::components) and
/// [`jp2`](Self::jp2) describe the image a decode under the same options
/// returns, so under [`read_header_with`](crate::read_header_with) they
/// follow the options: the reduction, window, tile, component subset,
/// [`raw_channels`](crate::DecodeOptions::raw_channels) and
/// [`color`](crate::DecodeOptions::color).
/// [`image_area`](Self::image_area), [`tiles`](Self::tiles),
/// [`max_reduction`](Self::max_reduction) and
/// [`quality_layers`](Self::quality_layers) describe the file's main header,
/// and are the same whatever the options.
///
/// The struct is `#[non_exhaustive]`, so more of the header can be added
/// without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Header {
    /// The image area's width, as [`Image::width`](crate::Image::width): on
    /// the reduced grid under a resolution reduction.
    pub width: u32,
    /// The image area's height, as [`Image::height`](crate::Image::height):
    /// on the reduced grid under a resolution reduction.
    pub height: u32,
    /// One entry per [`Image::components`](crate::Image::components) entry,
    /// in the same order: for a palettized JP2 file, the palette's channels.
    /// That is not the number of components the decode holds: it decodes
    /// every component the codestream codes, mapped or not. To bound a
    /// decode's samples, set
    /// [`sample_limit`](crate::DecodeOptions::sample_limit) to
    /// [`SampleLimit::AtMost`](crate::SampleLimit::AtMost) rather than summing
    /// these.
    pub components: Vec<ComponentInfo>,
    /// The largest
    /// [`resolution_reduction`](crate::DecodeOptions::resolution_reduction)
    /// the main header allows: the fewest decomposition levels any component
    /// has in the main header's `COD` and `COC` markers. A larger reduction is
    /// [`Error::InvalidOptions`](crate::Error::InvalidOptions). A tile-part
    /// `COD` or `COC` with fewer levels can still refuse a reduction up to
    /// this one, but only the decode finds that.
    pub max_reduction: u8,
    /// The number of quality layers the main header's `COD` marker declares
    /// (`SGcod`, at least 1). A
    /// [`max_quality_layers`](crate::DecodeOptions::max_quality_layers) cap
    /// (set with
    /// [`with_max_quality_layers`](crate::DecodeOptions::with_max_quality_layers))
    /// at or past it decodes every layer of a tile that keeps this `COD`. A
    /// tile-part `COD` can declare another count for its tile, and only the
    /// decode reads that.
    pub quality_layers: u16,
    /// The image area on the reference grid, as the `SIZ` marker declares
    /// it: `XOsiz..Xsiz` by `YOsiz..Ysiz`, at full resolution. A
    /// [`Region::Window`](crate::Region::Window) is in the same coordinates,
    /// so a window of the image's top-left 64 x 64 samples starts at
    /// `(image_area.x0, image_area.y0)`, not at `(0, 0)`.
    pub image_area: Window,
    /// The tile grid the `SIZ` marker declares. Its
    /// [`count`](TileGrid::count) is how many tiles
    /// [`Region::Tile`](crate::Region::Tile) can name: `0` to `count() - 1`.
    pub tiles: TileGrid,
    /// What [`decode`](crate::decode) would put on
    /// [`Image::jp2`](crate::Image::jp2): `None` for a raw codestream.
    pub jp2: Option<Jp2Metadata>,
}

/// How an image is cut into tiles: the grid that the `SIZ` marker declares
/// on the reference grid, the coordinate space of the whole codestream.
///
/// Every tile is [`tile_width`](Self::tile_width) by
/// [`tile_height`](Self::tile_height), and the grid starts at
/// ([`x_offset`](Self::x_offset), [`y_offset`](Self::y_offset)). Tiles at the
/// edges are cut to the image area, so they can be smaller. They are
/// numbered in raster order from 0, as [`Region::Tile`](crate::Region::Tile)
/// numbers them: tile `n` is in column `n % columns` and row `n / columns`.
/// An image of one tile has a grid of 1 by 1.
///
/// The grid follows ISO/IEC 15444-1 B.3: `columns` is `ceil((Xsiz - XTOsiz)
/// / XTsiz)` (Eq. B-5), `rows` the same down, and [`tile`](Self::tile) gives
/// each tile's rectangle (Eq. B-7). On ordinary files the values match what
/// `opj_dump` prints as `tw`, `th`, `tdx`, `tdy`, `tx0` and `ty0`.
///
/// ```
/// use rust_j2k::{DecodeOptions, Region, decode_with, read_header};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let bytes = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/window/tiled_53.j2k"
/// # )).to_vec();
/// # /*
/// let bytes = std::fs::read("image.j2k")?;
/// # */
/// let tiles = read_header(&bytes)?.tiles;
/// for index in 0..tiles.count() {
///     let area = tiles.tile(index).expect("the index is below the count");
///     let options = DecodeOptions::default().with_region(Region::Tile(index));
///     let tile = decode_with(&bytes, &options)?;
///     assert_eq!((tile.width, tile.height), (area.x1 - area.x0, area.y1 - area.y0));
///     println!("tile {index} at ({}, {}): {}x{}", area.x0, area.y0, tile.width, tile.height);
/// }
/// # assert_eq!((tiles.columns, tiles.rows), (3, 3));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct TileGrid {
    /// Tiles across.
    pub columns: u32,
    /// Tiles down.
    pub rows: u32,
    /// The width of a tile before the cut to the image area (`XTsiz`).
    pub tile_width: u32,
    /// The height of a tile before the cut to the image area (`YTsiz`).
    pub tile_height: u32,
    /// The left edge of the first column of tiles on the reference grid
    /// (`XTOsiz`). It is at or left of the image area's left edge.
    pub x_offset: u32,
    /// The top edge of the first row of tiles on the reference grid
    /// (`YTOsiz`). It is at or above the image area's top edge.
    pub y_offset: u32,
    /// The image area, which cuts the edge tiles.
    image: Window,
}

impl TileGrid {
    /// How many tiles the image has: `columns * rows`, at most 65535.
    #[must_use]
    pub const fn count(&self) -> u32 {
        self.columns.saturating_mul(self.rows)
    }

    /// Tile `index`'s rectangle on the reference grid, cut to the image
    /// area: the area [`Region::Tile`](crate::Region::Tile)`(index)` decodes.
    /// `None` when `index` is at or past [`count`](Self::count).
    ///
    /// Column `p = index % columns` and row `q = index / columns` span
    /// `max(x_offset + p * tile_width, x0)..min(x_offset + (p + 1) *
    /// tile_width, x1)` across, for the image area `x0..x1`, and the same
    /// down (ISO/IEC 15444-1 Eq. B-7).
    #[must_use]
    pub fn tile(&self, index: u32) -> Option<Window> {
        if index >= self.count() {
            return None;
        }
        let (p, q) = (
            u64::from(index % self.columns),
            u64::from(index / self.columns),
        );
        let (w, h) = (u64::from(self.tile_width), u64::from(self.tile_height));
        let (xo, yo) = (u64::from(self.x_offset), u64::from(self.y_offset));
        let image = self.image;
        // Each end is clamped into the image area, which is u32.
        let clamp = |v: u64, lo: u32, hi: u32| v.clamp(u64::from(lo), u64::from(hi)) as u32;
        Some(Window::new(
            clamp(xo + p * w, image.x0, image.x1),
            clamp(yo + q * h, image.y0, image.y1),
            clamp(xo + (p + 1) * w, image.x0, image.x1),
            clamp(yo + (q + 1) * h, image.y0, image.y1),
        ))
    }

    /// The grid `siz` declares.
    pub(crate) fn of_siz(siz: &Siz) -> Self {
        let (columns, rows) = siz.tile_grid();
        TileGrid {
            columns,
            rows,
            tile_width: siz.tile_width,
            tile_height: siz.tile_height,
            x_offset: siz.tile_x_offset,
            y_offset: siz.tile_y_offset,
            image: image_area(siz),
        }
    }
}

/// The image area `siz` declares.
pub(crate) fn image_area(siz: &Siz) -> Window {
    Window::new(siz.x_offset, siz.y_offset, siz.x_size, siz.y_size)
}

/// One output channel's sample format and size, as on the
/// [`Component`](crate::Component) that [`decode`](crate::decode) would
/// return.
///
/// The struct is `#[non_exhaustive]`, so more of a channel's description can
/// be added without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentInfo {
    /// Bits per sample: 1 to 31 for unsigned samples, 1 to 32 for signed
    /// ones, as [`Component::bit_depth`](crate::Component::bit_depth).
    pub bit_depth: u8,
    /// Whether the samples are signed.
    pub signed: bool,
    /// Horizontal sub-sampling factor on the reference grid; `1` means no
    /// sub-sampling.
    pub x_sampling: u8,
    /// Vertical sub-sampling factor on the reference grid; `1` means no
    /// sub-sampling.
    pub y_sampling: u8,
    /// The channel's width in samples, as
    /// [`Component::width`](crate::Component::width): `ceil(x1 / dx) -
    /// ceil(x0 / dx)` for the image area `x0..x1` and the sub-sampling `dx`.
    /// Under a resolution reduction of `r` levels each end is reduced on its
    /// own, as OpenJPEG has it: `ceil(ceil(x1 / dx) / 2^r) - ceil(ceil(x0 /
    /// dx) / 2^r)`. Neither is `ceil(width / dx)`, nor the full width halved:
    /// those are off by one when the image area starts at an odd offset.
    pub width: u32,
    /// The channel's height in samples, as
    /// [`Component::height`](crate::Component::height); see
    /// [`width`](Self::width).
    pub height: u32,
}

impl ComponentInfo {
    /// A channel of this format and sub-sampling, sized from `siz` at a
    /// resolution reduction. A zero sub-sampling factor, which the parser
    /// rejects, gives an empty channel.
    pub(crate) fn on_grid(
        siz: &Siz,
        bit_depth: u8,
        signed: bool,
        x_sampling: u8,
        y_sampling: u8,
        reduction: u8,
    ) -> Self {
        let (width, height) = siz
            .sampled_extent_at(x_sampling, y_sampling, reduction)
            .unwrap_or((0, 0));
        ComponentInfo {
            bit_depth,
            signed,
            x_sampling,
            y_sampling,
            width,
            height,
        }
    }

    /// The codestream's components as coded, sized at a resolution reduction.
    pub(crate) fn of_siz(siz: &Siz, reduction: u8) -> Vec<Self> {
        siz.components
            .iter()
            .map(|c| {
                Self::on_grid(
                    siz,
                    c.bit_depth,
                    c.signed,
                    c.x_sampling,
                    c.y_sampling,
                    reduction,
                )
            })
            .collect()
    }
}
