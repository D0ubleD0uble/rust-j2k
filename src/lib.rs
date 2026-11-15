//! A pure-Rust JPEG 2000 decoder. It has no C dependency, so cross-compiling it
//! needs no C toolchain.
//!
//! The decoder reads a **raw codestream** (ISO/IEC 15444-1 Annex A) or a
//! **JP2 file** (Annex I) into **integer** samples: one grid per component,
//! signed up to 32 bits, unsigned up to 31 (an `i32` container), each with its
//! own bit depth, sign, and sub-sampling. Both wavelet paths decode, the
//! reversible 5/3 bit-exactly and the irreversible 9/7 within the standard's
//! compliance bounds, as do tiling and tile-parts, precincts, quality layers,
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
//! The [feature scope](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/scope.md)
//! maps what is decoded and what is not.
//!
//! # Pipeline
//!
//! ```text
//! bytes ─▶ [jp2 boxes] ─▶ codestream ─▶ tier-2 ─▶ tier-1 ─▶ dequant
//!          (Annex I)      (markers)     (packets)  (MQ+EBCOT) (quant)
//!
//!       ─▶ inverse DWT ─▶ inverse MCT ─▶ [palette, cdef, sRGB] ─▶ Image
//!          (5/3 | 9/7)    (RCT | ICT)    (JP2 only)
//! ```
//!
//! [`decode`] runs these stages in order.
//!
//! # Example
//!
//! The core of the public surface is [`decode`]: codestream or JP2 bytes in,
//! an [`Image`] out, with one [`Component`] per output channel. Malformed
//! input never panics; it comes back as a typed [`Error`].
//!
//! ```
//! use rust_j2k::{decode, Error};
//!
//! // In real use these are the bytes of a `.j2k` codestream or `.jp2` file, or the GRIB2 §7
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
//! # Damaged input
//!
//! Decoding is strict by default. Truncated or inconsistent data is an
//! error, and no partial image comes back with it. Input cut before the
//! codestream's main header is whole, that is, before its first `SOT` marker,
//! is [`Error::Incomplete`], so a caller whose input streams in can tell it to
//! wait for more bytes. That covers every cut from the empty input on,
//! including a JP2 file's boxes before the codestream. A cut later fails as
//! [`Error::Codestream`], or as [`Error::FileFormat`] when it falls in a JP2
//! box after the codestream. A codestream whose last tile-part states its
//! length is refused without its closing `EOC` marker. A last tile-part of
//! length 0 (`Psot = 0`, "to the end") takes the data up to the final two
//! bytes without checking that they are `EOC`, as OpenJPEG does, so a
//! missing `EOC` there costs the tile its last two bytes and usually fails
//! the tile instead; in a JP2 file such a tile-part can also end where its
//! `jp2c` box ends. Input that is not JPEG 2000 at all, plain text included,
//! is `Codestream` too.
//!
//! Damage can show up as any variant, not only the ones named here. A
//! flipped bit in a header field can read as an absurd image size
//! ([`Error::Limit`]) or an unknown marker or a feature the decoder does not
//! read ([`Error::Unsupported`]). Do not retry a file with
//! [`WorkLimit::Unlimited`] on that evidence alone.
//!
//! JPEG 2000 carries no checksum, so damage inside the entropy-coded data is
//! not always detected: a flipped byte in a code-block can decode without an
//! error and give a wrong image. If that matters, check the file against a
//! hash you keep. [The correctness notes][strict] list what is refused and
//! where this decoder is stricter than OpenJPEG.
//!
//! ```
//! use rust_j2k::{Error, decode};
//!
//! let bytes = include_bytes!(concat!(
//!     env!("CARGO_MANIFEST_DIR"),
//!     "/tests/fixtures/jpeg2000_regular_latlon.j2k"
//! ));
//! assert!(decode(bytes).is_ok());
//!
//! // Cut short: an error, not a partial image.
//! let cut = &bytes[..bytes.len() / 2];
//! assert!(matches!(decode(cut), Err(Error::Codestream(_))));
//! // Without its closing `EOC` marker (FF D9): refused.
//! assert!(decode(&bytes[..bytes.len() - 2]).is_err());
//! // Not JPEG 2000 at all.
//! assert!(matches!(decode(b"hello"), Err(Error::Codestream(_))));
//! ```
//!
//! [strict]: https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/openjpeg-departures.md#stricter-than-the-oracle-on-non-conforming-streams
//!
//! With [`DecodeOptions::strict`] off, a codestream cut short, or one with
//! bytes left after a tile's last packet, decodes to the image the data
//! holds, and [`Image::report`] says what is missing (a [`DecodeReport`]).
//! This is what `opj_decompress -allow-partial` does. A codestream that
//! streams in can be decoded as it arrives: the report's
//! [`complete_reduction`](DecodeReport::complete_reduction) is the finest
//! level the data already gives in full. Until the main header has arrived,
//! the decode, like [`read_header`], is [`Error::Incomplete`]. Everything else
//! damaged is refused as when strict.
//!
//! # Limits
//!
//! A valid file can still be refused with [`Error::Limit`]. The decoder caps
//! what a file may declare and how much work it may ask for. Three limits
//! can be changed, each set with a [`WorkLimit`]:
//! [`DecodeOptions::sample_limit`] for the image's size,
//! [`DecodeOptions::packet_work_limit`] for reading the packet headers and
//! [`DecodeOptions::code_block_work_limit`] for decoding the code-blocks.
//! The other guards have no option; the list is on [`Error::Limit`]. The
//! message says which guard the file hit. For one of the three it also
//! names a [`WorkLimit::Raised`] factor (see [`WorkLimit`] for how far to
//! trust it).
//!
//! For a file from a source you trust, lift all three. [`WorkLimit`] has an
//! example that retries after an `Error::Limit`.
//!
//! # Conformance
//!
//! All 23 codestreams of the ISO/IEC 15444-4 conformance suite decode within
//! their class-1 error bounds, and its nine JP2 files within their compliance
//! class. That gate runs in `tests/` against committed oracle snapshots, with
//! no external tools. Correctness is defined by cross-checking OpenJPEG, not
//! by self-consistency. HTJ2K code-blocks decode with all three of their
//! coding passes. An input using a feature outside the decoded set (mixed-mode
//! code-blocks, region of interest on HT code-blocks, a Part 2 extension) is
//! rejected with [`Error::Unsupported`], never half-decoded. GRIB2 §5.40 (`grid_jpeg`)
//! payloads, the crate's original target, are raw codestreams and decode
//! directly. The feature map is in [`docs/scope.md`](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/scope.md).
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
pub(crate) mod limit_hint;
pub(crate) mod mct;
pub(crate) mod pipeline;
pub(crate) mod quant;
pub(crate) mod report;
pub(crate) mod samples;
pub(crate) mod tier1;
pub(crate) mod tier2;
pub(crate) mod window;

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
pub use report::{DecodeReport, ReportEntry};
pub use window::Window;

