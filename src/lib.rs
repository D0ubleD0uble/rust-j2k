//! Pure-Rust JPEG 2000 codec, built GRIB2-decode-first toward OpenJPEG-level
//! coverage. No C dependency, so it cross-compiles cleanly to every target.
//!
//! The decoder reads a **raw codestream** (ISO/IEC 15444-1 Annex A) or a
//! **JP2 file** (Annex I) into **integer** samples: one grid per component,
//! signed up to 32 bits, unsigned up to 31 (an `i32` container), each with its
//! own bit depth, sign, and sub-sampling. Both wavelet paths decode — the
//! reversible 5/3 bit-exactly, the irreversible 9/7 within the standard's
//! compliance bounds — as do tiling and tile-parts, precincts, quality layers,
//! all five progression orders and POC changes between them, packed packet
//! headers (PPM/PPT), every Part 1 code-block coding style, region of interest
//! (maxshift), non-zero canvas offsets, resolution-reduced decoding, and the
//! reversible (RCT) and irreversible (ICT) colour transforms.
//!
//! A JP2 file's codestream decodes the same way, and the file's header comes
//! back on [`Image::jp2`]. A palettised JP2 file decodes to the palette's
//! channels, and a file with channel definitions puts its colour channels in
//! colour order. [`read_header`] reads the same header without decoding any
//! tile data. Samples come out as coded by default;
//! [`ColourOutput::Srgb`] converts sYCC and restricted ICC profiles to sRGB.
//! HTJ2K and an encoder are later work, not permanent non-goals.
//! See `docs/roadmap.md` and `docs/scope.md`.
//!
//! # Pipeline
//!
//! ```text
//! bytes ─▶ [jp2 boxes] ─▶ codestream ─▶ tier-2 ─▶ tier-1 ─▶ dequant ─▶ inverse DWT ─▶ Image
//!          (Annex I)      (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)
//! ```
//!
//! Each stage is a module below. [`decode`] wires them together.
//!
//! # Example
//!
//! The core of the public surface is [`decode`]: codestream or JP2 bytes in,
//! an [`Image`] out. An [`Image`] is the image area on the reference grid plus
//! one [`Component`] per output channel: per codestream component, or per
//! palette channel for a palettised JP2 file. A sub-sampled component holds
//! fewer samples than that area, so size sample buffers from the component,
//! not from the image. Malformed input never panics; it comes back as a typed
//! [`Error`].
//!
//! ```
//! use rust_j2k::{decode, Error};
//!
//! // In real use these are the bytes of a `.j2k` codestream, or the GRIB2 §7
//! // data section of a `grid_jpeg` message. Invalid input is rejected cleanly:
//! match decode(b"not a codestream") {
//!     Ok(image) => {
//!         let first = image.component(0).expect("at least one component");
//!         println!(
//!             "image area {}x{}, first component {}x{}",
//!             image.width, image.height, first.width, first.height,
//!         );
//!     }
//!     Err(Error::Unsupported(what)) => println!("outside the decoded subset: {what}"),
//!     Err(e) => println!("decode failed: {e}"),
//! }
//! ```
//!
//! # Status
//!
//! All 23 entries of the ISO/IEC 15444-4 conformance suite decode within their
//! class-1 error bounds, and that gate runs in `tests/` against committed
//! oracle snapshots, with no external tools. Correctness is defined by
//! cross-checking a reference decoder (OpenJPEG / eccodes), not by
//! self-consistency. An input using a feature outside the decoded set — HTJ2K,
//! a Part 2 extension — is rejected with
//! [`Error::Unsupported`], never half-decoded. GRIB2 §5.40 (`grid_jpeg`)
//! payloads, the crate's original target, are raw codestreams and decode
//! directly. See each module's docs for the ISO §reference and what it owns;
//! the feature map is in `docs/scope.md`.
#![warn(missing_docs)]

