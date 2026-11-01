//! What a JP2 file says about its samples, as public plain data on
//! [`Image::jp2`](crate::Image::jp2).

/// What the JP2 wrapper said about the decoded samples.
///
/// `None` on [`Image::jp2`](crate::Image::jp2) for a raw codestream. The
/// struct is `#[non_exhaustive]`, so more of the file's metadata can be added
/// without a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Jp2Metadata {
    /// The colour space of the colour channels.
    pub colour: ColourSpace,
    /// One entry per [`Image::components`](crate::Image::components) entry,
    /// saying what that channel is: its entry in the file's channel
    /// definition (`cdef`) box, or, without one, colour `i + 1` for channel
    /// `i`.
    pub channels: Vec<ChannelDefinition>,
    /// Whether the samples went through the file's palette. `false` when the
    /// file has no palette (or a `pclr` without a `cmap`), or the caller set
    /// [`DecodeOptions::raw_channels`](crate::DecodeOptions::raw_channels).
    pub palette_applied: bool,
}

/// The colour space a JP2 file declares for its colour channels.
///
/// It comes from the first usable Colour Specification (`colr`) box, the way
/// OpenJPEG reads it: a box whose method is neither 1 (enumerated) nor 2
/// (restricted ICC) is skipped and does not count as the first, and every
/// later usable box is ignored. Boxes placed after `jp2h` at the top level
/// count in file order with the ones inside `jp2h`. The samples are never converted, whatever
/// the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColourSpace {
    /// sRGB (enumerated colour space 16).
    Srgb,
    /// Greyscale (enumerated colour space 17).
    Greyscale,
    /// sYCC (enumerated colour space 18).
    Sycc,
    /// A restricted ICC profile (a monochrome or three-component matrix/TRC
    /// input profile), the box's bytes after its three header bytes, verbatim.
    /// Not checked and not applied to the samples.
    RestrictedIcc(Vec<u8>),
    /// Another enumerated colour space value from the first usable box,
    /// reported but not interpreted.
    Enumerated(u32),
    /// The file gives no colour space: no usable `colr` box. The image
    /// header's unknown-colour-space flag does not change this.
    Unknown,
}

/// What one output channel is: its type and the colour it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelDefinition {
    /// The channel's type.
    pub kind: ChannelType,
    /// Which colour, if any, the channel is associated with.
    pub association: Association,
}

/// A channel's type, from a channel definition (`Typ`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelType {
    /// A colour channel (`Typ` 0).
    Colour,
    /// An opacity channel (`Typ` 1).
    Opacity,
    /// A premultiplied opacity channel (`Typ` 2).
    PremultipliedOpacity,
    /// The type is not specified (`Typ` 65535).
    Unspecified,
    /// A reserved `Typ` value, reported as is.
    Other(u16),
}

/// The colour a channel is associated with, from a channel definition
/// (`Asoc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Association {
    /// The channel applies to the whole image (`Asoc` 0), as an opacity
    /// channel usually does.
    WholeImage,
    /// The channel is colour `n` of the colour space, counting from 1 (for
    /// sRGB, 1 is red, 2 green, 3 blue).
    Colour(u16),
    /// No association (`Asoc` 65535).
    None,
    /// An `Asoc` value this decoder does not interpret, reported as is.
    Other(u16),
}