/// Decode a JPEG 2000 **codestream** (Annex A) or **JP2 file** (Annex I)
/// into an [`Image`]: the reference-grid image area plus one [`Component`] per
/// output channel, each on its own sample grid. For a raw codestream that is
/// one per SIZ component, in SIZ order. A JP2 file can change the channels:
/// its palette and channel definitions (below), and, through [`decode_with`],
/// the sRGB output option.
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
/// A JPH file (a JP2-family file holding an HTJ2K codestream) decodes as a
/// JP2 file does. A malformed box structure is [`Error::FileFormat`]. A file
/// whose File Type box has neither `jp2 ` nor `jph ` as its brand or in its
/// compatibility list is valid JPEG 2000 but is rejected with
/// [`Error::Unsupported`]. Anything else outside the decoded subset is
/// rejected the same way, never half-decoded.
///
/// Three limits bound a decode: the size of the image, and the work of
/// reading the packet headers and decoding the code-blocks for the file's
/// size. Past any of them the decode fails with [`Error::Limit`]. OpenJPEG has none of
/// them.
///
/// - The decoded image may hold at most 2^28 samples (1 GiB
///   as `i32`), summed over its output channels at the decode's
///   [`resolution_reduction`](DecodeOptions::resolution_reduction): every
///   component, a palette's expanded channels, and the channels sRGB output
///   adds. That admits a 10980 x 10980 Sentinel-2 band, or an RGB image of
///   up to about 89 MP. A larger image is refused from its header alone,
///   before any tile is read. Under a window or a tile, only the tiles read
///   count (see [`DecodeOptions::sample_limit`]). Peak memory is about 1.35
///   times the output for a square tile, an estimated 1.4 GiB at the limit,
///   and more for a tall, narrow tile or while a palette expands (see [the
///   sample limit][sample-limit]).
/// - Reading the packet headers of every tile may take
///   at most 2^24 steps, or 32 per byte of tile data when that is more. A
///   step is a packet read, a run of code-blocks a header skips without
///   reading a bit, or a position a progression order sweeps; [the tier-2
///   work bound][work-bound] gives the full count. A valid file of many
///   layers that each include a few scattered code-blocks can exceed it.
/// - Decoding the code-blocks may take at most
///   2^26 sample-passes (passes run times code-block samples), or 2^15 per
///   byte of tile data when that is more. A tile past it is refused before
///   any of its code-blocks is decoded. On the slowest data the bound comes
///   to about 0.5 s, or 0.25 ms a byte (see [the tier-1 work
///   bound][tier1-bound]).
///
/// [`decode_with`] can raise or lift each of the three, for a file from a
/// source you trust: see [`DecodeOptions::sample_limit`],
/// [`DecodeOptions::packet_work_limit`] and
/// [`DecodeOptions::code_block_work_limit`]. The message of an
/// [`Error::Limit`] from one of them names the option and a
/// [`WorkLimit::Raised`] factor; [`WorkLimit`] shows a retry and how far to
/// trust the factor. The guards with no option are listed on
/// [`Error::Limit`].
///
/// Decoding is strict by default: damaged or truncated data is an
/// [`Error`], not a partial image. With [`DecodeOptions::strict`] off,
/// missing data and leftover bytes give an image with an [`Image::report`]
/// of what is missing. A flipped byte in entropy-coded data can go unnoticed,
/// since the format has no checksum, and damage can show up as any variant
/// (see the crate docs).
///
/// [sample-limit]: https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md#the-sample-limit
/// [work-bound]: https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md#the-tier-2-work-bound
/// [tier1-bound]: https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md#the-tier-1-work-bound
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
/// A readable header does not mean [`decode`] will succeed. The tile data is
/// not read (except the packet headers of the last tile of a JP2 file whose
/// tile-part has `Psot = 0`, described on [`read_header_with`]), so it can
/// still be damaged, unsupported, or past a work bound ([`Error::Limit`]).
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
/// missing still has a readable header. Input that ends before the first
/// `SOT` marker is whole, from the empty input on and in a JP2 file's boxes
/// too, is [`Error::Incomplete`]: wait for more bytes and try again. Nothing
/// is allocated for the samples.
/// One case reads further: in a JP2 file whose last tile-part has `Psot = 0`
/// (no length) and whose `jp2c` box ends before the file does, the packet
/// headers of that tile-part's tile are walked to see whether the codestream
/// ends at the box's end. No samples are decoded, and every step of the walk
/// is bounded. A codestream of more than 4096 tiles or 1024 components, or of
/// fewer than 14 bytes per tile, is not walked, nor is a tile of more than
/// 16384 code-blocks or precincts. The walk stops after 2^22 steps, or 8 per
/// byte of the tile's data when that is more, counting the same steps as
/// the decode's bound (see [`decode`]);
/// [`packet_work_limit`](DecodeOptions::packet_work_limit) raises or lifts
/// this bound as it does the decode's. Past a bound the walk gives up, and the
/// codestream is taken to run to the end of the file, as for any `Psot = 0`
/// tile-part; [`decode_with`] does the same.
///
/// A fault in what it reads is the same [`Error`] that `decode_with(bytes,
/// options)` returns for that input, since `decode_with` runs the same checks
/// first. The options matter: a `cmap` that
/// [`raw_channels`](DecodeOptions::raw_channels) does not check, or an image
/// or palette that fits the sample limit only once
/// [`resolution_reduction`](DecodeOptions::resolution_reduction) shrinks the
/// image or [`sample_limit`](DecodeOptions::sample_limit) raises the limit,
/// is a fault under some options and not others. A fault in the tile
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
        let area = &prefix.selection.area;
        let (width, height) = area.image_extent_at(reduction);
        let mut components = if prefix.selection.subset {
            pick(
                ComponentInfo::of_siz(area, reduction),
                &prefix.selection.order,
            )
        } else {
            prefix.plans.outputs(area, reduction)?
        };
        if let Some(srgb) = &prefix.srgb {
            srgb.describe_outputs(&mut components);
        }
        return Ok(Header {
            width,
            height,
            components,
            max_reduction: max_reduction(&prefix.parsed.header),
            quality_layers: prefix.parsed.header.cod.layers,
            jp2: Some(prefix.metadata),
        });
    }
    let parsed = codestream::parse_header(bytes)?;
    let siz = &parsed.header.siz;
    // The selection's refusals come after the checks before it, as the decode
    // has them, but the sample limit counts the tiles it reads.
    let selection = window::select(siz, options);
    samples::check_read(
        siz,
        selection.as_ref().ok(),
        reduction,
        samples::cap(&options.sample_limit),
    )?;
    check_raw_color(options, siz.components.len())?;
    check_reduction(&parsed.header, reduction, None)?;
    let selection = selection?;
    let (width, height) = selection.area.image_extent_at(reduction);
    Ok(Header {
        width,
        height,
        components: pick(
            ComponentInfo::of_siz(&selection.area, reduction),
            &selection.order,
        ),
        max_reduction: max_reduction(&parsed.header),
        quality_layers: parsed.header.cod.layers,
        jp2: None,
    })
}

/// The entries of `all` that `order` names, in its order. Each index is in
/// range and named once ([`window::select`] checked).
fn pick<T: Clone>(all: Vec<T>, order: &[usize]) -> Vec<T> {
    if order.len() == all.len() && order.iter().enumerate().all(|(i, &c)| i == c) {
        return all;
    }
    order.iter().filter_map(|&c| all.get(c).cloned()).collect()
}

