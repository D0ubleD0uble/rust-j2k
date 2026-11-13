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
//! reversible (RCT) and irreversible (ICT) color transforms.
//!
//! A JP2 file's codestream decodes the same way, and the file's header comes
//! back on [`Image::jp2`]. A palettized JP2 file decodes to the palette's
//! channels, and a file with channel definitions puts its color channels in
//! color order. [`read_header`] and [`read_header_with`] read the same header
//! without decoding any tile data. Samples come out as coded by default;
//! [`ColorOutput::Srgb`] converts sYCC and restricted ICC profiles to sRGB.
//! HTJ2K and an encoder are later work, not permanent non-goals. See the
//! [roadmap](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/roadmap.md) and the [feature scope](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/scope.md).
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
//! palette channel for a palettized JP2 file. A sub-sampled component holds
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
//! A JP2 file's header says how to read its samples. To get sRGB instead of
//! the samples as coded, ask for it in the options:
//!
//! ```no_run
//! use rust_j2k::{ColorOutput, ColorSpace, DecodeOptions, decode_with};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("photo.jp2")?;
//! let options = DecodeOptions::default().with_color(ColorOutput::Srgb);
//! let image = decode_with(&bytes, &options)?;
//! if let Some(jp2) = &image.jp2 {
//!     // After a conversion the color space says sRGB, or grayscale for a
//!     // monochrome profile; a file left as coded keeps the space it
//!     // declares. `channels` labels each component (color 1, 2, 3, opacity).
//!     if jp2.color == ColorSpace::Srgb {
//!         println!("sRGB, channels {:?}", jp2.channels);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Status
//!
//! All 23 codestreams of the ISO/IEC 15444-4 conformance suite decode within
//! their class-1 error bounds, and its nine JP2 files within their compliance
//! class. That gate runs in `tests/` against committed oracle snapshots, with
//! no external tools. Correctness is defined by cross-checking a reference
//! decoder (OpenJPEG / eccodes), not by self-consistency. An input using a
//! feature outside the decoded set (HTJ2K, a Part 2 extension) is rejected
//! with [`Error::Unsupported`], never half-decoded. GRIB2 §5.40 (`grid_jpeg`)
//! payloads, the crate's original target, are raw codestreams and decode
//! directly. See each module's docs for the ISO §reference and what it owns;
//! the feature map is in [`docs/scope.md`](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/scope.md).
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
pub(crate) mod samples;
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
    ColorSpace, ColorSpecification, FileType, ImageHeader, IprBox, Jp2Metadata, MappingKind,
    Palette, PaletteColumn, Ratio, ReaderRequirements, Res, Resolution, StandardFeature, UuidBox,
    UuidInfo, VendorFeature, XmlBox,
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
/// definitions (`cdef`) moves each color channel to its color's position
/// (color 1 first) and labels every channel on [`Jp2Metadata::channels`].
/// [`DecodeOptions::raw_channels`] skips both and returns the codestream's
/// components as coded.
///
/// ```no_run
/// use rust_j2k::{decode, ColorSpace};
///
/// // A `.jp2` file decodes the same way as a raw codestream; the format is
/// // detected from the first bytes.
/// let bytes = std::fs::read("photo.jp2")?;
/// let image = decode(&bytes)?;
/// if let Some(jp2) = &image.jp2 {
///     if jp2.color == ColorSpace::Srgb {
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
///
/// The decoded image may hold at most 2^28 samples (1 GiB as `i32`), summed
/// over its output channels at the decode's
/// [`resolution_reduction`](DecodeOptions::resolution_reduction): every
/// component, a palette's expanded channels, and the channels sRGB output
/// adds. That admits a 10980 x 10980 Sentinel-2 band, or an RGB image of up
/// to about 89 MP. A larger image is refused with [`Error::Limit`] from its
/// header alone, before any tile is read, so a small file cannot make the
/// decode allocate gigabytes. Peak memory is about 1.35 times the output for
/// one tile, and about twice it while a palette expands (see [the sample
/// limit][sample-limit]). OpenJPEG has no such limit.
///
/// The work of reading the packet headers is bounded, as the code-block count
/// is. Walking every tile's packets may take at most
/// 2^24 steps, or 32 per byte of tile data when that is more. A step is a
/// packet read, a run of code-blocks a header skips without reading a bit,
/// or a position a positional progression order sweeps, once for each
/// (component, resolution) checked there; code-block entries that read a bit
/// are paid for by the header's bytes and are not counted. A codestream past
/// the bound is refused with [`Error::Limit`]. Encoder output seen spends
/// under 13% of the bound; a packet header of one byte can still skip each
/// row of code-blocks in its precinct, and a sweep can check millions of
/// positions that hold no packet, so a file of a few hundred bytes could
/// otherwise take minutes. OpenJPEG has no such bound. A valid file of many
/// layers that each include a few scattered code-blocks can exceed it (see
/// [the tier-2 work bound][work-bound]). A JP2 file whose last tile-part has
/// `Psot = 0` may first walk that tile as [`read_header_with`] does, under its
/// own smaller bound, so its decode can take about 1.25 times as many steps.
///
/// The work of decoding the code-blocks is bounded too. Each coding pass
/// scans its whole code-block, however few bytes it carries, so the work is
/// counted as the passes run times the code-block's samples, over every
/// code-block decoded. It may come to at most 2^26 sample-passes, or 2^15
/// per byte of tile data when that is more, for every tile together. On
/// the data slowest to decode that is about 0.5 s, or 0.25 ms a byte. A
/// tile past the bound is refused with [`Error::Limit`] before any of its
/// code-blocks is decoded. A packet header can ask for 88 passes over 4096
/// samples in about 4 bytes, so a 76 KB file could otherwise take 19 s.
/// Encoder output seen spends up to about 14000 per byte, 43% of the bound
/// (see [the tier-1 work bound][tier1-bound]). OpenJPEG has no such bound.
///
/// [`decode_with`] can raise or lift the sample limit and either work bound,
/// for a file from a source you trust: see [`DecodeOptions::sample_limit`],
/// [`DecodeOptions::packet_work_limit`] and
/// [`DecodeOptions::code_block_work_limit`].
///
/// [sample-limit]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/correctness.md#the-sample-limit
/// [work-bound]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/correctness.md#the-tier-2-work-bound
/// [tier1-bound]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/correctness.md#the-tier-1-work-bound
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
/// One case reads further: in a JP2 file whose last tile-part has `Psot = 0`
/// (no length) and whose `jp2c` box ends before the file does, the packet
/// headers of that tile-part's tile are walked to see whether the codestream
/// ends at the box's end. No samples are decoded, and every step of the walk
/// is bounded. A codestream of more than 4096 tiles or 1024 components, or of
/// fewer than 14 bytes per tile, is not walked, nor is a tile of more than
/// 16384 code-blocks or precincts. The walk stops after 2^22 visits, or 8 per
/// byte of the tile's data when that is more, counting the same steps as
/// the decode's bound (see [`decode`]);
/// [`packet_work_limit`](DecodeOptions::packet_work_limit) raises or lifts
/// this bound as it does the decode's. Past a bound the codestream is taken to run to
/// the end of the file, the rule for a `Psot = 0` tile-part the check does
/// not apply to, and [`decode_with`] takes it the same way.
///
/// A fault in what it reads is the same [`Error`] that `decode_with(bytes,
/// options)` returns for that input, since `decode_with` runs the same checks
/// first. The options matter: a `cmap` that
/// [`raw_channels`](DecodeOptions::raw_channels) does not check, or an image
/// or palette that fits the sample limit only once
/// [`resolution_reduction`](DecodeOptions::resolution_reduction) shrinks the
/// image or [`sample_limit`](DecodeOptions::sample_limit) raises it, is a fault under some options and not others. A fault in the tile
/// data (a broken tile-part, a tile header that changes a component's
/// wavelet, a reduction that a tile-part COD or COC with fewer levels cannot
/// take) is only found by decoding. So is a POC sequence that leaves out
/// packets of a tile, even one in the main header: whether it covers a tile
/// depends on that tile's geometry. A reduction that the main header's own
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
    samples::check_codestream(siz, reduction, samples::cap(&options.sample_limit))?;
    check_raw_color(options, siz.components.len())?;
    check_reduction(&parsed.header, reduction, None)?;
    let (width, height) = siz.image_extent_at(reduction);
    Ok(Header {
        width,
        height,
        components: ComponentInfo::of_siz(siz, reduction),
        jp2: None,
    })
}