// The pipeline modules are crate-internal: the public API is `decode`,
// `read_header`, their options and output types, `Error`, and `Result`. Keeping
// the stages private lets each one evolve freely
// (the roadmap widens all of them) without churning the crate's committed
// surface, and keeps the docs.rs page to what a caller can actually use.
pub(crate) mod codestream;
pub(crate) mod dwt;
pub(crate) mod error;
pub(crate) mod header;
pub(crate) mod image;
pub(crate) mod jp2;
pub(crate) mod mct;
pub(crate) mod pipeline;
pub(crate) mod quant;
pub(crate) mod tier1;
pub(crate) mod tier2;

// Structured fuzz entry points for the detached fuzz/ workspace. Compiled
// unconditionally so that workspace builds under plain cargo (rust-analyzer,
// `cargo check` in fuzz/), but hidden from docs and exempt from semver: this
// is a test seam, not public API.
#[doc(hidden)]
pub mod fuzz;

pub use error::{Error, Result};
pub use header::{ComponentInfo, Header};
pub use image::{Component, Image};
pub use jp2::{
    Association, ChannelDefinition, ChannelDefinitionEntry, ChannelMapping, ChannelType,
    ColourSpace, Jp2Metadata, MappingKind, Palette, PaletteColumn, Ratio, Res, Resolution, UuidBox,
};

/// Decode a JPEG 2000 **codestream** (Annex A) or **JP2 file** (Annex I)
/// into an [`Image`]: the reference-grid image area plus one [`Component`] per
/// output channel, each on its own sample grid. For a raw codestream that is
/// one per SIZ component, in SIZ order. A JP2 file can change the channels:
/// its palette and channel definitions (below), and the sRGB output option.
///
/// The input kind is detected from its first bytes. Input that opens with the
/// `SOC` marker (`FF 4F`) is a codestream. Otherwise, input whose bytes 4..8
/// are `jP  ` (the signature box's type) is a JP2 file, and anything else goes
/// to the codestream parser, which rejects it for its missing `SOC`. A JP2 file
/// decodes its codestream (the first `jp2c` box) and reports the file's header
/// on [`Image::jp2`], which is `None` for a raw codestream.
///
/// A JP2 file with a palette (`pclr` and `cmap`) decodes to the palette's
/// channels, one per palette column, so it can have more components than its
/// codestream. A `pclr` without a `cmap` is ignored. A file with channel
/// definitions (`cdef`) moves each colour channel to its colour's position
/// (colour 1 first) and labels every channel on [`Jp2Metadata::channels`].
/// [`DecodeOptions::raw_channels`] skips both and returns the codestream's
/// components as coded.
///
/// ```no_run
/// use rust_j2k::{decode, ColourSpace};
///
/// // A `.jp2` file decodes the same way as a raw codestream; the format is
/// // detected from the first bytes.
/// let bytes = std::fs::read("photo.jp2")?;
/// let image = decode(&bytes)?;
/// if let Some(jp2) = &image.jp2 {
///     if jp2.colour == ColourSpace::Srgb {
///         println!("{} sRGB channels", image.components.len());
///     }
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// This is the whole public surface for the GRIB2 use case: the §7 data
/// section of a `grid_jpeg` message is a raw codestream. That subset is
/// single-component, so its images carry one component; a codestream declaring
/// more decodes each component onto its own grid, independently.
///
/// A malformed JP2 box structure is [`Error::FileFormat`]. A file that is not
/// JP2-compatible is valid JPEG 2000 but is rejected with
/// [`Error::Unsupported`]. Anything else outside the decoded subset is
/// rejected the same way, never half-decoded.
pub fn decode(bytes: &[u8]) -> Result<Image> {
    decode_with(bytes, &DecodeOptions::default())
}

