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
    /// The file's channel definition box (`cdef`) as written, in file order:
    /// `Some` whenever the file has one, whatever the options, and `None`
    /// without one. The entries are checked only when the decode applies them,
    /// so under [`raw_channels`](crate::DecodeOptions::raw_channels) a `cn`
    /// may name a channel the file does not have.
    ///
    /// The channels an entry's [`channel`](ChannelDefinitionEntry::channel)
    /// counts are the file's components after the palette is expanded, in the
    /// order [`channel_mapping`](Self::channel_mapping) gives (the components
    /// as coded for a file without a palette). The decode puts them in colour
    /// order like this: take the channels in that order; for each entry in
    /// file order whose type is [`ChannelType::Colour`] and whose association
    /// is [`Association::Colour`]`(n)`, swap its channel with the channel now
    /// at position `n - 1`. Every other entry moves nothing. A channel's label
    /// on [`channels`](Self::channels) is that of the last entry naming it.
    ///
    /// [`channels`](Self::channels) itself is only relabelled when the `cdef`
    /// applies: under `raw_channels` every channel keeps its default label.
    pub channel_definitions: Option<Vec<ChannelDefinitionEntry>>,
    /// The file's palette, from its `pclr` box. Present when the palette
    /// applies, and also when
    /// [`raw_channels`](crate::DecodeOptions::raw_channels) left it unapplied
    /// (then [`palette_applied`](Self::palette_applied) is `false`). `None`
    /// without a `pclr`. `None` too for a `pclr` without a `cmap`: the
    /// standard requires both or neither, so the file is invalid, and OpenJPEG
    /// ignores the palette in that case, as this decoder does.
    pub palette: Option<Palette>,
    /// The file's component mapping, set whenever [`palette`](Self::palette)
    /// is: one entry per output channel of a palettised decode, in the order
    /// before any `cdef` reordering. An entry is
    /// [`Direct`](crate::MappingKind::Direct) for a component used as coded.
    ///
    /// It is the mapping the default decode applies, in either mode, including
    /// OpenJPEG's correction for a single-component file (which turns every
    /// entry into a lookup in its own column, so it can differ from the box).
    /// Only a `cmap` the default decode refuses is reported as written, and
    /// only under `raw_channels`, which does not check it; an `MTYP` other
    /// than 0 or 1 is then [`Reserved`](crate::MappingKind::Reserved).
    pub channel_mapping: Option<Vec<ChannelMapping>>,
    /// The capture and default display resolutions from the header's `res `
    /// box, each `None` when the file does not give it.
    pub resolution: Resolution,
    /// The contents of every top-level `xml ` box, verbatim and in file
    /// order, including those after the codestream. Not parsed.
    pub xml: Vec<Vec<u8>>,
    /// Every top-level `uuid` box, in file order, including those after the
    /// codestream. Not interpreted.
    pub uuid: Vec<UuidBox>,
}

use super::palette::{ChannelMapping, Palette};

/// The resolutions a JP2 file's `res ` box gives, in grid points per metre.
///
/// The capture resolution (`resc`) is the one the image was digitised at; the
/// default display resolution (`resd`) is the one it should be shown at. A
/// box that is missing, or that does not hold exactly its ten bytes, leaves
/// its field `None`: OpenJPEG does not read `res ` at all, so a malformed one
/// never fails a decode. When more than one is present, the first counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Resolution {
    /// The capture resolution (`resc`).
    pub capture: Option<Res>,
    /// The default display resolution (`resd`).
    pub display: Option<Res>,
}

/// One resolution: grid points per metre in each direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Res {
    /// Vertical grid points per metre (`VRN`, `VRD`, `VRE`).
    pub vertical: Ratio,
    /// Horizontal grid points per metre (`HRN`, `HRD`, `HRE`).
    pub horizontal: Ratio,
}

/// A resolution as the file stores it: `numerator / denominator × 10^exponent`
/// grid points per metre. Kept as integers, so the value is exact;
/// [`Ratio::grid_points_per_metre`] gives it as a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ratio {
    /// `N`.
    pub numerator: u16,
    /// `D`.
    pub denominator: u16,
    /// `E`, a power of ten.
    pub exponent: i8,
}