/// What color space a JP2 file's color channels come out in: the
/// [`DecodeOptions::color`] option.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
#[doc(alias = "ColourOutput")]
pub enum ColorOutput {
    /// The samples as the codestream coded them, after the palette and the
    /// channel definitions. [`Jp2Metadata::color`] says how to read them;
    /// an ICC profile is passed through, not applied.
    #[default]
    AsCoded,
    /// sRGB. A restricted ICC profile (a monochrome or a three-component
    /// matrix/TRC input profile) is applied at its rendering intent, as
    /// OpenJPEG applies it with LittleCMS: the samples match
    /// `opj_decompress -force-rgb` sample for sample. sYCC is converted to
    /// RGB, with sub-sampled chroma (2x1 or 2x2) replicated to full
    /// resolution first, using the IEC 61966-2-1 constants and rounding;
    /// OpenJPEG shortens the constants and truncates, so the two differ by an
    /// MSE of about 0.38 to 0.49 a channel. The ISO references agree with it
    /// (MSE under 0.03). The color channels are the ones the `cdef` box
    /// labels color 1, 2 and 3, wherever they sit. No conversion changes a
    /// channel's bit depth, and other channels (opacity, say) are untouched.
    ///
    /// The ICC reference is OpenJPEG 2.5.0 with LittleCMS 2.14 on x86-64
    /// Linux (glibc 2.39). The output is the same on every platform whose
    /// `fma` is correctly rounded; CI checks this on x86-64 Linux, macOS on
    /// aarch64 and wasm32-wasip1. MinGW's `fma` (`*-pc-windows-gnu`) may not
    /// be correctly rounded (rust-lang/rust#140515), and i586 targets use x87
    /// arithmetic, which is not exact IEEE, so the output there can differ.
    ///
    /// When the samples are converted, [`Jp2Metadata::color`] says what they
    /// now are: [`ColorSpace::Srgb`], or [`ColorSpace::Grayscale`] for a
    /// monochrome profile, with no ICC profile. A monochrome profile whose
    /// output is not neutral (absolute colorimetric with a tinted white
    /// point) gives three sRGB channels in place of the gray one, as OpenJPEG
    /// does. [`read_header`] doesn't predict that; [`read_header_with`] with
    /// the same options does.
    ///
    /// These pass through as coded, with the metadata as declared, except a
    /// three-channel 4:2:0 or 4:2:2 image (below):
    ///
    /// - sRGB and grayscale;
    /// - one or two channels, whatever the color space, as OpenJPEG reads them
    ///   as gray, or gray and opacity; only a monochrome profile converts
    ///   them;
    /// - an ICC profile that LittleCMS refuses to open or to build a
    ///   transform from, or whose color space is not RGB, gray or YCbCr, as
    ///   OpenJPEG leaves it. [`Jp2Metadata::color`] stays
    ///   [`ColorSpace::RestrictedIcc`], so a caller can tell nothing was
    ///   converted;
    /// - an RGB profile when any of the first four channels differs from the
    ///   first in depth, sign or sub-sampling (OpenJPEG's "AFL test").
    ///
    /// Anything else that cannot be converted is [`Error::Unsupported`]:
    ///
    /// - three or more channels with no color space (a raw codestream
    ///   included) or another enumerated color space;
    /// - a three-channel 4:2:0 or 4:2:2 image (luma at 1x1, both chroma
    ///   channels at 2x2 or both at 2x1) that is not declared sYCC, which
    ///   OpenJPEG converts as sYCC whatever the file declares;
    /// - color channels that are missing, or that differ in depth, sign or
    ///   sub-sampling outside the cases above;
    /// - signed sYCC, sYCC in another layout, and samples a profile is not
    ///   applied at (8- or 16-bit unsigned for RGB, 8-bit unsigned for gray);
    /// - a YCbCr profile, a profile LittleCMS would apply through a lookup
    ///   table or the Lab connection space, and a monochrome profile on more
    ///   than two channels.
    ///
    /// Replicated chroma, or a tinted gray channel, past the sample limit is
    /// [`Error::Limit`]. These errors apply only when `Srgb` is asked for:
    /// [`ColorOutput::AsCoded`] decodes the same files. The [table of
    /// outcomes][outcomes] lists every case against what OpenJPEG does, and
    /// the profiles LittleCMS refuses.
    ///
    /// [outcomes]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/correctness.md#outcomes
    Srgb,
}