/// Read a JPEG 2000 codestream's or JP2 file's [`Header`] without decoding any
/// tile data: the image area, each output channel's sample format, and, for a
/// JP2 file, the same [`Jp2Metadata`] that [`decode`] would put on
/// [`Image::jp2`].
///
/// This is [`read_header_with`] at the default [`DecodeOptions`], so a fault in
/// what it reads is the same [`Error`] that [`decode`] returns for that input.
/// Use [`read_header_with`] to read the header for other options.
///
/// ```
/// use rust_j2k::{read_header, Error};
///
/// // In real use these are a codestream's or JP2 file's bytes; the tile data
/// // may be missing. Invalid input is the same typed error `decode` gives.
/// match read_header(b"not a codestream") {
///     Ok(header) => println!("{}x{}, {} channels", header.width, header.height, header.components.len()),
///     Err(Error::Codestream(why)) => println!("malformed: {why}"),
///     Err(e) => println!("refused: {e}"),
/// }
/// ```
pub fn read_header(bytes: &[u8]) -> Result<Header> {
    read_header_with(bytes, &DecodeOptions::default())
}

/// Read a JPEG 2000 codestream's or JP2 file's [`Header`] without decoding any
/// tile data, for the same `options` as [`decode_with`]: the header describes
/// the [`Image`] that `decode_with(bytes, options)` would return.
///
/// The input is detected as [`decode`] detects it. The read covers a JP2
/// file's boxes and the codestream's main header, up to the first tile-part
/// (`SOT` marker), and stops there, so a file whose tile data is cut short or
/// missing still has a readable header. Nothing is allocated for the samples.
///
/// A fault in what it reads is the same [`Error`] that `decode_with(bytes,
/// options)` returns for that input, since `decode_with` runs the same checks
/// first. The options matter: a `cmap` that
/// [`raw_channels`](DecodeOptions::raw_channels) does not check, or a palette
/// that fits the sample budget once
/// [`resolution_reduction`](DecodeOptions::resolution_reduction) halves the
/// image, is a fault under some options and not others. A fault in the tile
/// data (a broken tile-part, a tile header that changes a component's
/// wavelet, a reduction that a tile-part COD or COC with fewer levels cannot
/// take) is only found by decoding. A reduction that the main header's own
/// levels cannot take is [`Error::InvalidOptions`] here, as OpenJPEG has it.
///
/// ```
/// use rust_j2k::{read_header_with, DecodeOptions};
///
/// let options = DecodeOptions::default().with_resolution_reduction(1);
/// // In real use these are a codestream's or JP2 file's bytes.
/// assert!(read_header_with(b"not a codestream", &options).is_err());
/// ```
pub fn read_header_with(bytes: &[u8], options: &DecodeOptions) -> Result<Header> {
    check_options(options)?;
    let reduction = options.resolution_reduction;
    if !bytes.starts_with(&[0xFF, 0x4F]) && jp2::looks_like_jp2(bytes) {
        let prefix = read_jp2_prefix(bytes, options)?;
        let siz = &prefix.parsed.header.siz;
        let (width, height) = siz.image_extent_at(reduction);
        let mut components = prefix.plans.outputs(siz, reduction)?;
        if let Some(srgb) = &prefix.srgb {
            srgb.describe_outputs(&mut components);
        }
        return Ok(Header {
            width,
            height,
            components,
            jp2: Some(prefix.metadata),
        });
    }
    let parsed = codestream::parse_header(bytes)?;
    let siz = &parsed.header.siz;
    check_raw_colour(options, siz.components.len())?;
    check_reduction(&parsed, reduction)?;
    let (width, height) = siz.image_extent_at(reduction);
    Ok(Header {
        width,
        height,
        components: ComponentInfo::of_siz(siz, reduction),
        jp2: None,
    })
}

