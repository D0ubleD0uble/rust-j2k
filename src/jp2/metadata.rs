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
    /// The color space of the color channels.
    pub color: ColorSpace,
    /// One entry per [`Image::components`](crate::Image::components) entry,
    /// saying what that channel is: its entry in the file's channel
    /// definition (`cdef`) box, or, without one, color `i + 1` for channel
    /// `i` while `i` is below the color space's number of colors (3 for
    /// sRGB and sYCC, 1 for grayscale, the profile's for an ICC profile). A
    /// channel past that has no meaning in I.5.3.6 and is
    /// [`ChannelType::Unspecified`] with [`Association::None`]. When the count is
    /// unknown (the file gives no color space, the enumerated value is not
    /// one this decoder knows, or an ICC profile is too short to name its
    /// color space) every channel is a color.
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
    /// as coded for a file without a palette). The decode puts them in color
    /// order like this. Start with the channels in that order, so channel `c`
    /// is at position `c`. For each entry in file order whose type is
    /// [`ChannelType::Color`] and whose association is
    /// [`Association::Color`]`(n)`, find where its channel is now (earlier
    /// swaps may have moved it) and swap it with whatever is at position
    /// `n - 1`. Every other entry moves nothing. The channel a swap moves out
    /// of the way keeps its place in later lookups: it is found where it went.
    ///
    /// For example, three color entries `(channel 0, color 3)`,
    /// `(channel 1, color 2)`, `(channel 2, color 1)` give the order
    /// `[2, 1, 0]`: the first swap gives `[2, 1, 0]`, the second finds channel
    /// 1 already at position 1, and the third finds channel 2 already at
    /// position 0. Reading `Cn` as a position instead would swap 0 and 2, then
    /// 1 with itself, then 2 and 0 back, and give `[0, 1, 2]`, which is wrong.
    ///
    /// The committed `jp2_cdef_reversed.jp2` has exactly those entries. This
    /// applies the rule above to them and checks it against the decoder:
    ///
    /// ```
    /// use rust_j2k::{Association, ChannelType, DecodeOptions, decode_with};
    ///
    /// # let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jp2_cdef_reversed.jp2");
    /// # let bytes = std::fs::read(path)?;
    /// let decoded = decode_with(&bytes, &DecodeOptions::default())?;
    /// let coded = decode_with(&bytes, &DecodeOptions::default().with_raw_channels(true))?;
    /// let entries = decoded.jp2.as_ref().unwrap().channel_definitions.as_ref().unwrap();
    ///
    /// // The rule: swap each color entry's channel into position `n - 1`.
    /// let mut order: Vec<usize> = (0..coded.components.len()).collect();
    /// for entry in entries {
    ///     if let (ChannelType::Color, Association::Color(n)) =
    ///         (entry.definition.kind, entry.definition.association)
    ///     {
    ///         let at = order.iter().position(|&c| c == usize::from(entry.channel)).unwrap();
    ///         order.swap(at, usize::from(n) - 1);
    ///     }
    /// }
    /// assert_eq!(order, [2, 1, 0]);
    ///
    /// // Output position `p` holds the coded channel `order[p]`, and the
    /// // coded channels differ, so the check is not vacuous.
    /// assert_ne!(coded.components[0].samples, coded.components[2].samples);
    /// for (p, &c) in order.iter().enumerate() {
    ///     assert_eq!(decoded.components[p].samples, coded.components[c].samples);
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// A channel's label on [`channels`](Self::channels) is that of the last
    /// entry naming it.
    ///
    /// [`channels`](Self::channels) itself is only relabeled when the `cdef`
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
    /// is: one entry per output channel of a palettized decode, in the order
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
    /// Every top-level `xml ` box, verbatim and in file order, including
    /// those after the codestream. Not parsed.
    pub xml: Vec<XmlBox>,
    /// Every top-level `uuid` box, in file order, including those after the
    /// codestream. Not interpreted.
    pub uuid: Vec<UuidBox>,
    /// Every top-level intellectual property box (`jp2i`), verbatim and in
    /// file order, including those after the codestream. The standard leaves
    /// the format of these bytes open, so they are not parsed.
    /// `jp2i` boxes inside other superboxes are not collected.
    pub ipr: Vec<IprBox>,
    /// Every well-formed top-level UUID info box (`uinf`), in file order,
    /// including those after the codestream: a list of UUIDs and a URL where
    /// more about them can be found. The URL is not fetched. A malformed
    /// `uinf` is left out and never fails a decode, as OpenJPEG skips the box
    /// (see [`UuidInfo`]).
    pub uuid_info: Vec<UuidInfo>,
    /// The Image Header box's (I.5.3.1) fields that are not taken from the
    /// codestream.
    pub image_header: ImageHeader,
    /// Every Colour Specification box (`colr`, I.5.3.3) in file order, the
    /// ignored ones too: those in every `jp2h` box and those misplaced at the
    /// top level after the first `jp2h`, before the codestream. A `colr` after
    /// the codestream is not reported. Only the first whose method is 1 or 2
    /// sets [`color`](Self::color); the others are reported here and not
    /// applied.
    ///
    /// The boxes are reported as stored in the file, whatever
    /// [`DecodeOptions::color`](crate::DecodeOptions::color) is. Under
    /// [`ColorOutput::Srgb`](crate::ColorOutput::Srgb) the first may still
    /// hold the original ICC profile while [`color`](Self::color) is
    /// [`ColorSpace::Srgb`].
    pub color_specifications: Vec<ColorSpecification>,
    /// The File Type box (`ftyp`, I.5.2).
    pub file_type: FileType,
    /// The first top-level Reader Requirements box (`rreq`, ISO/IEC 15444-2
    /// M.11.1), before or after the codestream. `None` without one, and when
    /// the first is malformed (see [`ReaderRequirements`]).
    pub reader_requirements: Option<ReaderRequirements>,
}