/// The largest reduction [`check_reduction`] lets the main header take: the
/// fewest decomposition levels of any component.
fn max_reduction(header: &codestream::MainHeader) -> u8 {
    header
        .components
        .iter()
        .map(|params| params.coding.decomposition_levels)
        .min()
        .unwrap_or(0)
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
    /// OpenJPEG applies it with LittleCMS. sYCC is converted to RGB, with
    /// sub-sampled chroma (2x1 or 2x2) replicated to full resolution first.
    /// The color channels are the ones the `cdef` box labels color 1, 2 and
    /// 3, wherever they sit. Other channels (opacity, say) are untouched. No
    /// conversion changes a channel's bit depth: a 12-bit file comes out as
    /// 12-bit sRGB, and a caller who wants 8-bit samples scales them (see the
    /// example below).
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
    /// When the samples are converted, [`Jp2Metadata::color`] says what they
    /// now are: [`ColorSpace::Srgb`], or [`ColorSpace::Grayscale`] for a
    /// monochrome profile, with no ICC profile. A monochrome profile whose
    /// output is not neutral (absolute colorimetric with a tinted white
    /// point) gives three sRGB channels in place of the gray one, as OpenJPEG
    /// does. [`read_header`] doesn't predict that; [`read_header_with`] with
    /// the same options does.
    ///
    /// An ICC conversion matches `opj_decompress -force-rgb` (OpenJPEG 2.5.0
    /// with LittleCMS 2.14 on x86-64 Linux, glibc 2.39) sample for sample.
    /// The output is the same on every platform whose `fma` is correctly
    /// rounded; CI checks this on x86-64 Linux, macOS on aarch64 and
    /// wasm32-wasip1. MinGW's `fma` (`*-pc-windows-gnu`) may not be correctly
    /// rounded (rust-lang/rust#140515), and i586 targets use x87 arithmetic,
    /// which is not exact IEEE, so the output there can differ. The sYCC
    /// conversion uses the IEC 61966-2-1 constants and rounds, and is within
    /// an MSE of 0.03 of the ISO references. OpenJPEG shortens the constants
    /// and truncates, so the two differ by an MSE of about 0.38 to 0.49 a
    /// channel.
    ///
    /// # Example: an 8-bit RGBA buffer
    ///
    /// Samples are `i32` at each channel's own depth and sign, so a caller
    /// that wants 8-bit RGBA does three things. It scales each sample to
    /// 8 bits, adding half the range first when the channel is signed. It
    /// finds the opacity channel by its [`ChannelType`] on
    /// [`Jp2Metadata::channels`], not by position. And it repeats a gray
    /// (one color channel) image's value for red, green and blue. This
    /// handles those three for a JP2 file whose channels share one grid; an
    /// image without opacity gets 255, and so does one whose opacity is
    /// [`ChannelType::PremultipliedOpacity`], which this recipe does not
    /// handle.
    ///
    /// ```
    /// use rust_j2k::{ChannelType, ColorOutput, DecodeOptions, Image, decode_with};
    ///
    /// /// One sample at its channel's depth and sign, as 8 bits.
    /// fn to_u8(sample: i32, bit_depth: u8, signed: bool) -> u8 {
    ///     let max = (1i64 << bit_depth) - 1;
    ///     let offset = if signed { 1i64 << (bit_depth - 1) } else { 0 };
    ///     let unsigned = (i64::from(sample) + offset).clamp(0, max);
    ///     ((unsigned * 255 + max / 2) / max) as u8
    /// }
    ///
    /// /// Interleaved RGBA, or `None` when the channels do not fit that shape
    /// /// (two colors, or channels on different grids).
    /// fn rgba8(image: &Image) -> Option<Vec<u8>> {
    ///     let first = image.components.first()?;
    ///     let same_grid = image
    ///         .components
    ///         .iter()
    ///         .all(|c| (c.width, c.height) == (first.width, first.height));
    ///     // A raw codestream has no labels: take every channel as a color.
    ///     let kinds: Vec<ChannelType> = match &image.jp2 {
    ///         Some(jp2) => jp2.channels.iter().map(|c| c.kind).collect(),
    ///         None => vec![ChannelType::Color; image.components.len()],
    ///     };
    ///     let colors: Vec<usize> = (0..kinds.len())
    ///         .filter(|&i| kinds[i] == ChannelType::Color)
    ///         .collect();
    ///     let alpha = kinds.iter().position(|&k| k == ChannelType::Opacity);
    ///     let (r, g, b) = match colors.as_slice() {
    ///         [gray] => (*gray, *gray, *gray),
    ///         [r, g, b, ..] => (*r, *g, *b),
    ///         _ => return None,
    ///     };
    ///     if !same_grid {
    ///         return None;
    ///     }
    ///     let mut out = Vec::with_capacity(first.samples.len() * 4);
    ///     for i in 0..first.samples.len() {
    ///         for channel in [Some(r), Some(g), Some(b), alpha] {
    ///             out.push(match channel {
    ///                 Some(c) => {
    ///                     let c = &image.components[c];
    ///                     to_u8(c.samples[i], c.bit_depth, c.signed)
    ///                 }
    ///                 None => 255,
    ///             });
    ///         }
    ///     }
    ///     Some(out)
    /// }
    ///
    /// let bytes = include_bytes!(concat!(
    ///     env!("CARGO_MANIFEST_DIR"),
    ///     "/tests/fixtures/jp2_cdef_rgba.jp2"
    /// ));
    /// let options = DecodeOptions::default().with_color(ColorOutput::Srgb);
    /// let image = decode_with(bytes, &options).expect("a 16x12 RGBA file");
    /// let rgba = rgba8(&image).expect("red, green, blue and opacity");
    /// assert_eq!(rgba.len(), 16 * 12 * 4);
    ///
    /// // A gray image repeats its value, and has no opacity channel.
    /// use rust_j2k::Component;
    /// let gray = Image::new(2, 1, vec![Component::new(2, 1, 12, false, 1, 1, vec![0, 4095])]);
    /// assert_eq!(rgba8(&gray), Some(vec![0, 0, 0, 255, 255, 255, 255, 255]));
    /// ```
    ///
    /// [outcomes]: https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/srgb-output.md#every-input-and-its-result
    Srgb,
}