/// What colour space a JP2 file's colour channels come out in: the
/// [`DecodeOptions::colour`] option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColourOutput {
    /// The samples as the codestream coded them, after the palette and the
    /// channel definitions. [`Jp2Metadata::colour`] says how to read them;
    /// an ICC profile is passed through, not applied.
    #[default]
    AsCoded,
    /// sRGB. sYCC is converted to RGB, with sub-sampled chroma (2x1 or 2x2)
    /// replicated to full resolution first, and a restricted ICC profile (a
    /// monochrome or a three-component matrix/TRC input profile) is applied
    /// at its rendering intent, as OpenJPEG applies it with LittleCMS: the
    /// samples match `opj_decompress -force-rgb`. The reference is OpenJPEG
    /// 2.5.0 linked against LittleCMS 2.14 on x86-64 Linux (glibc); the
    /// output depends on the C library's `powf`, so other LittleCMS versions
    /// or platforms can differ in the last bit. sRGB and greyscale pass
    /// through. A monochrome profile gives one grey channel. A monochrome
    /// profile whose output is not neutral (absolute colorimetric with a
    /// tinted white point) gives three sRGB channels in its place, as
    /// OpenJPEG does, so the channel count can't be predicted from the
    /// header. No conversion changes a channel's bit depth. Non-colour
    /// channels (opacity, say) are untouched. When the
    /// samples are converted, [`Jp2Metadata::colour`] says what they now
    /// are: [`ColourSpace::Srgb`], or [`ColourSpace::Greyscale`] for one
    /// grey channel, with no ICC profile. [`read_header`] and the default
    /// [`ColourOutput::AsCoded`] report the colour space the file declares.
    /// sYCC or an RGB profile on one or two colour channels passes through,
    /// as OpenJPEG leaves it, and so do one or two channels with no colour
    /// space to convert from (a raw codestream, a file with no `colr`, or
    /// another enumerated colour space), read as grey, or grey and opacity;
    /// their metadata stays as declared.
    ///
    /// Anything else is [`Error::Unsupported`]: three or more channels in
    /// another colour space or with no colour specification (a raw
    /// codestream included), another kind
    /// of ICC profile, an RGB profile on samples that are not 8 or 16-bit
    /// unsigned (8-bit for a monochrome profile), signed sYCC, and colour
    /// channels that differ in depth, sign or sub-sampling other than sYCC's
    /// chroma layouts. An ICC profile too short for its header or with a tag
    /// past its end is [`Error::FileFormat`].
    Srgb,
}

/// Options for [`decode_with`]. The default decodes at full resolution —
/// [`decode`] is exactly that.
///
/// Start from [`DecodeOptions::default`] and set what differs, with a
/// `with_*` method where one exists or the field directly. The struct is `#[non_exhaustive]`,
/// so a new option is not a breaking change.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// How many of the finest resolution levels to discard. Each level halves
    /// the output in both axes (rounding up), so `1` decodes a half-size image,
    /// `2` a quarter-size, and so on — the wavelet pyramid's own lower
    /// resolutions, not a resample. Every component must keep at least one
    /// resolution: a reduction that consumes some component's whole pyramid is
    /// rejected as [`Error::InvalidOptions`].
    pub resolution_reduction: u8,
    /// Return a JP2 file's codestream components as coded, without applying
    /// the file's palette (`pclr` + `cmap`) or channel definitions (`cdef`):
    /// the file decodes exactly as its codestream does on its own, in SIZ
    /// order, and [`Jp2Metadata::palette_applied`] is `false`. It means no
    /// colour conversion either, so it cannot be combined with
    /// [`ColourOutput::Srgb`] ([`Error::InvalidOptions`]). Each index
    /// plane is reported on [`Jp2Metadata::channels`] as
    /// [`ChannelType::Unspecified`] with [`Association::None`], since it is
    /// not a colour until it goes through the palette; every other channel
    /// `i` is reported as colour `i + 1`, whatever the `cdef` box says. The
    /// boxes' layout is still checked, so a malformed `pclr`, `cmap` or
    /// `cdef` is still [`Error::FileFormat`]; the palette mapping and column
    /// depths, and the channel definitions' entries, are not.
    ///
    /// The metadata carries what the decode left out, so a caller can rebuild
    /// the default decode, in order, from the raw one:
    ///
    /// 1. Expand the index planes. For each entry of
    ///    [`Jp2Metadata::channel_mapping`], in order, take the raw channel
    ///    `component`; a [`MappingKind::Direct`] entry is used as it is, and a
    ///    [`MappingKind::Palette`] entry looks each sample up as an index into
    ///    that column of [`Jp2Metadata::palette`] (an index past the table
    ///    reads its last entry, a negative one its first). Without a palette
    ///    the raw channels are used as they are.
    /// 2. Put the channels in colour order with
    ///    [`Jp2Metadata::channel_definitions`], by the rule documented there
    ///    (each swap looks up where the entry's channel has moved to, not its
    ///    original position). Without a `cdef` the order from step 1 stands.
    ///
    /// The result is the channels of the default decode. No effect on a raw
    /// codestream. Default `false`.
    pub raw_channels: bool,
    /// What colour space the colour channels come out in. Default
    /// [`ColourOutput::AsCoded`].
    pub colour: ColourOutput,
}