/// Options for [`decode_with`]. The default decodes at full resolution —
/// [`decode`] is exactly that.
///
/// Start from [`DecodeOptions::default`] and set what differs with its
/// `with_*` method, or the field directly. The struct is `#[non_exhaustive]`,
/// so a new option is not a breaking change.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// How many of the finest resolution levels to discard. Each level about
    /// halves the output in both axes, so `1` decodes a half-size image, `2`
    /// a quarter-size, and so on: the wavelet pyramid's own lower
    /// resolutions, not a resample. The exact size depends on the image
    /// origin; [`Image`] and [`ComponentInfo::width`] give the formula. Every
    /// component must keep at least one resolution: a reduction that consumes
    /// some component's whole pyramid is rejected as
    /// [`Error::InvalidOptions`].
    pub resolution_reduction: u8,
    /// Return a JP2 file's codestream components as coded, without applying
    /// the file's palette (`pclr` + `cmap`) or channel definitions (`cdef`):
    /// the file decodes exactly as its codestream does on its own, in SIZ
    /// order, and [`Jp2Metadata::palette_applied`] is `false`. It means no
    /// color conversion either, so it cannot be combined with
    /// [`ColorOutput::Srgb`] ([`Error::InvalidOptions`]). Each index
    /// plane is reported on [`Jp2Metadata::channels`] as
    /// [`ChannelType::Unspecified`] with [`Association::None`], since it is
    /// not a color until it goes through the palette; every other channel
    /// is labeled as a file without a `cdef` is (see
    /// [`Jp2Metadata::channels`]), whatever the `cdef` box says. The
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
    ///    reads its last entry, a negative one its first). Each channel
    ///    takes its bit depth and sign from its palette column: column
    ///    `column` for a palette entry, and column `i` for a direct entry at
    ///    position `i`, as OpenJPEG has it. Without a palette the raw
    ///    channels are used as they are.
    /// 2. Put the channels in color order with
    ///    [`Jp2Metadata::channel_definitions`], by the rule documented there
    ///    (each swap looks up where the entry's channel has moved to, not its
    ///    original position). Without a `cdef` the order from step 1 stands.
    ///
    /// The result is the channels of the default decode. No effect on a raw
    /// codestream. Default `false`.
    pub raw_channels: bool,
    /// What color space the color channels come out in. Default
    /// [`ColorOutput::AsCoded`].
    pub color: ColorOutput,
    /// How much work reading the packet headers may take, against the bound
    /// described on [`decode`]. Default [`WorkLimit::Default`].
    ///
    /// This also bounds the walk of a `Psot = 0` tile that [`read_header_with`]
    /// describes, so [`read_header_with`] takes the same option.
    pub packet_work_limit: WorkLimit,
    /// How much work decoding the code-blocks may take, against the bound
    /// described on [`decode`]. Default [`WorkLimit::Default`].
    pub code_block_work_limit: WorkLimit,
    /// How many samples the decoded image may hold, against the limit
    /// described on [`decode`]: 2^28 samples (1 GiB as `i32`) by default,
    /// summed over the output channels at
    /// [`resolution_reduction`](Self::resolution_reduction). Default
    /// [`WorkLimit::Default`].
    ///
    /// `Raised(n)` allows `n` times 2^28 samples. `Unlimited` lifts the
    /// limit: the decode allocates what the header declares, about 4 bytes
    /// per sample for the output and as much again while the image is
    /// decoded (see [`decode`]), so a small hostile file can ask for many
    /// gigabytes. If the allocation fails the process aborts, as Rust's
    /// allocation failures do. Only an image too large for this target to
    /// address (`isize::MAX` bytes of samples) is still refused, with
    /// [`Error::Limit`]. Use it only for files from a source you trust.
    ///
    /// [`read_header_with`] checks the same limit, so it refuses what the
    /// decode would.
    pub sample_limit: WorkLimit,
}