/// Options for [`decode_with`]. [`decode`] is `decode_with` at
/// [`DecodeOptions::default`].
///
/// Start from [`DecodeOptions::default`] and set what differs with its
/// `with_*` method, or the field directly. The struct is `#[non_exhaustive]`,
/// so a new option is not a breaking change.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// How many of the finest resolution levels to discard. Each level about
    /// halves the output in both axes, so `1` decodes a half-size image, `2`
    /// a quarter-size, and so on: the wavelet pyramid's own lower
    /// resolutions, not a resample. The exact size depends on the image
    /// origin; [`Image`] and [`ComponentInfo::width`] give the formula. Every
    /// component must keep at least one resolution: a reduction that consumes
    /// some component's whole pyramid is rejected as
    /// [`Error::InvalidOptions`]. [`Header::max_reduction`] gives the largest
    /// the main header allows.
    pub resolution_reduction: u8,
    /// How many of the leading quality layers to decode: `Some(n)` decodes
    /// layers `0..n` of each tile, `None` (the default) every layer. A
    /// codestream builds its image up in layers, each adding coding passes
    /// to the code-blocks, so a cap gives a lower-fidelity image at full
    /// size from fewer passes, and the block decoder does less work. It is
    /// OpenJPEG's `opj_decompress -l n`.
    ///
    /// A cap at or above a tile's layer count decodes that tile whole, as
    /// OpenJPEG does. `Some(0)` is [`Error::InvalidOptions`]. It combines
    /// with [`resolution_reduction`](Self::resolution_reduction).
    ///
    /// The packets of the later layers are still read, since they frame the
    /// packets after them, so a fault in one is still an error, and they
    /// count against [`packet_work_limit`](Self::packet_work_limit) as
    /// without a cap. Only their coding passes are left out. With
    /// [`strict`](Self::strict) off, a packet of a layer past the cap that
    /// the data does not reach is not reported missing.
    ///
    /// [`Header::quality_layers`](crate::Header::quality_layers) is the layer
    /// count the main header declares.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, decode, decode_with};
    ///
    /// // 32x24, in three quality layers, the last one lossless.
    /// let bytes = include_bytes!(concat!(
    ///     env!("CARGO_MANIFEST_DIR"),
    ///     "/tests/fixtures/multilayer_lossless.j2k"
    /// ));
    /// let whole = decode(bytes).expect("decodes");
    /// let first = decode_with(bytes, &DecodeOptions::default().with_max_quality_layers(Some(1)))
    ///     .expect("decodes");
    /// // The same size, from fewer coding passes.
    /// assert_eq!((first.width, first.height), (whole.width, whole.height));
    /// assert_ne!(first.components[0].samples, whole.components[0].samples);
    /// // A cap at or past the layer count is the whole decode.
    /// let all = decode_with(bytes, &DecodeOptions::default().with_max_quality_layers(Some(3)))
    ///     .expect("decodes");
    /// assert_eq!(all.components[0].samples, whole.components[0].samples);
    /// ```
    #[doc(alias = "layers")]
    #[doc(alias = "cp_layer")]
    pub max_quality_layers: Option<u16>,
    /// Decode only this rectangle of the reference grid: `Some(window)`
    /// returns the part of the image inside it, `None` (the default) the
    /// whole image. It is OpenJPEG's `opj_set_decode_area`
    /// (`opj_decompress -d x0,y0,x1,y1`).
    ///
    /// The window is in reference-grid coordinates, as SIZ declares them,
    /// whatever the [`resolution_reduction`](Self::resolution_reduction).
    /// The part of it outside the image area is left out, as OpenJPEG clips
    /// it. The returned [`Image`] covers what is left, `x0..x1` by `y0..y1`:
    /// its [`width`](Image::width) is `x1 - x0` at full resolution and
    /// `ceil(x1 / 2^r) - ceil(x0 / 2^r)` at a reduction of `r`, and each
    /// component's is the same of `ceil(x0 / dx)` and `ceil(x1 / dx)` for its
    /// sub-sampling `dx` ([`ComponentInfo::width`] gives the formula for the
    /// whole image). The samples are those of a whole decode at the same
    /// position. [`ColorOutput::Srgb`] from sYCC with sub-sampled chroma is
    /// [`Error::Unsupported`] with a window, as the chroma replication
    /// depends on where the decoded area starts.
    ///
    /// Only the tiles the window reaches are decoded, so a small window of a
    /// large tiled image costs a few tiles. The packets of the other tiles
    /// are not read, so damage in them is not found, as in OpenJPEG. In the
    /// tiles it reaches, only the code-blocks whose wavelet support reaches
    /// the window are decoded, so damage in the others is not found either.
    /// Nor are the refusals of a code-block it skips: an HT code-block with
    /// refinement passes or a region of interest, and a code-block past the
    /// bit-plane limit. A window can therefore decode where the whole image
    /// is refused as [`Error::Unsupported`] or [`Error::Limit`].
    ///
    /// An empty window, one with no part inside the image area, and one that
    /// holds no sample of a returned component at the reduction are
    /// [`Error::InvalidOptions`]. It cannot be set with [`tile`](Self::tile).
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Window, decode, decode_with};
    ///
    /// // 32x24 at the grid's origin.
    /// let bytes = include_bytes!(concat!(
    ///     env!("CARGO_MANIFEST_DIR"),
    ///     "/tests/fixtures/multilayer_lossless.j2k"
    /// ));
    /// let whole = decode(bytes).expect("decodes");
    /// let options = DecodeOptions::default().with_window(Some(Window::new(8, 4, 20, 30)));
    /// let part = decode_with(bytes, &options).expect("decodes");
    /// // The window is cut to the image: rows 4 to 23.
    /// assert_eq!((part.width, part.height), (12, 20));
    /// assert_eq!(part.components[0].sample(0, 0), whole.components[0].sample(8, 4));
    /// ```
    #[doc(alias = "decode_area")]
    #[doc(alias = "region")]
    pub window: Option<Window>,
    /// Decode only this tile: `Some(index)` returns tile `index`'s area of
    /// the image, `None` (the default) the whole image. Tiles are numbered
    /// in raster order from 0, as the codestream's `Isot` numbers them. It is
    /// OpenJPEG's `opj_get_decoded_tile` (`opj_decompress -t index`).
    ///
    /// The result is the decode with [`window`](Self::window) set to the
    /// tile's rectangle on the reference grid: its size, its samples and its
    /// failures, the refusal of sRGB output from sub-sampled sYCC included.
    /// A tile index past the tile grid is [`Error::InvalidOptions`], and so
    /// is setting both this and [`window`](Self::window).
    pub tile: Option<u32>,
    /// Decode only these components, in this order: `Some(list)` returns one
    /// [`Component`] per entry of `list`, each the component of that SIZ
    /// index, and `None` (the default) every component. It is OpenJPEG's
    /// `opj_set_decoded_components` (`opj_decompress -c`), which takes them
    /// in the given order too.
    ///
    /// Each returned component has the samples of a whole decode. The other
    /// components' packets are still read, as they frame the packets after
    /// them, but their code-blocks are not decoded. Under the codestream's
    /// color transform (RCT or ICT), components 0, 1 and 2 are decoded
    /// together whenever one of them is asked for, since each of them needs
    /// all three. OpenJPEG skips the color transform instead when the
    /// components are chosen, so its `-c` returns other samples for those
    /// three.
    ///
    /// On a JP2 file the list names codestream components, which come out
    /// as coded: the palette and channel definitions are not applied, as
    /// with [`raw_channels`](Self::raw_channels), and as OpenJPEG has it.
    /// [`Jp2Metadata::channels`] describes the components returned.
    ///
    /// An empty list, an index named twice, an index past SIZ's component
    /// count, and a list with [`ColorOutput::Srgb`] are
    /// [`Error::InvalidOptions`]. The last holds even on a gray codestream,
    /// where sRGB output would pass the samples through, as it does for
    /// [`raw_channels`](Self::raw_channels).
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, decode, decode_with};
    ///
    /// // Three components under the irreversible color transform.
    /// let bytes = include_bytes!(concat!(
    ///     env!("CARGO_MANIFEST_DIR"),
    ///     "/tests/fixtures/layers/cprl_irreversible.j2k"
    /// ));
    /// let whole = decode(bytes).expect("decodes");
    /// let options = DecodeOptions::default().with_components(Some(vec![2, 0]));
    /// let some = decode_with(bytes, &options).expect("decodes");
    /// assert_eq!(some.components.len(), 2);
    /// assert_eq!(some.components[0], whole.components[2]);
    /// assert_eq!(some.components[1], whole.components[0]);
    /// ```
    #[doc(alias = "opj_set_decoded_components")]
    pub components: Option<Vec<u16>>,
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
    /// Under a [`window`](Self::window) or a [`tile`](Self::tile), the count
    /// is the samples of the tiles the decode reads, in every component, as
    /// each of those tiles is reconstructed whole; a palette's expanded
    /// channels and sRGB output count only the returned area. A small window
    /// of a large tiled image therefore fits the default, but a window of
    /// one large tile counts the whole tile. A
    /// [`components`](Self::components) subset does not lower the count, as
    /// the packets of every component of a tile read are still walked.
    ///
    /// `Raised(n)` allows `n` times 2^28 samples. `Unlimited` lifts the
    /// limit: the decode allocates what the header declares, 4 bytes per
    /// sample for the output, and about 1.35 to 3.2 times that in all at its
    /// peak (see [the sample limit](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md#the-sample-limit)), so a small hostile file can ask
    /// for many gigabytes. If
    /// the allocation fails the process aborts, as Rust's allocation
    /// failures do. Only an image too large for this target to
    /// address (`isize::MAX` bytes of samples) is still refused, with
    /// [`Error::Limit`]. Use it only for files from a source you trust.
    ///
    /// [`read_header_with`] checks the same limit, so it refuses what the
    /// decode would.
    pub sample_limit: WorkLimit,
    /// Refuse missing data with an [`Error`] (`true`, the default), or decode
    /// what is there and say what is missing on [`Image::report`] (`false`).
    /// It is the same switch as OpenJPEG's strict mode
    /// (`opj_decoder_set_strict_mode`, `opj_decompress -allow-partial`) and
    /// GDAL's `STRICT` option.
    ///
    /// `false` relaxes these two things and no others; any other relaxation
    /// is a separate option:
    ///
    /// - The input may end anywhere after the first `SOT`
    ///   marker, which ends the main header (input that ends before it is
    ///   [`Error::Incomplete`], as when strict): inside a tile-part, its header or
    ///   its packets, with or without the `EOC` marker. Tiles with no
    ///   tile-part are filled with 0, and packets that were never read (cut
    ///   off, or left out by the `POC` volumes) add nothing to their
    ///   code-blocks. A tile-part count (`TNsot`) that
    ///   disagrees with the tile-parts found is taken as advisory.
    /// - Bytes left after a tile's last packet are skipped
    ///   rather than refused, and the tile is reported as possibly wrong.
    ///
    /// Everything else is refused as a strict decode refuses it: a damaged
    /// packet header, coded data that fails to decode, a marker fault, tile-parts
    /// out of order or with a gap, and every [`Error::Limit`]. The work bounds
    /// and the sample limit hold as they do when strict, counted on the data
    /// that is there. A missing tile is not walked, but each of its
    /// tile-components is charged against the packet work bound, as are the
    /// packets after the end of the data, so a short input cannot ask for
    /// unbounded work.
    ///
    /// Strict or not, only the data a decode reads is checked. With a
    /// [`window`](Self::window) or [`tile`](Self::tile), the tiles outside it
    /// are not read, with a window the code-blocks of the tiles it reaches
    /// that the window does not need are not decoded, and with
    /// [`components`](Self::components) the code-blocks of the components
    /// left out are not decoded, so damage there goes unseen, as with
    /// `opj_decompress -t` and `-d`. So do the code-blocks a decode would
    /// refuse, such as HT code-blocks with refinement passes: a decode that
    /// skips them can succeed where the whole decode is refused.
    ///
    /// For a codestream as it streams in: a prefix that holds every packet of
    /// the resolutions a reduction keeps decodes at that reduction to the same
    /// samples as the whole codestream, and the report's
    /// [`complete_reduction`](DecodeReport::complete_reduction) says which
    /// reduction that is.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Error, decode_with, read_header};
    ///
    /// let bytes = include_bytes!(concat!(
    ///     env!("CARGO_MANIFEST_DIR"),
    ///     "/tests/fixtures/jpeg2000_regular_latlon.j2k"
    /// ));
    /// let prefix = &bytes[..bytes.len() - 20];
    /// // Strict, the default: a cut codestream is an error.
    /// assert!(matches!(
    ///     decode_with(prefix, &DecodeOptions::default()),
    ///     Err(Error::Codestream(_))
    /// ));
    /// // Not strict: the image, and what it lacks.
    /// let options = DecodeOptions::default().with_strict(false);
    /// let image = decode_with(prefix, &options).expect("decodes");
    /// assert!(!image.report.is_complete());
    /// // The coarsest level a decode may ask for, and the finest the data holds.
    /// let max = read_header(prefix).expect("the main header is there").max_reduction;
    /// if let Some(r) = image.report.complete_reduction.filter(|&r| r <= max) {
    ///     let coarse = decode_with(prefix, &options.with_resolution_reduction(r)).expect("decodes");
    ///     assert!(coarse.report.is_complete());
    /// }
    /// ```
    #[doc(alias = "allow_partial")]
    #[doc(alias = "allow-partial")]
    pub strict: bool,
}