/// The Image Header box's (`ihdr`, I.5.3.1) fields that are not taken from
/// the codestream, as stored. None of them changes the decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImageHeader {
    /// `C`: the compression type. The standard requires 7 (JPEG 2000).
    pub compression: u8,
    /// `UnkC`: 1 when the file does not know the color space, 0 when its
    /// color specification boxes give it accurately. Not consulted for
    /// [`Jp2Metadata::color`].
    pub unknown_color_space: u8,
    /// `IPR`: 1 when the file claims to hold intellectual property rights
    /// boxes, 0 when it does not. This is only the flag as stored: it is not
    /// checked against the boxes present, so it may disagree with
    /// [`Jp2Metadata::ipr`], which holds the `jp2i` boxes actually found.
    pub ipr_flag: u8,
}

/// One Colour Specification box (`colr`, I.5.3.3), as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ColorSpecification {
    /// `METH`: 1 for an enumerated color space, 2 for a restricted ICC
    /// profile. Other values are reserved by this standard (ISO/IEC 15444-2
    /// defines more), and such a box is ignored.
    pub method: u8,
    /// `PREC`: the precedence. The standard reserves it and requires 0, and
    /// a reader ignores it. ISO/IEC 15444-2 reads the byte as a signed value.
    pub precedence: u8,
    /// `APPROX`: the color space approximation. The standard requires 0, and
    /// a reader ignores it.
    pub approximation: u8,
    /// The bytes after `APPROX`, verbatim: `EnumCS` (four bytes, big-endian)
    /// for `METH` 1, the ICC profile for `METH` 2.
    pub payload: Vec<u8>,
}

/// The File Type box (`ftyp`, I.5.2), as stored. A file is read when `jp2 `
/// is its brand or in its compatibility list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileType {
    /// `BR`: the brand, `jp2 ` for a JP2 file.
    pub brand: [u8; 4],
    /// `MinV`: the minor version. The standard requires 0, and a reader
    /// ignores it.
    pub minor_version: u32,
    /// `CLi`: the compatibility list, in box order.
    pub compatibility: Vec<[u8; 4]>,
}

/// A Reader Requirements box (`rreq`, ISO/IEC 15444-2 M.11.1): the features
/// a reader must support to use the file, each with a mask saying which of
/// the file's requirement expressions it belongs to.
///
/// Every mask is `ML` bytes, as stored (big-endian). A box is reported when it
/// holds exactly the fields its counts give: `ML`, the two masks, `NSF` and
/// its standard features, `NVF` and its vendor features, and nothing after.
/// One that is cut short or has bytes left over is malformed: it is left out
/// of [`Jp2Metadata::reader_requirements`] and never fails a decode, as
/// OpenJPEG skips the box. The values are not checked (ISO/IEC 15444-2 allows
/// `ML` of 1, 2, 4 or 8) and nothing in the box changes the decode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReaderRequirements {
    /// `ML`: the length of each mask in bytes.
    pub mask_length: u8,
    /// `FUAM`: the fully-understand-aspects mask.
    pub fully_understand: Vec<u8>,
    /// `DCM`: the decode-completely mask.
    pub decode_completely: Vec<u8>,
    /// The `NSF` standard features (`SF` and `SM`), in box order.
    pub standard_features: Vec<StandardFeature>,
    /// The `NVF` vendor features (`VF` and `VM`), in box order.
    pub vendor_features: Vec<VendorFeature>,
}