/// How far a decode may go against one of its bounds: the
/// [`DecodeOptions::packet_work_limit`] and
/// [`DecodeOptions::code_block_work_limit`] work bounds, and the
/// [`DecodeOptions::sample_limit`] on the image's size.
///
/// Each work bound scales with the size of the file, so a small file cannot
/// ask for minutes of work, and the sample limit keeps a small file from
/// asking for gigabytes of memory (see [`decode`]). A valid file can still go
/// past a bound, when it codes many samples in very few bytes or is simply
/// very large: raise the limit, or lift it, to decode such a file from a
/// source you trust.
///
/// ```
/// use rust_j2k::{DecodeOptions, WorkLimit};
///
/// // Ten times the default bound for the code-blocks, and no bound on the
/// // packet headers.
/// let options = DecodeOptions::default()
///     .with_code_block_work_limit(WorkLimit::Raised(10))
///     .with_packet_work_limit(WorkLimit::Unlimited);
/// assert_eq!(options.code_block_work_limit, WorkLimit::Raised(10));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkLimit {
    /// The bound as [`decode`] describes it.
    #[default]
    Default,
    /// The default bound times this factor: for a work bound, both its floor
    /// and the work it allows per byte. `Raised(1)` sets the same bound as
    /// `Default`, though the two do not compare equal. `Raised(0)` is
    /// [`Error::InvalidOptions`].
    Raised(u32),
    /// No bound. For a work bound, the decode does all the work the file
    /// asks for, however long that takes; for the sample limit, it allocates
    /// all the memory the file asks for (see [`DecodeOptions::sample_limit`]).
    /// The other limits stay in force: the other options, the
    /// code-block-count cap, and the cap on the memory that remembers which
    /// packets a POC sequence has read.
    Unlimited,
}