impl DecodeOptions {
    /// These options with
    /// [`resolution_reduction`](Self::resolution_reduction) set to `levels`.
    ///
    /// ```
    /// use rust_j2k::DecodeOptions;
    ///
    /// let options = DecodeOptions::default().with_resolution_reduction(1);
    /// assert_eq!(options.resolution_reduction, 1);
    /// ```
    #[must_use]
    pub fn with_resolution_reduction(mut self, levels: u8) -> Self {
        self.resolution_reduction = levels;
        self
    }

    /// These options with [`raw_channels`](Self::raw_channels) set to `raw`.
    ///
    /// ```
    /// use rust_j2k::DecodeOptions;
    ///
    /// let options = DecodeOptions::default().with_raw_channels(true);
    /// assert!(options.raw_channels);
    /// ```
    #[must_use]
    pub fn with_raw_channels(mut self, raw: bool) -> Self {
        self.raw_channels = raw;
        self
    }

    /// These options with [`colour`](Self::colour) set to `colour`.
    ///
    /// ```
    /// use rust_j2k::{ColourOutput, DecodeOptions};
    ///
    /// let options = DecodeOptions::default().with_colour(ColourOutput::Srgb);
    /// assert_eq!(options.colour, ColourOutput::Srgb);
    /// ```
    #[must_use]
    pub fn with_colour(mut self, colour: ColourOutput) -> Self {
        self.colour = colour;
        self
    }
}

/// Decode a JPEG 2000 codestream or JP2 file like [`decode`], governed by
/// `options`.
pub fn decode_with(bytes: &[u8], options: &DecodeOptions) -> Result<Image> {
    check_options(options)?;
    // A codestream opens with SOC; anything else that carries the signature
    // box's type is read as a JP2 file, so a damaged signature box is reported
    // as a box fault. The rest falls to the codestream parser, which reports
    // the missing SOC.
    if !bytes.starts_with(&[0xFF, 0x4F]) && jp2::looks_like_jp2(bytes) {
        return decode_jp2(bytes, options);
    }
    // The header checks come before the tile walk, as `read_header_with` has it.
    let parsed = codestream::parse_header(bytes)?;
    check_raw_colour(options, parsed.header.siz.components.len())?;
    check_reduction(&parsed, options.resolution_reduction)?;
    let cs = codestream::parse_tiles(bytes, parsed)?;
    decode_codestream(cs, options)
}

/// Options that contradict each other, whatever the input.
fn check_options(options: &DecodeOptions) -> Result<()> {
    if options.colour == ColourOutput::Srgb && options.raw_channels {
        return Err(Error::InvalidOptions(
            "raw channels means no colour conversion, so it cannot be combined with sRGB \
             output"
                .into(),
        ));
    }
    Ok(())
}