impl Default for DecodeOptions {
    /// Full resolution, every quality layer, the whole image, components as
    /// the file's boxes shape them, color as coded, the default limits, and strict.
    fn default() -> Self {
        DecodeOptions {
            resolution_reduction: 0,
            max_quality_layers: None,
            window: None,
            tile: None,
            components: None,
            raw_channels: false,
            color: ColorOutput::default(),
            packet_work_limit: WorkLimit::default(),
            code_block_work_limit: WorkLimit::default(),
            sample_limit: WorkLimit::default(),
            strict: true,
        }
    }
}

/// How far a decode may go against one of its bounds: the
/// [`DecodeOptions::packet_work_limit`] and
/// [`DecodeOptions::code_block_work_limit`] work bounds, and the
/// [`DecodeOptions::sample_limit`] on the image's size.
///
/// Each work bound scales with the size of the file, so a small file cannot
/// ask for minutes of work, and the sample limit holds the memory a file may
/// ask for to an estimated 1.4 GiB for a square tile, and to a few GiB for a tall,
/// narrow one (see [the sample limit](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.4.0/docs/resource-bounds.md#the-sample-limit)). A valid file can still go past
/// a bound, when it codes many samples in very few bytes or is very
/// large. The decode then fails with [`Error::Limit`], and its message names
/// the option and a [`Raised`](WorkLimit::Raised) factor. Treat the factor as
/// a hint, not a measure of the file:
///
/// - For the sample limit it is exact for the check it names: the image's
///   samples are known from its header. Palette expansion or sRGB output is
///   checked later, against the same limit, and may need a higher factor.
/// - For the code-block bound it is what the tiles seen so far need, exact
///   for a file of one tile.
/// - For the packet bound it is the next factor up from the one that failed
///   (or, when the refusal comes from a POC sequence's reach or a
///   reduction's dropped code-blocks, the factor that check needs), and the
///   file may need more: the walk stops as soon as it runs
///   out, so it does not know the total. Double `n` on each retry rather than
///   stepping by one: each retry costs more, and a file can need a factor of
///   dozens.
///
/// Which value to pick:
///
/// - For a file from a source you trust, use [`Unlimited`](WorkLimit::Unlimited).
///   The decode then takes as long as the file asks.
/// - To keep a ceiling, use `Raised(n)` with an `n` of your own choosing,
///   the largest cost you will accept. Read the factor from the message to
///   size it: a file that needs `Raised(3)` is refused under `Raised(2)`. Do
///   not parse the message in code; it is a diagnostic, not a stable format.
///
/// Only these three limits can be changed. The other guards also end in
/// [`Error::Limit`], and no option here raises them (see [`Error::Limit`]).
/// A damaged file can hit a guard too, so do not lift a bound on that
/// evidence alone.
///
/// ```
/// use rust_j2k::{DecodeOptions, Error, WorkLimit, decode, decode_with};
///
/// // A valid 1 KB file that asks for far more work than its size allows.
/// let bytes = include_bytes!(concat!(
///     env!("CARGO_MANIFEST_DIR"),
///     "/tests/fixtures/tier1_work_bound/t1bomb_1024_64_88.j2k"
/// ));
/// let image = match decode(bytes) {
///     // The example fails if the bomb is ever admitted by default.
///     Ok(_) => panic!("expected the work bound to refuse this file"),
///     // Past a limit. The message says which, and the factor.
///     Err(Error::Limit(why)) => {
///         println!("refused: {why}");
///         // The file is trusted here, so lift all three limits. To keep a
///         // ceiling, use `WorkLimit::Raised(n)` for the one named instead.
///         let options = DecodeOptions::default()
///             .with_sample_limit(WorkLimit::Unlimited)
///             .with_code_block_work_limit(WorkLimit::Unlimited)
///             .with_packet_work_limit(WorkLimit::Unlimited);
///         decode_with(bytes, &options).expect("a valid file decodes once the limits are lifted")
///     }
///     Err(e) => panic!("not a work-bound failure: {e}"),
/// };
/// assert_eq!((image.width, image.height), (1024, 1024));
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
    /// The other two limits keep their own settings, and the guards with no
    /// option (listed on [`Error::Limit`]) stay in force.
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

    /// These options with
    /// [`max_quality_layers`](Self::max_quality_layers) set to `layers`.
    /// [`Header::quality_layers`](crate::Header::quality_layers) is how many
    /// layers the main header declares.
    ///
    /// ```
    /// use rust_j2k::DecodeOptions;
    ///
    /// let options = DecodeOptions::default().with_max_quality_layers(Some(2));
    /// assert_eq!(options.max_quality_layers, Some(2));
    /// ```
    #[must_use]
    pub fn with_max_quality_layers(mut self, layers: Option<u16>) -> Self {
        self.max_quality_layers = layers;
        self
    }

    /// These options with [`window`](Self::window) set to `window`.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Window};
    ///
    /// let options = DecodeOptions::default().with_window(Some(Window::new(0, 0, 64, 64)));
    /// assert_eq!(options.window, Some(Window::new(0, 0, 64, 64)));
    /// ```
    #[must_use]
    pub fn with_window(mut self, window: Option<Window>) -> Self {
        self.window = window;
        self
    }

    /// These options with [`tile`](Self::tile) set to `tile`.
    ///
    /// ```
    /// use rust_j2k::DecodeOptions;
    ///
    /// let options = DecodeOptions::default().with_tile(Some(3));
    /// assert_eq!(options.tile, Some(3));
    /// ```
    #[must_use]
    pub fn with_tile(mut self, tile: Option<u32>) -> Self {
        self.tile = tile;
        self
    }

    /// These options with [`components`](Self::components) set to
    /// `components`.
    ///
    /// ```
    /// use rust_j2k::DecodeOptions;
    ///
    /// let options = DecodeOptions::default().with_components(Some(vec![0]));
    /// assert_eq!(options.components, Some(vec![0]));
    /// ```
    #[must_use]
    pub fn with_components(mut self, components: Option<Vec<u16>>) -> Self {
        self.components = components;
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
    /// // Up to 3 x 2^28 samples, enough for a 16384 x 16384 RGB image.
    /// let options = DecodeOptions::default().with_sample_limit(WorkLimit::Raised(3));
    /// assert_eq!(options.sample_limit, WorkLimit::Raised(3));
    /// ```
    #[must_use]
    pub fn with_sample_limit(mut self, limit: WorkLimit) -> Self {
        self.sample_limit = limit;
        self
    }

    /// These options with [`strict`](Self::strict) set to `strict`.
    ///
    /// ```
    /// use rust_j2k::DecodeOptions;
    ///
    /// assert!(DecodeOptions::default().strict);
    /// let options = DecodeOptions::default().with_strict(false);
    /// assert!(!options.strict);
    /// ```
    #[must_use]
    #[doc(alias = "allow_partial")]
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }
}