impl Ratio {
    /// The resolution in grid points per metre, or `None` when the
    /// denominator is 0.
    ///
    /// ```
    /// use rust_j2k::read_header;
    ///
    /// # fn jp2_with_resolution(codestream: &[u8]) -> Result<Vec<u8>, rust_j2k::Error> {
    /// #     fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    /// #         let mut out = (8 + payload.len() as u32).to_be_bytes().to_vec();
    /// #         out.extend_from_slice(kind);
    /// #         out.extend_from_slice(payload);
    /// #         out
    /// #     }
    /// #     let header = read_header(codestream)?;
    /// #     let mut ihdr = header.height.to_be_bytes().to_vec();
    /// #     ihdr.extend_from_slice(&header.width.to_be_bytes());
    /// #     ihdr.extend_from_slice(&(header.components.len() as u16).to_be_bytes());
    /// #     ihdr.extend_from_slice(&[header.components[0].bit_depth - 1, 7, 0, 0]);
    /// #     // Capture resolution 10000 / 254 points per metre, both ways.
    /// #     let mut resc = Vec::new();
    /// #     for value in [10000u16, 254, 10000, 254] {
    /// #         resc.extend_from_slice(&value.to_be_bytes());
    /// #     }
    /// #     resc.extend_from_slice(&[0, 0]);
    /// #     let mut file = boxed(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]);
    /// #     file.extend(boxed(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    /// #     let colr = boxed(b"colr", &[1, 0, 0, 0, 0, 0, 17]);
    /// #     let res = boxed(b"res ", &boxed(b"resc", &resc));
    /// #     file.extend(boxed(b"jp2h", &[boxed(b"ihdr", &ihdr), colr, res].concat()));
    /// #     file.extend(boxed(b"jp2c", codestream));
    /// #     Ok(file)
    /// # }
    /// # fn main() -> Result<(), rust_j2k::Error> {
    /// # let codestream = std::fs::read("tests/fixtures/derived_quant_lossy.j2k").unwrap();
    /// let bytes = jp2_with_resolution(&codestream)?;
    /// let jp2 = read_header(&bytes)?.jp2.expect("a JP2 file");
    /// let capture = jp2.resolution.capture.expect("a capture resolution");
    /// // 10000 / 254 points per metre is one per 25.4 mm: 393.7... per millimetre.
    /// let per_metre = capture.horizontal.grid_points_per_metre().unwrap();
    /// assert!((per_metre - 10000.0 / 254.0).abs() < 1e-9);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn grid_points_per_metre(self) -> Option<f64> {
        if self.denominator == 0 {
            return None;
        }
        Some(
            f64::from(self.numerator) / f64::from(self.denominator)
                * 10f64.powi(i32::from(self.exponent)),
        )
    }
}

/// A `uuid` box: a 16-byte identifier and the data it labels.
///
/// GeoJP2 georeferencing and XMP are carried this way. A `uuid` box shorter
/// than its 16-byte identifier is skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidBox {
    /// The identifier, as stored.
    pub id: [u8; 16],
    /// The box contents after the identifier, verbatim.
    pub payload: Vec<u8>,
}

/// The colour space a JP2 file declares for its colour channels.
///
/// It comes from the first usable Colour Specification (`colr`) box, the way
/// OpenJPEG reads it: a box whose method is neither 1 (enumerated) nor 2
/// (restricted ICC) is skipped and does not count as the first, and every
/// later usable box is ignored. Boxes placed after `jp2h` at the top level
/// count in file order with the ones inside `jp2h`. This is what the file
/// declares. Decoding returns the samples as coded unless the caller asks for
/// [`ColourOutput::Srgb`](crate::ColourOutput::Srgb); then the colour channels
/// are converted and [`Jp2Metadata::colour`] reports the converted space.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColourSpace {
    /// sRGB (enumerated colour space 16).
    Srgb,
    /// Greyscale (enumerated colour space 17).
    Greyscale,
    /// sYCC (enumerated colour space 18).
    Sycc,
    /// A restricted ICC profile (a monochrome or three-component matrix/TRC
    /// input profile), the box's bytes after its three header bytes, verbatim.
    /// Applied to the samples only under
    /// [`ColourOutput::Srgb`](crate::ColourOutput::Srgb), which then reports
    /// the converted colour space instead.
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

/// One entry of a file's channel definition box (`cdef`), as written: the
/// channel it describes and that channel's definition.
///
/// [`Jp2Metadata::channels`] has the labels of the output channels in output
/// order; these entries are indexed by `Cn`, the channel's position before
/// the `cdef` box reorders anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChannelDefinitionEntry {
    /// `Cn`: the channel described, counting the components after palette
    /// expansion (the codestream's components for a file without a palette).
    pub channel: u16,
    /// `Typ` and `Asoc`.
    pub definition: ChannelDefinition,
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
}