/// A reduction must leave every component a resolution to decode. This is the
/// check against the main header's coding parameters (COD and COC), which
/// `decode_with` and `read_header_with` both make before any tile is read; a
/// tile-part COD or COC with fewer levels is only found while decoding, where
/// the same test runs per tile.
fn check_reduction(parsed: &codestream::ParsedHeader, reduction: u8) -> Result<()> {
    for (component, params) in parsed.header.components.iter().enumerate() {
        let resolutions = u32::from(params.coding.decomposition_levels) + 1;
        if u32::from(reduction) >= resolutions {
            return Err(Error::InvalidOptions(format!(
                "resolution reduction {reduction} discards all {resolutions} resolutions \
                 of component {component}"
            )));
        }
    }
    Ok(())
}

/// A raw codestream declares no colour space. Like OpenJPEG, one or two
/// components are read as grey (and grey plus opacity) and pass through; three
/// or more have nothing to convert from, so sRGB output refuses them.
fn check_raw_colour(options: &DecodeOptions, components: usize) -> Result<()> {
    if options.colour == ColourOutput::Srgb && components > 2 {
        return Err(Error::Unsupported(format!(
            "sRGB output from a raw codestream of {components} components, which declares \
             no colour space"
        )));
    }
    Ok(())
}

/// A JP2 file read up to its tile data: the boxes, the codestream's main
/// header, the channel plans and the metadata. Everything here is checked
/// before any tile is read, so [`read_header`] and [`decode`] share it and
/// report a header fault the same way.
struct Jp2Prefix<'a> {
    codestream: &'a [u8],
    parsed: codestream::ParsedHeader,
    plans: jp2::ChannelPlans,
    metadata: Jp2Metadata,
    /// The sRGB step, when the caller asked for it.
    srgb: Option<jp2::SrgbPlan>,
}

/// Read a JP2 file up to its tile data, in OpenJPEG's order: the box walk,
/// the codestream's main header, `ihdr` against SIZ, then the channel plans.
/// The header boxes after the codestream, which OpenJPEG reads again once it
/// has decoded, are checked next, then the sRGB step is planned; all of it
/// before any tile is read.
fn read_jp2_prefix<'a>(bytes: &'a [u8], options: &DecodeOptions) -> Result<Jp2Prefix<'a>> {
    let file = jp2::parse(bytes)?;
    let parsed = codestream::parse_header(file.codestream)?;
    let siz = &parsed.header.siz;
    jp2::check_against_siz(&file.header, siz)?;
    let plans = jp2::ChannelPlans::plan(
        &file.header,
        siz,
        options.resolution_reduction,
        options.raw_channels,
    )?;
    jp2::check_after_codestream(&file, options.raw_channels)?;
    let mut metadata = jp2::metadata(&file, &plans, siz)?;
    let srgb = match options.colour {
        ColourOutput::AsCoded => None,
        ColourOutput::Srgb => {
            let plan = jp2::plan_srgb(&metadata, &plans, siz, options.resolution_reduction)?;
            // A converted image is sRGB (or grey) and carries no profile, as
            // OpenJPEG reports it.
            plan.describe(&mut metadata);
            Some(plan)
        }
    };
    check_reduction(&parsed, options.resolution_reduction)?;
    Ok(Jp2Prefix {
        codestream: file.codestream,
        parsed,
        plans,
        metadata,
        srgb,
    })
}

/// Decode a JP2 file: read it up to its tile data, decode the codestream,
/// and apply the channel plans, then any sRGB step, to the samples.
fn decode_jp2(bytes: &[u8], options: &DecodeOptions) -> Result<Image> {
    let prefix = read_jp2_prefix(bytes, options)?;
    let cs = codestream::parse_tiles(prefix.codestream, prefix.parsed)?;
    let image = decode_codestream(cs, options)?;
    let mut components = prefix.plans.apply(image.components)?;
    if let Some(srgb) = &prefix.srgb {
        components = srgb.apply(components)?;
    }
    Ok(Image {
        components,
        jp2: Some(prefix.metadata),
        ..image
    })
}