/// Decode a JPEG 2000 codestream or JP2 file like [`decode`], governed by
/// `options`.
///
/// Every [`DecodeOptions`] field is set with a `with_*` method. This reads
/// a JP2 file at half size and converts it to sRGB:
///
/// ```
/// use rust_j2k::{ColorOutput, DecodeOptions, Error, decode_with};
///
/// let bytes = include_bytes!(concat!(
///     env!("CARGO_MANIFEST_DIR"),
///     "/tests/fixtures/jp2_cdef_rgba.jp2"
/// ));
/// let options = DecodeOptions::default()
///     .with_resolution_reduction(1)
///     .with_color(ColorOutput::Srgb);
/// match decode_with(bytes, &options) {
///     Ok(image) => {
///         // The file is 16x12; one level discarded halves each side.
///         assert_eq!((image.width, image.height), (8, 6));
///         // Red, green, blue and opacity.
///         assert_eq!(image.components.len(), 4);
///     }
///     // Options that contradict each other or do not fit the file (a
///     // reduction past its levels, `raw_channels` with sRGB output), or a
///     // file the conversion cannot handle (`Unsupported`), are handled
///     // here in real code. This file is fine, so they fail the example.
///     Err(Error::InvalidOptions(why) | Error::Unsupported(why)) => panic!("{why}"),
///     Err(e) => panic!("decode failed: {e}"),
/// }
/// ```
///
/// Failures are the same [`Error`] variants as [`decode`], plus
/// [`Error::InvalidOptions`] for options that are invalid on their own
/// ([`WorkLimit::Raised`]`(0)`, a [`max_quality_layers`](DecodeOptions::max_quality_layers)
/// of `Some(0)`), contradict each other, or do not fit the file.
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
    let selection = window::select(&parsed.header.siz, options);
    samples::check_read(
        &parsed.header.siz,
        selection.as_ref().ok(),
        options.resolution_reduction,
        samples::cap(&options.sample_limit),
    )?;
    check_raw_color(options, parsed.header.siz.components.len())?;
    check_reduction(&parsed.header, options.resolution_reduction, None)?;
    let selection = selection?;
    let cs = codestream::parse_tiles_with(bytes, parsed, options.strict)?;
    decode_codestream(cs, options, &selection)
}

/// Options that contradict each other, whatever the input.
fn check_options(options: &DecodeOptions) -> Result<()> {
    if options.color == ColorOutput::Srgb && options.raw_channels {
        return Err(Error::InvalidOptions(
            "`raw_channels` returns the channels as coded, so it cannot be combined with \
             `ColorOutput::Srgb`"
                .into(),
        ));
    }
    for (name, limit) in [
        ("packet_work_limit", &options.packet_work_limit),
        ("code_block_work_limit", &options.code_block_work_limit),
        ("sample_limit", &options.sample_limit),
    ] {
        if *limit == WorkLimit::Raised(0) {
            return Err(Error::InvalidOptions(format!(
                "`DecodeOptions::{name}` is `WorkLimit::Raised(0)`, but the factor must be \
                 at least 1"
            )));
        }
    }
    if options.max_quality_layers == Some(0) {
        return Err(Error::InvalidOptions(
            "`DecodeOptions::max_quality_layers` is `Some(0)`, but a decode keeps at least \
             one layer"
                .into(),
        ));
    }
    window::check_options(options)
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
    /// The area and components the decode returns.
    selection: window::Selection,
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
    let selection = window::select(siz, options);
    samples::check_read(
        siz,
        selection.as_ref().ok(),
        options.resolution_reduction,
        cap,
    )?;
    jp2::check_against_siz(&file.header, siz)?;
    // A component subset names codestream components, which come out as
    // coded: OpenJPEG bypasses the JP2 channel transforms under `-c`.
    let raw_channels = options.raw_channels || options.components.is_some();
    // A palette expands the area the decode returns.
    let plans = jp2::ChannelPlans::plan(
        &file.header,
        selection.as_ref().map_or(siz, |selection| &selection.area),
        options.resolution_reduction,
        cap,
        raw_channels,
    )?;
    jp2::check_after_codestream(&file, raw_channels)?;
    let mut metadata = jp2::metadata(&file, &plans, siz)?;
    let selection = selection?;
    if selection.subset {
        metadata.channels = pick(metadata.channels, &selection.order);
    }
    let srgb = match options.color {
        ColorOutput::AsCoded => None,
        ColorOutput::Srgb => {
            let plan = jp2::plan_srgb(
                &metadata,
                &plans,
                &selection.area,
                options.resolution_reduction,
                cap,
            )?;
            // Chroma replication keys on the parity of the area's origin, so
            // on part of the image it would not give the crop of the whole
            // image's conversion.
            if plan.replicates_chroma() && (options.window.is_some() || options.tile.is_some()) {
                return Err(Error::Unsupported(
                    "sRGB output from sub-sampled sYCC chroma under \
                     `DecodeOptions::window` or `DecodeOptions::tile`"
                        .into(),
                ));
            }
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
        selection,
    })
}

/// Decode a JP2 file: read it up to its tile data, decode the codestream,
/// and apply the channel plans, then any sRGB step, to the samples.
fn decode_jp2(bytes: &[u8], options: &DecodeOptions) -> Result<Image> {
    let prefix = read_jp2_prefix(bytes, options)?;
    let cs = codestream::parse_tiles_with(prefix.codestream, prefix.parsed, options.strict)?;
    let image = decode_codestream(cs, options, &prefix.selection)?;
    // A subset is planned raw, so the plans would pass it through, but they
    // are planned for every component.
    let mut components = if prefix.selection.subset {
        image.components
    } else {
        prefix.plans.apply(image.components)?
    };
    if let Some(srgb) = &prefix.srgb {
        components = srgb.apply(components)?;
    }
    Ok(Image {
        components,
        jp2: Some(prefix.metadata),
        ..image
    })
}