impl WorkLimit {
    /// The bound this limit sets, from the default bound `bound`.
    pub(crate) fn apply(&self, bound: u64) -> u64 {
        match self {
            WorkLimit::Default => bound,
            WorkLimit::Raised(factor) => bound.saturating_mul(u64::from(*factor)),
            WorkLimit::Unlimited => u64::MAX,
        }
    }
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

    /// These options with [`color`](Self::color) set to `color`.
    ///
    /// ```
    /// use rust_j2k::{ColorOutput, DecodeOptions};
    ///
    /// let options = DecodeOptions::default().with_color(ColorOutput::Srgb);
    /// assert_eq!(options.color, ColorOutput::Srgb);
    /// ```
    #[must_use]
    #[doc(alias = "with_colour")]
    pub fn with_color(mut self, color: ColorOutput) -> Self {
        self.color = color;
        self
    }

    /// These options with [`packet_work_limit`](Self::packet_work_limit)
    /// set to `limit`.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, WorkLimit};
    ///
    /// let options = DecodeOptions::default().with_packet_work_limit(WorkLimit::Raised(4));
    /// assert_eq!(options.packet_work_limit, WorkLimit::Raised(4));
    /// ```
    #[must_use]
    pub fn with_packet_work_limit(mut self, limit: WorkLimit) -> Self {
        self.packet_work_limit = limit;
        self
    }