/// Decode a parsed codestream into its components.
fn decode_codestream(cs: codestream::Codestream<'_>, options: &DecodeOptions) -> Result<Image> {
    let reduction = options.resolution_reduction;

    // Each tile is a whole decode of its own: its own header, its own packets,
    // its own wavelet pyramid in its own coordinate frame. Nothing crosses a
    // tile boundary — that is what tiling is for — so the tiles meet only at the
    // end, when each writes its rectangle of the canvas.
    let mut canvas = image::Canvas::new(&cs.header, reduction)?;
    for (index, tile) in cs.tiles.iter().enumerate() {
        // The tile's own parameters: the main header with this tile's tile-part
        // header resolved over it. Held for one tile at a time — a header is
        // sized by the component count, and a codestream can declare tens of
        // thousands of tiles.
        let header = cs.tile_header(index)?;

        // A reduction must leave every component a resolution to decode, checked
        // against each component's own decomposition count: a COC can give one
        // component a shallower pyramid than the rest (p0_08 does exactly that),
        // and a tile COD can give one tile a shallower pyramid than the rest — so
        // this is per tile, not once for the image.
        for (component, params) in header.components.iter().enumerate() {
            let resolutions = u32::from(params.coding.decomposition_levels) + 1;
            if u32::from(reduction) >= resolutions {
                return Err(Error::InvalidOptions(format!(
                    "resolution reduction {reduction} discards all {resolutions} resolutions \
                     of component {component} in tile {index}"
                )));
            }
        }

        // Tier-2: parse packets into per-code-block coded segments, per
        // component. The walk covers every resolution whatever the reduction:
        // packet order is the codestream's framing, so the dropped levels'
        // packets must still be stepped over to reach the kept ones.
        let coded = tier2::decode_packets(&header, tile)?;
        // Tier-1: MQ + EBCOT bit-plane decode each code block into subband
        // coeffs. The dropped resolutions' code-blocks are skipped, never
        // decoded.
        let coeffs = tier1::decode_code_blocks(&header, &coded, reduction)?;

        // Dequantize, then invert the DWT per resolution level into samples.
        let mut samples = coeffs
            .into_iter()
            .enumerate()
            .map(|(component, coeffs)| {
                let dequant = quant::dequantize(&header, component, coeffs)?;
                dwt::inverse(&header, component, dequant)
            })
            .collect::<Result<Vec<_>>>()?;

        // Undo the inter-component decorrelation, if any, before the DC level
        // shift (ISO/IEC 15444-1 G.1). It is a per-tile transform: the flag
        // lives in COD, which a tile-part header can override. Components past
        // the third are untouched.
        if header.cod.multiple_component_transform {
            // The wavelet picks the transform (G.1): 5/3 reconstructs integers
            // and inverts with the reversible RCT, 9/7 reconstructs floats and
            // inverts with the irreversible ICT. All three colour components
            // share the wavelet (checked at parse), so component 0's decides.
            match header.components[0].coding.transform {
                codestream::markers::Transform::Reversible53 => mct::inverse_rct(&mut samples)?,
                codestream::markers::Transform::Irreversible97 => mct::inverse_ict(&mut samples)?,
            }
        }

        for (component, samples) in samples.into_iter().enumerate() {
            let rect = cs
                .header
                .siz
                .tile_component_rect(tile.index, component)
                .ok_or_else(|| {
                    Error::Inconsistent(format!(
                        "no tile {} or no component {component} in SIZ",
                        tile.index
                    ))
                })?;
            canvas.place(component, rect.reduced(reduction), samples)?;
        }
    }

    image::assemble(&cs.header, canvas.into_planes(), reduction)
}
