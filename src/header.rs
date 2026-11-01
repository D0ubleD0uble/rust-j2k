//! What [`read_header`](crate::read_header) returns: an image's shape and
//! metadata, read without decoding any tile data.

use crate::codestream::markers::SizComponent;
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

/// One output channel's sample format, as on the
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
}

impl From<&SizComponent> for ComponentInfo {
    fn from(component: &SizComponent) -> Self {
        ComponentInfo {
            bit_depth: component.bit_depth,
            signed: component.signed,
            x_sampling: component.x_sampling,
            y_sampling: component.y_sampling,
        }
    }
}
