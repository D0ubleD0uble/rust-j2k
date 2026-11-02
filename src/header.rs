//! What [`read_header`](crate::read_header) returns: an image's shape and
//! metadata, read without decoding any tile data.

use crate::codestream::markers::Siz;
use crate::jp2::Jp2Metadata;

/// An image's header: what [`decode`](crate::decode) would return, without
/// the samples.
///
/// The struct is `#[non_exhaustive]`, so more of the header can be added
/// without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Header {
    /// The image area's width on the reference grid, as
    /// [`Image::width`](crate::Image::width).
    pub width: u32,
    /// The image area's height on the reference grid, as
    /// [`Image::height`](crate::Image::height).
    pub height: u32,
    /// One entry per [`Image::components`](crate::Image::components) entry,
    /// in the same order: for a palettised JP2 file, the palette's channels.
    pub components: Vec<ComponentInfo>,
    /// What [`decode`](crate::decode) would put on
    /// [`Image::jp2`](crate::Image::jp2): `None` for a raw codestream.
    pub jp2: Option<Jp2Metadata>,
}

/// One output channel's sample format and size, as on the
/// [`Component`](crate::Component) that [`decode`](crate::decode) would
/// return.
///
/// The struct is `#[non_exhaustive]`, so more of a channel's description can
/// be added without a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentInfo {
    /// Bits per sample.
    pub bit_depth: u8,
    /// Whether the samples are signed.
    pub signed: bool,
    /// Horizontal sub-sampling factor on the reference grid; `1` is none.
    pub x_sampling: u8,
    /// Vertical sub-sampling factor on the reference grid; `1` is none.
    pub y_sampling: u8,
    /// The channel's width in samples, as
    /// [`Component::width`](crate::Component::width): `ceil(x1 / dx) -
    /// ceil(x0 / dx)` for the image area `x0..x1` and the sub-sampling `dx`,
    /// then halved (rounding up) once per level of resolution reduction. It
    /// is not `ceil(width / dx)`: that is off by one when the image area
    /// starts at an odd offset.
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