/// One standard feature of a Reader Requirements box: `SF` and its `SM`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StandardFeature {
    /// `SF`: the feature, numbered as in ISO/IEC 15444-2.
    pub feature: u16,
    /// `SM`: the feature's mask.
    pub mask: Vec<u8>,
}

/// One vendor feature of a Reader Requirements box: `VF` and its `VM`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct VendorFeature {
    /// `VF`: the feature's UUID.
    pub id: [u8; 16],
    /// `VM`: the feature's mask.
    pub mask: Vec<u8>,
}

use super::palette::{ChannelMapping, Palette};

/// The resolutions a JP2 file's `res ` box gives, in grid points per meter.
///
/// The capture resolution (`resc`) is the one the image was digitized at; the
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

/// One resolution: grid points per meter in each direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Res {
    /// Vertical grid points per meter (`VRN`, `VRD`, `VRE`).
    pub vertical: Ratio,
    /// Horizontal grid points per meter (`HRN`, `HRD`, `HRE`).
    pub horizontal: Ratio,
}

/// A resolution as the file stores it: `numerator / denominator × 10^exponent`
/// grid points per meter. Kept as integers, so the value is exact;
/// [`Ratio::grid_points_per_meter`] gives it as a float.
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
    /// The resolution in grid points per meter, or `None` when the
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
    /// #     // Capture resolution 10000 / 254 points per meter, both ways.
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
    /// // 10000 / 254 is about 39.37 grid points per meter.
    /// let per_meter = capture.horizontal.grid_points_per_meter().unwrap();
    /// assert!((per_meter - 10000.0 / 254.0).abs() < 1e-9);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn grid_points_per_meter(self) -> Option<f64> {
        if self.denominator == 0 {
            return None;
        }
        Some(
            f64::from(self.numerator) / f64::from(self.denominator)
                * 10f64.powi(i32::from(self.exponent)),
        )
    }
}

/// An `xml ` box: XML text the file carries, which is not parsed.
///
/// The payload is the bytes after the box header, not checked as UTF-8 or XML.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct XmlBox {
    /// The box contents, verbatim.
    pub payload: Vec<u8>,
}

/// An intellectual property box (`jp2i`). The standard leaves the format of
/// its contents open, so they are not parsed.
///
/// The payload is the bytes after the box header, not checked in any way.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IprBox {
    /// The box contents, verbatim.
    pub payload: Vec<u8>,
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

/// A UUID info box (`uinf`): the UUIDs its UUID list box (`ulst`) names and
/// its data entry URL box (`url `), which says where more about them can be
/// found.
///
/// A `uinf` is reported when it is well formed: its contents are whole boxes,
/// exactly one `ulst` and exactly one `url ` in either order (other boxes
/// inside it are ignored), the `ulst` holds exactly the `NU` UUIDs its count
/// gives, and the `url ` holds at least its 4-byte version and flags. A `uinf`
/// that breaks any of these is left out of
/// [`Jp2Metadata::uuid_info`] as a whole; the decode goes on. The version,
/// flags and location are reported as stored, so a version or flags other
/// than 0 and a location that is not valid UTF-8 or lacks its terminating NUL
/// are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UuidInfo {
    /// The UUIDs from the `ulst` box (`ID0` to `ID(NU-1)`), in box order.
    pub ids: Vec<[u8; 16]>,
    /// `VERS` from the `url ` box; the standard requires 0.
    pub version: u8,
    /// `FLAG` from the `url ` box, as stored (big-endian); the standard
    /// requires 0.
    pub flags: [u8; 3],
    /// `LOC` from the `url ` box: the bytes after the version and flags,
    /// verbatim, including any terminating NUL. See
    /// [`location_str`](Self::location_str) for the URL as text.
    pub location: Vec<u8>,
}