/// Decode a parsed codestream into the area and components `selection`
/// names.
fn decode_codestream(
    cs: codestream::Codestream<'_>,
    options: &DecodeOptions,
    selection: &window::Selection,
) -> Result<Image> {
    let reduction = options.resolution_reduction;
    let layers = options.max_quality_layers;

    // Each tile is a whole decode of its own: its own header, its own packets,
    // its own wavelet pyramid in its own coordinate frame. Nothing crosses a
    // tile boundary — that is what tiling is for — so the tiles meet only at the
    // end, when each writes its rectangle of the canvas. A window keeps the
    // part of each tile inside it, and the tiles with no part inside it are
    // not decoded.
    let mut canvas = image::Canvas::over(&cs.header, &selection.area, reduction, &selection.order)?;
    let reads = selection.tiles(&cs.header.siz);
    // Whether the decode returns less than the image, so a tile may need
    // only part of its samples.
    let windowed = selection.area.image_rect() != cs.header.siz.image_rect();
    // One work bound for every tile's packet walk, and one for every tile's
    // code-blocks, in proportion to the tiles' data, so that many small
    // tiles cannot each spend a floor's worth. A POC sequence's bitmap stays
    // within the default bound however far the caller raises it.
    let bytes = tile_bytes(&cs);
    let poc_bitmap = tier2::decode_visits(bytes);
    let meter = tier2::Meter::new(options.packet_work_limit.apply(poc_bitmap));
    let mut block_work = match &options.code_block_work_limit {
        WorkLimit::Unlimited => None,
        limit => Some(limit.apply(tier1::decode_work(bytes))),
    };
    let first_work = block_work.unwrap_or(0);
    // Not strict: what the codestream walk found missing, then each tile's
    // packets, and the smallest reduction every tile is complete at so far.
    let mut entries = window::keep_entries(cs.entries.clone(), &reads);
    let mut complete = Some(0u8);
    // A missing tile reads no packet, but setting its samples to 0 visits
    // each of its tile-components, and a header of a few kilobytes can
    // declare 65535 tiles of 16384 components with none of them present. So
    // each tile-component of a missing tile costs a visit, charged here,
    // before any tile is decoded.
    let missing: u64 = entries
        .iter()
        .filter_map(|entry| match entry {
            ReportEntry::MissingTiles { tiles } => Some(u64::from(tiles.end - tiles.start)),
            _ => None,
        })
        .sum();
    let cost = missing.saturating_mul(cs.header.siz.components.len() as u64);
    if !meter.try_spend(cost) {
        return Err(Error::Limit(limit_hint::missing_tiles(
            cost,
            meter.first(),
            poc_bitmap,
        )));
    }
    for (position, tile) in cs.tiles.iter().enumerate() {
        // `tiles` holds every tile when strict, so `index` is also its
        // position; not strict, a missing tile is left out.
        let index = tile.index as usize;
        if !reads.get(index).copied().unwrap_or(false) {
            continue;
        }
        // The tile's own parameters: the main header with this tile's tile-part
        // header resolved over it. A tile with no markers of its own borrows the
        // main header; any other is a copy, sized by the component count, held
        // for this tile alone, as a codestream can declare tens of thousands.
        // That copy is paid for by the geometry the packet walk then builds for
        // every component, whether or not the tile's data runs out; a tile
        // with no data at all is not resolved. So the copies stay within
        // `tiles × components`, which SIZ's rule that no tile-component is
        // empty holds to the image's samples (under a window or a tile, the
        // samples of the tiles read), with `strict` off as with it on.
        let header = cs.tile_header(position)?;

        // A tile COD can give one tile a shallower pyramid than the rest, so
        // the reduction is checked per tile, not once for the image.
        check_reduction(&header, reduction, Some(index))?;

        // Tier-2: parse packets into per-code-block coded segments, per
        // component. The walk covers every resolution and layer whatever the
        // reduction and layer cap: packet order is the codestream's framing,
        // so the dropped levels' and layers' packets must still be stepped
        // over to reach the kept ones.
        let walked = if options.strict {
            tier2::decode_packets_metered(&header, tile, &meter, poc_bitmap, reduction, layers)
                .map(|coded| (coded, None))
        } else {
            tier2::decode_packets_partial(&header, tile, &meter, poc_bitmap, reduction, layers)
                .map(|(coded, reach)| (coded, Some(reach)))
        };
        let mut coded = match walked {
            Ok((coded, reach)) => {
                if let Some(reach) = reach {
                    let tile_complete =
                        report_tile(&header, tile, &reach, reduction, layers, &mut entries);
                    complete = complete.zip(tile_complete).map(|(a, b)| a.max(b));
                }
                coded
            }
            Err(tier2::Refused::Work(work)) => {
                // The meter holds what is left after the refusal, so this
                // counts this tile's charges before it as well as the
                // earlier tiles'.
                return Err(Error::Limit(limit_hint::packet(
                    work,
                    index,
                    meter.spent(),
                    meter.first(),
                    poc_bitmap,
                )));
            }
            Err(tier2::Refused::Other(error)) => return Err(error),
        };
        // The components to reconstruct: those the decode returns, and with
        // them the rest of the color transform's three, which each need all
        // of them. It is a per-tile transform: the flag lives in COD, which a
        // tile-part header can override. On an image of fewer than three
        // components the flag is ignored, as OpenJPEG ignores it
        // (`mct::applies`).
        let mct = mct::applies(
            header.cod.multiple_component_transform,
            header.components.len(),
        );
        let mut wanted: Vec<bool> = (0..header.components.len())
            .map(|c| canvas.keeps(c))
            .collect();
        if mct && wanted[..3].contains(&true) {
            wanted[..3].fill(true);
        }

        // Under a window, each component reconstructed needs only the samples
        // whose wavelet support reaches the window: its footprint. `None`
        // where the window covers the whole tile-component.
        let footprints = coded
            .components
            .iter()
            .enumerate()
            .map(|(component, coded)| {
                if !windowed || !wanted[component] {
                    return Ok(None);
                }
                let rect = cs.header.siz.tile_component_rect(tile.index, component);
                let area = selection.area.component_rect(component);
                let transform = header.components.get(component).map(|c| c.coding.transform);
                let (Some(rect), Some(area), Some(transform)) = (rect, area, transform) else {
                    return Err(Error::Inconsistent(format!(
                        "no tile {} or no component {component} in `SIZ`",
                        tile.index
                    )));
                };
                dwt::Footprint::of(
                    coded,
                    reduction,
                    rect.reduced(reduction),
                    area.reduced(reduction),
                    transform,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        for (coded, footprint) in coded.components.iter_mut().zip(&footprints) {
            if let Some(footprint) = footprint {
                footprint.prune(coded);
            }
        }

        // Tier-1: the block coder (EBCOT or HT) decodes each code block into
        // subband coeffs. The dropped resolutions' code-blocks are skipped, never
        // decoded, and so are those of the components not reconstructed and
        // those outside a footprint. Their work is counted first, so a tile
        // past the bound is refused before any of it is done.
        if let Some(left) = &mut block_work {
            let work = tier1::work(&header, &coded, reduction, &wanted);
            #[cfg(test)]
            tests::BLOCK_WORK.with(|spent| spent.set(spent.get() + work));
            let Some(rest) = left.checked_sub(work) else {
                return Err(Error::Limit(limit_hint::code_blocks(
                    index,
                    work,
                    *left,
                    first_work.saturating_sub(*left),
                    tier1::decode_work(bytes),
                )));
            };
            *left = rest;
        }
        #[cfg(test)]
        tests::BLOCKS
            .with(|count| count.set(count.get() + tests::blocks(&coded, reduction, &wanted)));
        let coeffs = tier1::decode_code_blocks(&header, &coded, reduction, &wanted)?;

        // Invert the DWT per resolution level into samples, over the whole
        // tile-component or over its footprint. Tier-1 has already
        // dequantized the 9/7 coefficients as it placed each block.
        let mut samples = coeffs
            .into_iter()
            .zip(&footprints)
            .enumerate()
            .map(|(component, (coeffs, footprint))| {
                coeffs
                    .map(|coeffs| match footprint {
                        Some(footprint) => dwt::inverse_window(coeffs, footprint),
                        None => dwt::inverse(&header, component, coeffs),
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?;

        // Undo the inter-component decorrelation, if any, before the DC level
        // shift (ISO/IEC 15444-1 G.1). Components past the third are
        // untouched.
        if mct && wanted[0] {
            let mut triple = samples[..3]
                .iter_mut()
                .map(Option::take)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    Error::Inconsistent(
                        "the color transform's three components were not all reconstructed".into(),
                    )
                })?;
            // The wavelet picks the transform (G.1): 5/3 reconstructs integers
            // and inverts with the reversible RCT, 9/7 reconstructs floats and
            // inverts with the irreversible ICT. All three color components
            // share the wavelet (checked at parse), so component 0's decides.
            match header.components[0].coding.transform {
                codestream::markers::Transform::Reversible53 => mct::inverse_rct(&mut triple)?,
                codestream::markers::Transform::Irreversible97 => mct::inverse_ict(&mut triple)?,
            }
            for (slot, plane) in samples.iter_mut().zip(triple) {
                *slot = Some(plane);
            }
        }

        for (component, samples) in samples.into_iter().enumerate() {
            let Some(samples) = samples else {
                continue;
            };
            let rect = cs
                .header
                .siz
                .tile_component_rect(tile.index, component)
                .ok_or_else(|| {
                    Error::Inconsistent(format!(
                        "no tile {} or no component {component} in `SIZ`",
                        tile.index
                    ))
                })?;
            let rect = match footprints.get(component) {
                Some(Some(footprint)) => footprint.rect(),
                _ => rect.reduced(reduction),
            };
            canvas.place(component, rect, samples)?;
        }
    }

    let mut image = image::assemble_over(
        &selection.area,
        canvas.into_planes(),
        &selection.order,
        reduction,
    )?;
    if !options.strict {
        for entry in &entries {
            if let ReportEntry::MissingTiles { tiles } = entry {
                complete = None;
                for tile in tiles.clone() {
                    image::zero_tile(
                        &mut image,
                        &cs.header.siz,
                        &selection.area,
                        &selection.order,
                        tile,
                        reduction,
                    );
                }
            }
        }
        image.report = DecodeReport::new(entries, complete, reduction);
    }
    Ok(image)
}

/// Add what a non-strict walk of `tile` found to `entries`, and give the
/// smallest reduction at which the tile has every packet. Under a layer cap
/// of `cap`, only the packets of the layers it keeps count: one past it that
/// was not read is not missing.
fn report_tile(
    header: &codestream::MainHeader,
    tile: &codestream::Tile<'_>,
    reach: &tier2::Reach,
    reduction: u8,
    cap: Option<u16>,
    entries: &mut Vec<ReportEntry>,
) -> Option<u8> {
    let layers = u32::from(cap.map_or(header.cod.layers, |cap| cap.min(header.cod.layers)));
    let missing = |comp: usize, res: usize| reach.missing_from[comp][res].filter(|&l| l < layers);
    let resolutions: Vec<usize> = reach.missing_from.iter().map(Vec::len).collect();
    // The smallest reduction at which every resolution kept is whole.
    let most = resolutions.iter().min().map_or(0, |&n| n.saturating_sub(1));
    let leftover = reach.leftover.is_some() || reach.leftover_packed.is_some();
    let tile_complete = (0..=most)
        .find(|&r| {
            resolutions
                .iter()
                .enumerate()
                .all(|(comp, &n)| (0..n.saturating_sub(r)).all(|res| missing(comp, res).is_none()))
        })
        .and_then(|r| u8::try_from(r).ok())
        .filter(|_| !leftover);
    // The leading layers whole at the resolutions this decode keeps.
    let mut whole = layers;
    for (comp, &n) in resolutions.iter().enumerate() {
        for res in 0..n.saturating_sub(usize::from(reduction)) {
            if let Some(from) = missing(comp, res) {
                whole = whole.min(from);
            }
        }
    }
    if whole < layers {
        entries.push(ReportEntry::MissingPackets {
            tile: tile.index,
            complete_layers: whole as u16,
            complete_reduction: tile_complete,
        });
    }
    if let Some((at, bytes)) = reach.leftover {
        entries.push(ReportEntry::LeftoverBytes {
            tile: tile.index,
            tile_part: tile.part_at(at),
            bytes: bytes as u64,
            in_packed_headers: false,
        });
    }
    if let Some(bytes) = reach.leftover_packed {
        entries.push(ReportEntry::LeftoverBytes {
            tile: tile.index,
            tile_part: tile.part_at(tile.data.len()),
            bytes: bytes as u64,
            in_packed_headers: true,
        });
    }
    tile_complete
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

    thread_local! {
        /// The sample-passes the decodes on this thread have counted against
        /// the code-block work bound.
        pub(crate) static BLOCK_WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        /// The code-blocks with coding passes the decodes on this thread have
        /// handed to the block decoder.
        pub(crate) static BLOCKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    /// The code-blocks with coding passes in the resolutions `reduction`
    /// keeps of the components `wanted` marks: those Tier-1 decodes.
    pub(crate) fn blocks(coded: &tier2::CodedData<'_>, reduction: u8, wanted: &[bool]) -> u64 {
        let mut count = 0;
        for (component, coded) in coded.components.iter().enumerate() {
            if !wanted[component] {
                continue;
            }
            let kept = coded
                .resolutions
                .len()
                .saturating_sub(usize::from(reduction));
            for resolution in coded.resolutions.iter().take(kept.max(1)) {
                for subband in &resolution.subbands {
                    count += subband.blocks.iter().filter(|b| b.num_passes > 0).count() as u64;
                }
            }
        }
        count
    }

    /// The sample-passes Tier-1 runs on `bytes` (one tile) under a layer
    /// cap of `layers`, and the packet visits the walk spends.
    fn capped_work(bytes: &[u8], layers: Option<u16>) -> (u64, u64) {
        let parsed = codestream::parse_header(bytes).unwrap();
        let cs = codestream::parse_tiles(bytes, parsed).unwrap();
        let header = cs.tile_header(0).unwrap();
        let meter = tier2::Meter::new(u64::MAX);
        let coded =
            tier2::decode_packets_metered(&header, &cs.tiles[0], &meter, u64::MAX, 0, layers)
                .unwrap();
        let wanted = vec![true; coded.components.len()];
        (tier1::work(&header, &coded, 0, &wanted), meter.spent())
    }

    /// Each layer the cap drops is Tier-1 work not done, and the packet walk
    /// spends the same visits whatever the cap, since it still reads every
    /// packet.
    #[test]
    fn a_layer_cap_cuts_block_work_and_not_packet_work() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layers/lrcp.j2k"
        ))
        .unwrap();
        let (full, visits) = capped_work(&bytes, None);
        let mut previous = 0;
        for n in 1..=4 {
            let (work, spent) = capped_work(&bytes, Some(n));
            assert!(work > previous, "-l {n}: {work} after {previous}");
            assert_eq!(spent, visits, "-l {n}");
            previous = work;
        }
        assert_eq!(previous, full);
        assert_eq!(capped_work(&bytes, Some(u16::MAX)), (full, visits));
    }

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

    /// The sample-passes the block decoder counts on a decode of `name` (in
    /// `tests/fixtures/window/`) under `options`.
    fn block_work(name: &str, options: &DecodeOptions) -> u64 {
        let bytes = std::fs::read(format!(
            "{}/tests/fixtures/window/{name}.j2k",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        work_of(&bytes, options)
    }

    /// The sample-passes the block decoder counts on a decode of `bytes`
    /// under `options`.
    fn work_of(bytes: &[u8], options: &DecodeOptions) -> u64 {
        BLOCK_WORK.with(|spent| spent.set(0));
        BLOCKS.with(|count| count.set(0));
        decode_with(bytes, options).unwrap();
        BLOCK_WORK.with(std::cell::Cell::get)
    }

    /// A window inside one tile decodes only the code-blocks whose wavelet
    /// support reaches it, not every code-block of the tile.
    #[test]
    fn a_window_inside_a_tile_decodes_fewer_code_blocks_than_the_tile() {
        // `tests/fixtures/bench/` is left out of the published package, so
        // the test is skipped where the directory is not there; a file
        // missing from a directory that is there fails it.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bench");
        if !dir.is_dir() {
            eprintln!(
                "{} is not present (packaged crate?); skipping",
                dir.display()
            );
            return;
        }
        let path = dir.join("tiled_2048_gray.j2k");
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let all = DecodeOptions::default();
        // Tile 27 is 768..1024 on both axes: 256 x 256 in 64 x 64 code-blocks,
        // over four decomposition levels.
        let tile = work_of(&bytes, &all.clone().with_tile(Some(27)));
        assert!(tile > 0);
        let blocks = || BLOCKS.with(std::cell::Cell::get);
        // The tile's code-blocks with coding passes: the LL band's, one in
        // each band of the three coarser levels, and six of the twelve of
        // the finest level's 128 x 128 bands.
        assert_eq!(blocks(), 16);
        // Windows inside one code-block of each finest band: at the tile's
        // top-left and bottom-right corners, and one sample in the
        // bottom-right one. Each needs that code-block of each finest band
        // and every coarser one; the bottom-right blocks of two finest bands
        // have no coding pass.
        for (window, count) in [
            (Window::new(768, 768, 800, 800), 13),
            (Window::new(992, 992, 1024, 1024), 11),
            (Window::new(900, 900, 901, 901), 11),
        ] {
            let part = work_of(&bytes, &all.clone().with_window(Some(window)));
            assert!(part < tile, "{window:?}: {part} of {tile}");
            assert_eq!(blocks(), count, "{window:?}");
        }
        // A window across the middle of the tile, whose wavelet support
        // reaches all four code-blocks of each finest band.
        let middle = Window::new(880, 880, 912, 912);
        assert_eq!(
            work_of(&bytes, &all.clone().with_window(Some(middle))),
            tile
        );
        assert_eq!(blocks(), 16);
        // The window that is the whole tile decodes all of it.
        let whole = Window::new(768, 768, 1024, 1024);
        assert_eq!(work_of(&bytes, &all.clone().with_window(Some(whole))), tile);
        assert_eq!(blocks(), 16);
    }

    /// A window decodes only the tiles it reaches, and a component subset
    /// only its components' code-blocks, but every one of the color
    /// transform's three when it asks for one of them.
    #[test]
    fn a_window_or_a_subset_decodes_fewer_code_blocks() {
        let all = DecodeOptions::default();
        for name in ["tiled_53", "tiled_53_nomct"] {
            let whole = block_work(name, &all);
            // Tile 4, the middle one, is a ninth of the image.
            let tile = block_work(name, &all.clone().with_tile(Some(4)));
            assert!(tile * 6 < whole, "{name}: {tile} of {whole}");
            assert_eq!(
                block_work(
                    name,
                    &all.clone().with_window(Some(Window::new(27, 22, 51, 42)))
                ),
                tile,
                "{name}"
            );
            // Tiles 0, 1, 3 and 4.
            let four = block_work(
                name,
                &all.clone().with_window(Some(Window::new(20, 10, 41, 31))),
            );
            assert!(four > tile && four * 2 < whole, "{name}: {four} of {whole}");
        }
        let one = all.clone().with_components(Some(vec![1]));
        let whole = block_work("tiled_53_nomct", &all);
        let single = block_work("tiled_53_nomct", &one);
        assert!(single * 2 < whole, "{single} of {whole}");
        assert_eq!(block_work("tiled_53", &one), block_work("tiled_53", &all));
    }
}