    /// These options with
    /// [`code_block_work_limit`](Self::code_block_work_limit) set to `limit`.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, WorkLimit};
    ///
    /// let options = DecodeOptions::default().with_code_block_work_limit(WorkLimit::Unlimited);
    /// assert_eq!(options.code_block_work_limit, WorkLimit::Unlimited);
    /// ```
    #[must_use]
    pub fn with_code_block_work_limit(mut self, limit: WorkLimit) -> Self {
        self.code_block_work_limit = limit;
        self
    }

    /// These options with [`sample_limit`](Self::sample_limit) set to
    /// `limit`.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, WorkLimit};
    ///
    /// // Up to 2^29 samples, such as a 16384 x 16384 RGB image.
    /// let options = DecodeOptions::default().with_sample_limit(WorkLimit::Raised(2));
    /// assert_eq!(options.sample_limit, WorkLimit::Raised(2));
    /// ```
    #[must_use]
    pub fn with_sample_limit(mut self, limit: WorkLimit) -> Self {
        self.sample_limit = limit;
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
    samples::check_codestream(
        &parsed.header.siz,
        options.resolution_reduction,
        samples::cap(&options.sample_limit),
    )?;
    check_raw_color(options, parsed.header.siz.components.len())?;
    check_reduction(&parsed.header, options.resolution_reduction, None)?;
    let cs = codestream::parse_tiles(bytes, parsed)?;
    decode_codestream(cs, options)
}

/// Options that contradict each other, whatever the input.
fn check_options(options: &DecodeOptions) -> Result<()> {
    if options.color == ColorOutput::Srgb && options.raw_channels {
        return Err(Error::InvalidOptions(
            "raw channels means no color conversion, so it cannot be combined with sRGB \
             output"
                .into(),
        ));
    }
    for (name, limit) in [
        ("packet work", &options.packet_work_limit),
        ("code-block work", &options.code_block_work_limit),
        ("sample", &options.sample_limit),
    ] {
        if *limit == WorkLimit::Raised(0) {
            return Err(Error::InvalidOptions(format!(
                "a {name} limit raised by a factor of 0; the default is \
                 `WorkLimit::Default`"
            )));
        }
    }
    Ok(())
}

/// A reduction must leave every component a resolution to decode, checked
/// against each component's own decomposition count: a COC can give one
/// component a shallower pyramid than the rest (p0_08 does exactly that).
/// `decode_with` and `read_header_with` make this check against the main header
/// (COD and COC) before any tile is read, with `tile` of `None`. A tile-part COD
/// or COC with fewer levels is only found while decoding, where the same test
/// runs per tile with that tile's index.
fn check_reduction(
    header: &codestream::MainHeader,
    reduction: u8,
    tile: Option<usize>,
) -> Result<()> {
    for (component, params) in header.components.iter().enumerate() {
        let resolutions = u32::from(params.coding.decomposition_levels) + 1;
        if u32::from(reduction) >= resolutions {
            let place = tile.map_or_else(String::new, |index| format!(" in tile {index}"));
            return Err(Error::InvalidOptions(format!(
                "resolution reduction {reduction} discards all {resolutions} resolutions \
                 of component {component}{place}"
            )));
        }
    }
    Ok(())
}

/// A raw codestream declares no color space. Like OpenJPEG, one or two
/// components are read as gray (and gray plus opacity) and pass through; three
/// or more have nothing to convert from, so sRGB output refuses them.
fn check_raw_color(options: &DecodeOptions, components: usize) -> Result<()> {
    if options.color == ColorOutput::Srgb && components > 2 {
        return Err(Error::Unsupported(format!(
            "sRGB output from a raw codestream of {components} components, which declares \
             no color space"
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
    let file = jp2::parse(bytes, &options.packet_work_limit)?;
    let parsed = codestream::parse_header(file.codestream)?;
    let siz = &parsed.header.siz;
    let cap = samples::cap(&options.sample_limit);
    samples::check_codestream(siz, options.resolution_reduction, cap)?;
    jp2::check_against_siz(&file.header, siz)?;
    let plans = jp2::ChannelPlans::plan(
        &file.header,
        siz,
        options.resolution_reduction,
        cap,
        options.raw_channels,
    )?;
    jp2::check_after_codestream(&file, options.raw_channels)?;
    let mut metadata = jp2::metadata(&file, &plans, siz)?;
    let srgb = match options.color {
        ColorOutput::AsCoded => None,
        ColorOutput::Srgb => {
            let plan = jp2::plan_srgb(&metadata, &plans, siz, options.resolution_reduction, cap)?;
            // A converted image is sRGB (or gray) and carries no profile, as
            // OpenJPEG reports it.
            plan.describe(&mut metadata);
            Some(plan)
        }
    };
    check_reduction(&parsed.header, options.resolution_reduction, None)?;
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
    // One work bound for every tile's packet walk, and one for every tile's
    // code-blocks, in proportion to the tiles' data, so that many small
    // tiles cannot each spend a floor's worth. A POC sequence's bitmap stays
    // within the default bound however far the caller raises it.
    let bytes = tile_bytes(&cs);
    let poc_bitmap = tier2::decode_visits(bytes);
    let mut visits = options.packet_work_limit.apply(poc_bitmap);
    let mut block_work = match &options.code_block_work_limit {
        WorkLimit::Unlimited => None,
        limit => Some(limit.apply(tier1::decode_work(bytes))),
    };
    for (index, tile) in cs.tiles.iter().enumerate() {
        // The tile's own parameters: the main header with this tile's tile-part
        // header resolved over it. A tile with no markers of its own borrows the
        // main header; any other is a copy, sized by the component count, held
        // for this tile alone, as a codestream can declare tens of thousands.
        let header = cs.tile_header(index)?;

        // A tile COD can give one tile a shallower pyramid than the rest, so
        // the reduction is checked per tile, not once for the image.
        check_reduction(&header, reduction, Some(index))?;

        // Tier-2: parse packets into per-code-block coded segments, per
        // component. The walk covers every resolution whatever the reduction:
        // packet order is the codestream's framing, so the dropped levels'
        // packets must still be stepped over to reach the kept ones.
        let coded = tier2::decode_packets(&header, tile, &mut visits, poc_bitmap)?;
        // Tier-1: MQ + EBCOT bit-plane decode each code block into subband
        // coeffs. The dropped resolutions' code-blocks are skipped, never
        // decoded. Their work is counted first, so a tile past the bound is
        // refused before any of it is done.
        if let Some(left) = &mut block_work {
            let work = tier1::work(&header, &coded, reduction);
            *left = left.checked_sub(work).ok_or_else(|| {
                Error::Limit(format!(
                    "code-block decoding exceeds its work bound: tile {index} needs \
                     {work} sample-passes, {left} are left (see \
                     `DecodeOptions::code_block_work_limit`)"
                ))
            })?;
        }
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
            // inverts with the irreversible ICT. All three color components
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

/// The bytes of tile data the codestream's packets are read from: every
/// tile's packet data and packed packet headers.
pub(crate) fn tile_bytes(cs: &codestream::Codestream<'_>) -> u64 {
    cs.tiles
        .iter()
        .map(|tile| (tile.data.len() + tile.packed_headers.len()) as u64)
        .fold(0, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reduction check names the component and the resolutions it would
    /// discard, and the tile only when it runs per tile.
    #[test]
    fn check_reduction_message_names_the_tile_only_when_given() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/roi_maxshift_lossless.j2k"
        ))
        .unwrap();
        let mut header = codestream::parse_header(&bytes).unwrap().header;
        // One component is shallower than the rest, as a COC or tile COD makes it.
        let last = header.components.len() - 1;
        header.components[last].coding.decomposition_levels = 1;
        assert!(check_reduction(&header, 1, None).is_ok());
        let message = |tile| match check_reduction(&header, 2, tile) {
            Err(Error::InvalidOptions(m)) => m,
            other => panic!("{other:?}"),
        };
        let tail = format!("resolution reduction 2 discards all 2 resolutions of component {last}");
        assert_eq!(message(None), tail);
        assert_eq!(message(Some(7)), format!("{tail} in tile 7"));
    }
}