impl UuidInfo {
    /// The location as a UTF-8 string, without its terminating NUL: `None`
    /// when the bytes are not valid UTF-8 or hold a NUL before the last byte.
    /// A location with no terminating NUL is read to its end.
    ///
    /// ```
    /// use rust_j2k::read_header;
    ///
    /// # fn jp2_with_uuid_info(codestream: &[u8]) -> Result<Vec<u8>, rust_j2k::Error> {
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
    /// #     let mut file = boxed(b"jP  ", &[0x0D, 0x0A, 0x87, 0x0A]);
    /// #     file.extend(boxed(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    /// #     let colr = boxed(b"colr", &[1, 0, 0, 0, 0, 0, 17]);
    /// #     file.extend(boxed(b"jp2h", &[boxed(b"ihdr", &ihdr), colr].concat()));
    /// #     // One UUID, and a URL with its terminating NUL.
    /// #     let ulst = boxed(b"ulst", &[&[0, 1][..], &[0x42; 16]].concat());
    /// #     let url = boxed(b"url ", b"\0\0\0\0https://example.com/ids\0");
    /// #     file.extend(boxed(b"uinf", &[ulst, url].concat()));
    /// #     file.extend(boxed(b"jp2c", codestream));
    /// #     Ok(file)
    /// # }
    /// # fn main() -> Result<(), rust_j2k::Error> {
    /// # let codestream = std::fs::read("tests/fixtures/derived_quant_lossy.j2k").unwrap();
    /// let bytes = jp2_with_uuid_info(&codestream)?;
    /// let jp2 = read_header(&bytes)?.jp2.expect("a JP2 file");
    /// let info = &jp2.uuid_info[0];
    /// assert_eq!(info.ids, [[0x42; 16]]);
    /// assert_eq!(info.location_str(), Some("https://example.com/ids"));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn location_str(&self) -> Option<&str> {
        let text = self.location.strip_suffix(&[0]).unwrap_or(&self.location);
        if text.contains(&0) {
            return None;
        }
        std::str::from_utf8(text).ok()
    }
}

/// The color space a JP2 file declares for its color channels.
///
/// It comes from the first usable Colour Specification (`colr`) box, the way
/// OpenJPEG reads it: a box whose method is neither 1 (enumerated) nor 2
/// (restricted ICC) is skipped and does not count as the first, and every
/// later usable box is ignored. Boxes placed after `jp2h` at the top level
/// count in file order with the ones inside `jp2h`. This is what the file
/// declares. Decoding returns the samples as coded unless the caller asks for
/// [`ColorOutput::Srgb`](crate::ColorOutput::Srgb); then the color channels
/// are converted and [`Jp2Metadata::color`] reports the converted space.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColorSpace {
    /// sRGB (enumerated color space 16).
    Srgb,
    /// Grayscale (enumerated color space 17).
    Grayscale,
    /// sYCC (enumerated color space 18).
    Sycc,
    /// A `METH` 2 box's ICC profile: the box's bytes after its three header
    /// bytes, verbatim. The standard restricts it to a monochrome or
    /// three-component matrix/TRC input profile, but that is not checked
    /// here: it holds whatever the box contains.
    /// Applied to the samples only under
    /// [`ColorOutput::Srgb`](crate::ColorOutput::Srgb), which then reports
    /// the converted color space instead.
    RestrictedIcc(Vec<u8>),
    /// Another enumerated color space value from the first usable box,
    /// reported but not interpreted.
    Enumerated(u32),
    /// The file gives no color space: no usable `colr` box. The image
    /// header's unknown-color-space flag
    /// ([`ImageHeader::unknown_color_space`]) does not change this.
    Unknown,
}

/// What one output channel is: its type and the color it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelDefinition {
    /// The channel's type.
    pub kind: ChannelType,
    /// Which color, if any, the channel is associated with.
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
    /// A color channel (`Typ` 0).
    Color,
    /// An opacity channel (`Typ` 1).
    Opacity,
    /// A premultiplied opacity channel (`Typ` 2).
    PremultipliedOpacity,
    /// The type is not specified (`Typ` 65535).
    Unspecified,
    /// A reserved `Typ` value, reported as is.
    Other(u16),
}

/// The color a channel is associated with, from a channel definition
/// (`Asoc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Association {
    /// The channel applies to the whole image (`Asoc` 0), as an opacity
    /// channel usually does.
    WholeImage,
    /// The channel is color `n` of the color space, counting from 1 (for
    /// sRGB, 1 is red, 2 green, 3 blue).
    Color(u16),
    /// No association (`Asoc` 65535).
    None,
}
