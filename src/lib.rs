//! A pure-Rust JPEG 2000 decoder. It has no C dependency, so cross-compiling it
//! needs no C toolchain.
//!
//! The decoder reads a **raw codestream** (ISO/IEC 15444-1 Annex A) or a
//! **JP2 file** (Annex I) into **integer** samples, and HTJ2K codestreams and
//! JPH files (ISO/IEC 15444-15) the same way: one grid per component,
//! signed up to 32 bits, unsigned up to 31 (an `i32` container), each with its
//! own bit depth, sign, and sub-sampling. Both wavelet paths decode, the
//! reversible 5/3 bit-exactly and the irreversible 9/7 within the standard's
//! compliance bounds, as do tiling and tile-parts, precincts, quality layers,
//! all five progression orders and POC changes between them, packed packet
//! headers (PPM/PPT), every Part 1 code-block coding style, region of interest
//! (maxshift), non-zero canvas offsets, resolution-reduced decoding, and the
//! reversible (RCT) and irreversible (ICT) color transforms. HTJ2K code-blocks
//! decode with all three of their coding passes, and a tile-component may mix
//! HT and Part 1 code-blocks. An HTJ2K file takes the same call and options
//! as any other. A decode can stop after some quality layers, or
//! return a window, one tile or some of the components ([`DecodeOptions`]).
//!
//! A JP2 file's codestream decodes the same way, and the file's header comes
//! back on [`Image::jp2`]. A palettized JP2 file decodes to the palette's
//! channels, and a file with channel definitions puts its color channels in
//! color order. [`read_header`] and [`read_header_with`] read the same header
//! without decoding any tile data. Samples come out as coded by default;
//! [`ColorOutput::Srgb`] converts sYCC and restricted ICC profiles to sRGB.
//! The [feature scope](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/scope.md)
//! maps what is decoded and what is not.
//!
//! # Pipeline
//!
//! ```text
//! bytes ─▶ [jp2 boxes] ─▶ codestream ─▶ tier-2 ─▶ tier-1 ─▶ dequant
//!          (Annex I)      (markers)     (packets)  (EBCOT|HT) (quant)
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
//! This section and the API docs use the standard's names for the parts of
//! a codestream: markers such as `SIZ`, `COD`, `SOT` and `EOC`, tiles and
//! tile-parts, packets, code-blocks and the reference grid. [What a JPEG 2000
//! file holds][terms] explains them.
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
//! # let bytes = include_bytes!(concat!(
//! #     env!("CARGO_MANIFEST_DIR"),
//! #     "/tests/fixtures/jpeg2000_regular_latlon.j2k"
//! # )).to_vec();
//! # /*
//! let bytes = std::fs::read("image.j2k")?;
//! # */
//! assert!(decode(&bytes).is_ok());
//!
//! // Cut short: an error, not a partial image.
//! let cut = &bytes[..bytes.len() / 2];
//! assert!(matches!(decode(cut), Err(Error::Codestream(_))));
//! // Without its closing `EOC` marker (FF D9): refused.
//! assert!(decode(&bytes[..bytes.len() - 2]).is_err());
//! // Not JPEG 2000 at all.
//! assert!(matches!(decode(b"hello"), Err(Error::Codestream(_))));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [terms]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/README.md#what-a-jpeg-2000-file-holds
//! [strict]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/openjpeg-departures.md#stricter-than-the-oracle-on-non-conforming-streams
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
//! For a file that arrives in pieces, use [`IncrementalDecoder`] rather than
//! decoding each prefix again: [`feed`](IncrementalDecoder::feed) appends
//! bytes, [`refine`](IncrementalDecoder::refine) decodes them from time to
//! time and returns the image they hold, with its report, or why it waits
//! ([`Waiting`]), and
//! [`finish`](IncrementalDecoder::finish) decodes the whole input once it
//! ends. Each refine goes on from where the last one stopped reading
//! packets.
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
//! error's [`LimitDetail`] says which guard the file hit, as a
//! [`LimitKind`]. For one of the three it also gives the [`WorkLimit`] to
//! retry with (see [`WorkLimit`] for how far to trust it).
//!
//! For a file from a source you trust, lift all three. [`WorkLimit`] and
//! [`LimitDetail`] have examples that retry after an `Error::Limit`.
//!
//! # Conformance
//!
//! All 23 codestreams of the ISO/IEC 15444-4 conformance suite decode within
//! their class-1 error bounds, and its nine JP2 files within their compliance
//! class. So do its 41 HTJ2K codestreams and 20 JPH files. That gate runs in
//! `tests/` against committed oracle snapshots, with no external tools.
//! Correctness is defined by cross-checking OpenJPEG, not by self-consistency.
//! An input using a feature outside the decoded set (an HTJ2K code-block that
//! needs more than one HT coding pass set, a Part 2 extension) is rejected with [`Error::Unsupported`],
//! never half-decoded. GRIB2 §5.40 (`grid_jpeg`) payloads, the crate's original
//! target, are raw codestreams and decode directly. The feature map is in
//! [`docs/scope.md`](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/scope.md).
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
pub(crate) mod incremental;
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

pub use error::{Error, ErrorDetail, LimitDetail, LimitKind, RefusedAt, Result};
pub use header::{ComponentInfo, Header, TileGrid};
pub use image::{Component, Image};
pub use incremental::{IncrementalDecoder, Refined, Waiting};
pub use jp2::{
    Association, ChannelDefinition, ChannelDefinitionEntry, ChannelMapping, ChannelType,
    ColorSpace, ColorSpecification, FileType, ImageHeader, IprBox, Jp2Metadata, MappingKind,
    Palette, PaletteColumn, Ratio, ReaderRequirements, Res, Resolution, StandardFeature, UuidBox,
    UuidInfo, VendorFeature, XmlBox,
};
pub use report::{DecodeReport, ReportEntry};
pub use window::{Region, Window};

// Compile the README's Rust examples as doctests so they cannot drift from
// the API. The blocks that read a file are marked `no_run`.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme_examples {}

/// Decode a JPEG 2000 **codestream** (Annex A) or **JP2 file** (Annex I)
/// into an [`Image`]: the reference-grid image area plus one [`Component`] per
/// output channel, each on its own sample grid. For a raw codestream that is
/// one per component of the `SIZ` marker, in `SIZ` order. [What a JPEG 2000
/// file holds](https://github.com/D0ubleD0uble/rust-j2k/blob/main/README.md#what-a-jpeg-2000-file-holds)
/// explains the codestream's parts. A JP2 file can change the channels:
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
/// - The decoded image may hold at most 2^29 samples (2 GiB
///   as `i32`), summed over its output channels at the decode's
///   [`resolution_reduction`](DecodeOptions::resolution_reduction): every
///   component, a palette's expanded channels, and the channels sRGB output
///   adds. Up to 64 MiB of input that admits a 10980 x 10980 Sentinel-2
///   true-color image (three components), or an RGB image of up to about
///   179 MP; past 64 MiB the limit is 8 samples per byte instead. An image
///   over the limit is refused from its header alone, before any tile is
///   read. Under a window or a tile, only the tiles read
///   count (see [`DecodeOptions::sample_limit`]). Peak memory is about 1.35
///   times the output for a square tile, about 2.7 GiB at 2^29 samples,
///   and more for a tall, narrow tile or while a palette expands (see [the
///   sample limit][sample-limit]). A tile's code-block segment records can
///   add up to about 65 bytes per byte of input. The limit also sets the code-blocks and
///   precincts one tile may hold: one for each 512 and each 2048 samples it
///   allows, and at least 2^20 and 2^18. This is for the one-shot functions; an
///   [`IncrementalDecoder`] keeps per-tile state and can hold up to about
///   three times one decode's peak, plus the bytes fed and a table of 24 bytes
///   per tile index up to the highest seen (about 1.5 MiB for 65535 tiles).
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
/// [`DecodeOptions::code_block_work_limit`]. An [`Error::Limit`] from one
/// of them names the option, as [`LimitDetail::kind`], and the
/// [`WorkLimit`] to retry with, as [`LimitDetail::retry_with`]; its message
/// names both too. [`WorkLimit`] shows a retry and how far to trust the
/// factor. The guards with no option are listed on [`Error::Limit`].
///
/// Decoding is strict by default: damaged or truncated data is an
/// [`Error`], not a partial image. With [`DecodeOptions::strict`] off,
/// missing data and leftover bytes give an image with an [`Image::report`]
/// of what is missing. A flipped byte in entropy-coded data can go unnoticed,
/// since the format has no checksum, and damage can show up as any variant
/// (see the crate docs).
///
/// [sample-limit]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/resource-bounds.md#the-sample-limit
/// [work-bound]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/resource-bounds.md#the-tier-2-work-bound
/// [tier1-bound]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/resource-bounds.md#the-tier-1-work-bound
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
/// not read, so it can still be damaged, unsupported, or past a work bound
/// ([`Error::Limit`]). The tile-part headers are read only where a tile is
/// over a cap on its code-blocks or precincts under the main header. Where
/// that read refuses a tile, finds a tile with no tile-part before it, or
/// stops at its budget (which can happen with no tile over a cap), it walks
/// every tile-part header as the decode's parse does. That walk keeps no
/// tile record: up to 10 bytes of heap a tile, plus the coding of long first
/// tile-part headers that later tile-parts restate. The packet headers of the
/// last tile of a JP2 file whose tile-part has `Psot = 0` are read too. All
/// of it is described on [`read_header_with`].
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
/// (`SOT` marker), and stops there, unless a tile is over a cap on its
/// code-blocks or precincts under the main header (see below). So a file
/// whose tile data is cut short or missing still has a readable header.
/// Input that ends before the first `SOT` marker is whole, from the empty
/// input on and in a JP2 file's boxes too, is [`Error::Incomplete`]: wait
/// for more bytes and try again. Nothing is allocated for the samples or the
/// tile data.
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
/// The read also counts the code-blocks and precincts of each tile it would
/// read under the main header, and refuses a tile past the caps
/// [`sample_limit`](DecodeOptions::sample_limit) sets, with the
/// [`Error::Limit`] the decode gives, where the decode refuses it too. Only
/// for a tile over a cap there does it read further: it follows the
/// tile-parts by their `Psot`, reading their `SOT`s, once in all and no
/// further than the last tile it counts, and reads the first tile-part header
/// of each tile it counts within its `Psot`, no tile data, holding one tile's
/// coding parameters at a time (but see the refusal of a tile with no
/// tile-part below). A tile-part `COD` or `COC` that brings
/// the tile within the caps admits it, as does
/// [`strict`](DecodeOptions::strict) off when the tile has no tile-part,
/// since the decode then leaves it out. The decode parses every tile-part
/// before it counts any tile, whatever the selection, so where the read
/// refuses a tile, stops at its budget, or, with `strict` on, finds a tile
/// with no tile-part (it looks up the tiles it would read up to a tile over
/// a cap before it counts any lower tile), it walks the tile-parts as that
/// parse does, keeping no tile record, and gives the parse's error where it
/// finds one: the first fault in the tile-parts or, with `strict` on, the
/// missing tile. Before
/// such a tile, it counts each lower tile it would read that the tile's own
/// header could refuse, under that header, as the decode counts each tile
/// in index order: each whose first tile-part carries a `COD` or `COC` and
/// whose size does not rule out a tile-part header taking it over the caps,
/// and under a reduction each with a `COD` or `COC` whatever its size, as
/// one can give fewer levels than the reduction takes. So the tile it
/// refuses is the lowest the decode refuses. Where a tile-part header the
/// read counts a tile under is at fault, or the tile's precincts and
/// code-blocks under it are, the read gives the parse's error in the same
/// way, if the parse finds one. With `strict` off it does not for a fault
/// the input ends inside, which the decode reads as the input cut short.
/// Where the read refuses no tile, a fault the
/// decode finds in a tile-part header the read does not read is left to
/// the decode, as is a
/// tile-part `COD` that takes a tile over the caps with no tile over them
/// under the main header after it. The count has a budget
/// of 2^22 steps under the main header and as many under the tiles' own;
/// the tiles past it are left to the decode too. For all of those it can
/// return a header for a file the decode refuses. The time is one walk of
/// the tile-parts at most plus the budget: 45 ms in release for a window of
/// 1530 tiles over a cap of a 235 MB file of 65535 tiles in 16.7 million
/// tile-parts, under a millisecond with no tile over a cap, and 63 ms for
/// 1000 tiles of 4096 components, each with a tile-part `COD`. A refusal
/// or a stop at the budget costs that walk of the tile-parts on top, with
/// up to 18 bytes of heap a tile (30 on an input of 4 GiB or more), and,
/// for each tile whose later tile-parts restate the coding of its first
/// with headers that differ, that coding, up to about 14 times the bytes
/// of those tiles' tile-parts: 240 ms and 640 KiB in release on that
/// 235 MB file with the tile read left out, and 150 ms, about what the
/// decode takes to refuse it, on a 13 MB file of 2048 tiles whose first
/// tile-part headers of 143 `QCC`s each 254 later tile-parts restate.
/// The decode can also refuse the file before it
/// gets to that tile, for what only decoding finds, and then names another
/// tile, kind or factor, or another error: a fault or a work bound in the
/// packets or code-blocks of a lower tile (with `strict` on, data that ends
/// early or runs on past the last packet), or, with `strict` off, the
/// charge of a step for each
/// tile-component of the tiles read that have no tile-part, past the
/// [`packet_work_limit`](DecodeOptions::packet_work_limit).
///
/// A fault in what it reads is the same [`Error`] that `decode_with(bytes,
/// options)` returns for that input, since `decode_with` runs the same checks
/// first. The options matter: a `cmap` that
/// [`raw_channels`](DecodeOptions::raw_channels) does not check, or an image
/// or palette that fits the sample limit only once
/// [`resolution_reduction`](DecodeOptions::resolution_reduction) shrinks the
/// image or [`sample_limit`](DecodeOptions::sample_limit) raises the limit,
/// is a fault under some options and not others. The sample limit is that
/// of the bytes passed, so a prefix holding only the header is held to the
/// 2^29-sample floor: the header of a valid file over 64 MiB and over 2^29
/// samples, read from a prefix, gives [`Error::Limit`] even when the whole
/// file decodes at the defaults. The [`Raised`](WorkLimit::Raised) factor
/// that refusal names also admits the whole file, whose default is larger.
/// A fault in the tile
/// data (a broken tile-part, a tile header that changes a component's
/// wavelet, a reduction that a tile-part COD or COC with fewer levels cannot
/// take, unless in a tile counted as above) is only found by decoding. So is a POC sequence that leaves out
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
    pipeline::decode::read_header(bytes, options)
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
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/jp2_cdef_rgba.jp2"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.jp2")?;
    /// # */
    /// // For a 16x12 RGBA file.
    /// let options = DecodeOptions::default().with_color(ColorOutput::Srgb);
    /// let image = decode_with(&bytes, &options).expect("a 16x12 RGBA file");
    /// let rgba = rgba8(&image).expect("red, green, blue and opacity");
    /// assert_eq!(rgba.len(), 16 * 12 * 4);
    ///
    /// // A gray image repeats its value, and has no opacity channel.
    /// use rust_j2k::Component;
    /// let gray = Image::new(2, 1, vec![Component::new(2, 1, 12, false, 1, 1, vec![0, 4095])]);
    /// assert_eq!(rgba8(&gray), Some(vec![0, 0, 0, 255, 255, 255, 255, 255]));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// [outcomes]: https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/srgb-output.md#every-input-and-its-result
    Srgb,
}

/// Options for [`decode_with`]. [`decode`] is `decode_with` at
/// [`DecodeOptions::default`].
///
/// Start from [`DecodeOptions::default`] and set what differs with its
/// `with_*` method, or the field directly. The struct is `#[non_exhaustive]`,
/// so a new option is not a breaking change.
#[derive(Clone)]
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
    /// // For a 32x24 file in three quality layers, the last one lossless.
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/multilayer_lossless.j2k"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.j2k")?;
    /// # */
    /// let whole = decode(&bytes).expect("decodes");
    /// let first = decode_with(&bytes, &DecodeOptions::default().with_max_quality_layers(Some(1)))
    ///     .expect("decodes");
    /// // The same size, from fewer coding passes.
    /// assert_eq!((first.width, first.height), (whole.width, whole.height));
    /// assert_ne!(first.components[0].samples, whole.components[0].samples);
    /// // A cap at or past the layer count is the whole decode.
    /// let all = decode_with(&bytes, &DecodeOptions::default().with_max_quality_layers(Some(3)))
    ///     .expect("decodes");
    /// assert_eq!(all.components[0].samples, whole.components[0].samples);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[doc(alias = "layers")]
    #[doc(alias = "cp_layer")]
    pub max_quality_layers: Option<u16>,
    /// The area to decode: [`Region::Whole`] (the default) the whole image,
    /// [`Region::Window`] a rectangle of the reference grid, and
    /// [`Region::Tile`] one tile. One field holds all three, so a window and a
    /// tile cannot be asked for together. A window is OpenJPEG's
    /// `opj_set_decode_area` (`opj_decompress -d x0,y0,x1,y1`); a tile is its
    /// `opj_get_decoded_tile` (`opj_decompress -t index`).
    ///
    /// # Window
    ///
    /// A [`Region::Window`] returns the part of the image inside it. It is in
    /// reference-grid coordinates, as SIZ declares them, whatever the
    /// [`resolution_reduction`](Self::resolution_reduction).
    /// [`Header::image_area`] gives the image area in those coordinates,
    /// and [`TileGrid::tile`] each tile's rectangle.
    /// The part of it outside the image area is left out, as OpenJPEG clips
    /// it. The returned [`Image`] covers what is left, `x0..x1` by `y0..y1`:
    /// its [`width`](Image::width) is `x1 - x0` at full resolution and
    /// `ceil(x1 / 2^r) - ceil(x0 / 2^r)` at a reduction of `r`, and each
    /// component's is the same of `ceil(x0 / dx)` and `ceil(x1 / dx)` for its
    /// sub-sampling `dx` ([`ComponentInfo::width`] gives the formula for the
    /// whole image). The samples are those of a whole decode at the same
    /// position. [`ColorOutput::Srgb`] from sYCC with sub-sampled chroma is
    /// [`Error::Unsupported`] when the window, cut to the image area, does not
    /// cover all of it, as the chroma replication depends on where the
    /// decoded area starts.
    ///
    /// Only the tiles the window reaches are decoded, so a small window of a
    /// large tiled image costs a few tiles. The packets of the other tiles
    /// are not read, so damage in them is not found, as in OpenJPEG. With
    /// [`strict`](Self::strict) off, a codestream cut short after the tiles
    /// the window reaches still gives the window's samples.
    /// `opj_decompress -allow-partial -d` 2.5.0 refuses such a file, while
    /// `-t` decodes each tile, the cut one included, as this decoder does. In
    /// the
    /// tiles it reaches, only the code-blocks whose wavelet support reaches
    /// the window are decoded, so damage in the others is not found either.
    /// Nor is a code-block it skips checked against the bit-plane limit, so
    /// a window can decode where the whole image is refused as
    /// [`Error::Limit`].
    ///
    /// An empty window, one with no part inside the image area, and one that
    /// holds no sample of a returned component at the reduction are
    /// [`Error::InvalidOptions`].
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Region, Window, decode, decode_with};
    ///
    /// // For a 32x24 file with its image area at the grid's origin.
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/multilayer_lossless.j2k"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.j2k")?;
    /// # */
    /// let whole = decode(&bytes).expect("decodes");
    /// let options = DecodeOptions::default().with_region(Region::Window(Window::new(8, 4, 20, 30)));
    /// let part = decode_with(&bytes, &options).expect("decodes");
    /// // The window is cut to the image: rows 4 to 23.
    /// assert_eq!((part.width, part.height), (12, 20));
    /// assert_eq!(part.components[0].sample(0, 0), whole.components[0].sample(8, 4));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Tile
    ///
    /// [`Region::Tile`] returns tile `index`'s area of the image. Tiles are
    /// numbered in raster order from 0, as the codestream's `Isot` numbers
    /// them. The result is the decode of a [`Region::Window`] set to the
    /// tile's rectangle on the reference grid: its size, its samples and its
    /// failures, the refusal of sRGB output from sub-sampled sYCC included
    /// (unless the image is one tile). [`Header::tiles`] gives the tile grid
    /// and [`TileGrid::count`] the number of tiles; an index at or past the
    /// count is [`Error::InvalidOptions`].
    #[doc(alias = "decode_area")]
    #[doc(alias = "window")]
    #[doc(alias = "tile")]
    pub region: Region,
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
    /// // For a file of three components under the irreversible color transform.
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/layers/cprl_irreversible.j2k"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.j2k")?;
    /// # */
    /// let whole = decode(&bytes).expect("decodes");
    /// let options = DecodeOptions::default().with_components(Some(vec![2, 0]));
    /// let some = decode_with(&bytes, &options).expect("decodes");
    /// assert_eq!(some.components.len(), 2);
    /// assert_eq!(some.components[0], whole.components[2]);
    /// assert_eq!(some.components[1], whole.components[0]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
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
    ///
    /// That walk, in a JP2 file whose last tile-part has `Psot = 0`, decides
    /// where the tile ends: at the end of the `jp2c` box if the packets end
    /// exactly there, otherwise at the end of the file. A walk that stops at
    /// the default bound reads to the end of the file, so the box's trailing
    /// bytes count as left after the last packet. Raising the limit can let
    /// the walk finish. The same file can then decode in full where the
    /// default gives [`Error::Codestream`] (or, with
    /// [`strict`](Self::strict) off, a report with a
    /// [`ReportEntry::LeftoverBytes`]), and no [`Error::Limit`] ever
    /// suggested the change.
    pub packet_work_limit: WorkLimit,
    /// How much work decoding the code-blocks may take, against the bound
    /// described on [`decode`]. Default [`WorkLimit::Default`].
    pub code_block_work_limit: WorkLimit,
    /// How many samples the decoded image may hold, against the limit
    /// described on [`decode`], summed over the output channels at
    /// [`resolution_reduction`](Self::resolution_reduction). Default
    /// [`WorkLimit::Default`].
    ///
    /// The default limit is 2^29 samples (2 GiB as `i32`), or 8 samples per
    /// byte of input when that is more: past 64 MiB of input, it grows with
    /// the file. The input is every byte passed in, a JP2 file's boxes
    /// included; for an [`IncrementalDecoder`] it is the bytes fed so far.
    /// Past 64 MiB, a file may ask for 32 bytes of output per byte of
    /// input, and a decode whose peak is its samples peaks at about 43 bytes
    /// per byte for a square tile and up to about 102 for a tile one sample
    /// wide: about 4.1 GiB and 9.6 GiB for a file of 100 MB, input included.
    /// A file that spends its bytes on code-block segments can peak past 43
    /// with square tiles; about 102 bytes per byte is the bound for any
    /// file.
    ///
    /// Under a window or a tile ([`region`](Self::region)), the count
    /// is the samples of the tiles the decode reads, in every component, as
    /// each of those tiles is reconstructed whole; a palette's expanded
    /// channels and sRGB output count only the returned area. A small window
    /// of a large tiled image therefore fits the default, but a window of
    /// one large tile counts the whole tile. A
    /// [`components`](Self::components) subset does not lower the count, as
    /// the packets of every component of a tile read are still walked.
    ///
    /// The limit also sets how many code-blocks and precincts one tile may
    /// hold, counted at full resolution over its components: one code-block
    /// for each 512 samples it allows and one precinct for each 2048, and at
    /// least 2^20 and 2^18. A tile past either is refused with an
    /// [`Error::Limit`] of [`LimitKind::TileCodeBlocks`] or
    /// [`LimitKind::TilePrecincts`], whose `retry_with` raises this option.
    /// Their bookkeeping is at most about a third of the output the same
    /// limit allows past 2^29 (see [code-blocks and precincts per
    /// tile](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/resource-bounds.md#code-blocks-and-precincts-per-tile)).
    ///
    /// `Raised(n)` allows `n` times the default for the same input: `n` times
    /// 2^29 samples up to 64 MiB, and `8 * n` samples per byte past it. So
    /// a factor never allows less than the default, and the factor a refusal
    /// names is the smallest that admits that file. `Unlimited` lifts the
    /// limit: the decode allocates what the header declares, 4 bytes per
    /// sample for the output, and about 1.35 to 3.2 times that in all at its
    /// peak (see [the sample limit](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/resource-bounds.md#the-sample-limit)), so a small hostile file can ask
    /// for many gigabytes. If
    /// the allocation fails the process aborts, as Rust's allocation
    /// failures do. Only an image too large for this target to
    /// address (`isize::MAX` bytes of samples) is still refused, with
    /// [`Error::Limit`]. Use it only for files from a source you trust.
    ///
    /// [`read_header_with`] checks the sample count as the decode does. It
    /// checks the tile caps below only where the main header shows them (see
    /// the exceptions on [`read_header_with`]), so a header that reads is not
    /// a promise that the decode passes the caps.
    ///
    /// Padding counts as input, so a file can raise its own limit with
    /// `COM` segments, a JP2 `xml ` box or bytes after `EOC`. For a ceiling
    /// that does not grow with the input, use
    /// [`AtMost(n)`](WorkLimit::AtMost): the decode may hold at most `n`
    /// samples, whatever the input's length, and `4 * n` bytes of output.
    /// The palette expansion and sRGB output are checked against `n` too,
    /// and so is every tile a region reads, in every component the
    /// codestream codes. The code-blocks and precincts a tile may hold are
    /// set from `n`. A refusal under it gives no retry. What else to bound
    /// around a decode, with the memory and time figures, is in [Running a
    /// decode service](https://github.com/D0ubleD0uble/rust-j2k/blob/main/SECURITY.md#running-a-decode-service).
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Error, LimitKind, WorkLimit, decode_with};
    ///
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/jpeg2000_regular_latlon.j2k"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.j2k")?;
    /// # */
    /// // At most 2^26 samples (256 MiB of output), however large the file.
    /// let options = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(1 << 26));
    /// let image = decode_with(&bytes, &options)?;
    /// assert_eq!((image.width, image.height), (16, 31));
    ///
    /// // A ceiling below the image's 496 samples refuses it, with nothing to
    /// // retry with.
    /// let options = DecodeOptions::default().with_sample_limit(WorkLimit::AtMost(400));
    /// let Err(Error::Limit(limit)) = decode_with(&bytes, &options) else {
    ///     panic!("expected the sample limit to refuse the image");
    /// };
    /// assert_eq!(limit.kind(), LimitKind::Samples);
    /// assert_eq!(limit.retry_with(), None);
    /// assert!(options.with_retry(&limit).is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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
    /// - The input may end anywhere after the first `SOT` marker, which ends
    ///   the main header: inside a tile-part, its header or its packets, with or without the `EOC`
    ///   marker. Input that ends before the first `SOT` is
    ///   [`Error::Incomplete`], as when strict. Tiles with no
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
    /// tile-components is charged against the packet work bound (under a
    /// window or tile, those of the missing tiles it reads), as are the
    /// packets after the end of the data, so a short input cannot ask for
    /// unbounded work.
    ///
    /// Strict or not, only the data a decode reads is checked. With a
    /// [`region`](Self::region) of a window or a tile, the tiles outside it
    /// are not read, with a window the code-blocks of the tiles it reaches
    /// that the window does not need are not decoded, and with
    /// [`components`](Self::components) the code-blocks of the components
    /// left out are not decoded, so damage there goes unseen, as with
    /// `opj_decompress -t` and `-d`. Nor is a code-block a decode skips
    /// checked against the bit-plane limit, so it can succeed where the
    /// whole decode is refused as [`Error::Limit`].
    ///
    /// For a codestream as it streams in: a prefix that holds every packet of
    /// the resolutions a reduction keeps decodes at that reduction to the same
    /// samples as the whole codestream, and the report's
    /// [`complete_reduction`](DecodeReport::complete_reduction) says which
    /// reduction that is. [`IncrementalDecoder`] decodes such prefixes for
    /// you: each refine goes on from where the last one stopped reading
    /// packets. The packets it reads again are listed in
    /// [`docs/incremental-decoding.md`](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/incremental-decoding.md).
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Error, decode_with, read_header};
    ///
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/jpeg2000_regular_latlon.j2k"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.j2k")?;
    /// # */
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
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[doc(alias = "allow_partial")]
    #[doc(alias = "allow-partial")]
    pub strict: bool,
    /// The fuzz targets' cap on the code-block work bound, set only by
    /// [`fuzz::with_code_block_work_cap`]; `None` everywhere else.
    pub(crate) code_block_work_cap: Option<u64>,
}

/// The derived format, with the fuzz cap shown only when it is set, so the
/// options print the same outside the fuzz targets.
impl std::fmt::Debug for DecodeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Destructured so that a new field cannot be left out.
        let DecodeOptions {
            resolution_reduction,
            max_quality_layers,
            region,
            components,
            raw_channels,
            color,
            packet_work_limit,
            code_block_work_limit,
            sample_limit,
            strict,
            code_block_work_cap,
        } = self;
        let mut s = f.debug_struct("DecodeOptions");
        s.field("resolution_reduction", resolution_reduction)
            .field("max_quality_layers", max_quality_layers)
            .field("region", region)
            .field("components", components)
            .field("raw_channels", raw_channels)
            .field("color", color)
            .field("packet_work_limit", packet_work_limit)
            .field("code_block_work_limit", code_block_work_limit)
            .field("sample_limit", sample_limit)
            .field("strict", strict);
        if let Some(cap) = code_block_work_cap {
            s.field("code_block_work_cap", cap);
        }
        s.finish()
    }
}

impl Default for DecodeOptions {
    /// Full resolution, every quality layer, the whole image, components as
    /// the file's boxes shape them, color as coded, the default limits, and strict.
    fn default() -> Self {
        DecodeOptions {
            resolution_reduction: 0,
            max_quality_layers: None,
            region: Region::Whole,
            components: None,
            raw_channels: false,
            color: ColorOutput::default(),
            packet_work_limit: WorkLimit::default(),
            code_block_work_limit: WorkLimit::default(),
            sample_limit: WorkLimit::default(),
            strict: true,
            code_block_work_cap: None,
        }
    }
}

/// How far a decode may go against one of its bounds: the
/// [`DecodeOptions::packet_work_limit`] and
/// [`DecodeOptions::code_block_work_limit`] work bounds, and the
/// [`DecodeOptions::sample_limit`] on the image's size.
///
/// Each bound scales with the size of the file, so a small file cannot ask for
/// minutes of work, and a file of up to 64 MiB is held to the sample limit's
/// floor: samples that peak at about 2.7 GiB for a square tile and several GiB
/// for a tall, narrow one, and at most about 6.4 GiB in all. Past 64 MiB the sample limit grows with the file (see
/// [the sample limit](https://github.com/D0ubleD0uble/rust-j2k/blob/main/docs/resource-bounds.md#the-sample-limit)).
/// A valid file can still go past a bound, when it codes many samples in very
/// few bytes or is very large. The decode then fails with [`Error::Limit`]. Its
/// [`LimitDetail::kind`] names the option, and [`LimitDetail::retry_with`]
/// gives a [`Raised`](WorkLimit::Raised) factor, which the message names too.
/// Treat the factor as a hint, not a measure of the file:
///
/// - For the sample limit it is exact for the check it names: the image's
///   samples are known from its header. Palette expansion or sRGB output is
///   checked later, against the same limit, and may need a higher factor.
/// - For the two work bounds it is the larger of what the tiles seen so far
///   need and twice the factor that failed, so it can be more than the file
///   needs, and the file may need more: a packet walk stops as soon as it
///   runs out, so it does not know the total, and later tiles are not
///   counted. The doubling is for a loop of retries: each costs more than
///   the last, and a file can need a factor of dozens, so a ladder that
///   doubles ends in about log2 of the factor the file needs, where one that
///   stepped up by one, or to each tile's need, would take a step for each
///   tile. Following [`DecodeOptions::with_retry`] does that. If you pick the
///   next factor yourself, double it too.
///
/// Which value to pick:
///
/// - For a file from a source you trust, use [`Unlimited`](WorkLimit::Unlimited).
///   The decode then takes as long as the file asks.
/// - To cap the samples a decode may hold, set the sample limit to
///   [`AtMost(n)`](WorkLimit::AtMost): at most `n` samples, `4 * n` bytes of
///   output, whatever the file's size or padding (see
///   [`DecodeOptions::sample_limit`]). A refusal under it gives no factor.
///   [Running a decode service](https://github.com/D0ubleD0uble/rust-j2k/blob/main/SECURITY.md#running-a-decode-service) says what to
///   bound besides.
/// - For a higher work bound, use `Raised(n)` with an `n` of your own
///   choosing, the largest cost you will accept. It is a factor of the
///   default, which grows with the file. Read the factor from
///   [`LimitDetail::retry_with`] to size it, or let
///   [`DecodeOptions::with_retry`] set it: a file that needs `Raised(3)` is
///   refused under `Raised(2)`. Do not parse the message; it is a
///   diagnostic, not a stable format.
///
/// Only these three limits can be changed, and only the sample limit takes
/// `AtMost`. The other guards also end in [`Error::Limit`], and no option
/// here raises them (see [`Error::Limit`]). A damaged file can hit a guard
/// too, so do not lift a bound on that evidence alone.
///
/// ```
/// use rust_j2k::{DecodeOptions, Error, WorkLimit, decode, decode_with};
///
/// // A file that asks for far more work than its size allows.
/// # let bytes = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/tier1_work_bound/t1bomb_1024_64_88.j2k"
/// # )).to_vec();
/// # /*
/// let bytes = std::fs::read("image.j2k")?;
/// # */
/// let image = match decode(&bytes) {
///     // The example fails if the bomb is ever admitted by default.
///     Ok(_) => panic!("expected the work bound to refuse this file"),
///     // Past a limit. `limit.kind()` says which, and `limit.retry_with()`
///     // the factor.
///     Err(Error::Limit(limit)) => {
///         println!("refused: {limit}");
///         // The file is trusted here, so lift all three limits. To keep a
///         // bound, use `WorkLimit::Raised(n)`, a factor of the default, for
///         // the one named instead.
///         let options = DecodeOptions::default()
///             .with_sample_limit(WorkLimit::Unlimited)
///             .with_code_block_work_limit(WorkLimit::Unlimited)
///             .with_packet_work_limit(WorkLimit::Unlimited);
///         decode_with(&bytes, &options).expect("a valid file decodes once the limits are lifted")
///     }
///     Err(e) => panic!("not a work-bound failure: {e}"),
/// };
/// assert_eq!((image.width, image.height), (1024, 1024));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkLimit {
    /// The bound as [`decode`] describes it.
    #[default]
    Default,
    /// The default bound times this factor: both its floor and what it
    /// allows per byte. `Raised(1)` sets the same bound as
    /// `Default`, though the two do not compare equal. `Raised(0)` is
    /// [`Error::InvalidOptions`].
    Raised(u32),
    /// No bound. For a work bound, the decode does all the work the file
    /// asks for, however long that takes; for the sample limit, it allocates
    /// all the memory the file asks for (see [`DecodeOptions::sample_limit`]).
    /// The other two limits keep their own settings, and the guards with no
    /// option (listed on [`Error::Limit`]) stay in force.
    Unlimited,
    /// A fixed ceiling of this many samples, for the
    /// [`sample_limit`](DecodeOptions::sample_limit) only. It does not grow
    /// with the input, so padding cannot raise it, and it is not a factor of
    /// the default: `AtMost(n)` allows `n` samples for a file of any size,
    /// more than the default for a small file and less for a large one.
    /// An `n` past the samples the target can address is held to that count.
    /// A refusal under it gives no [`LimitDetail::retry_with`], so
    /// [`DecodeOptions::with_retry`] gives `None`. `AtMost(0)` is
    /// [`Error::InvalidOptions`], and so is `AtMost` on
    /// [`packet_work_limit`](DecodeOptions::packet_work_limit) or
    /// [`code_block_work_limit`](DecodeOptions::code_block_work_limit).
    AtMost(u64),
}

impl WorkLimit {
    /// The bound this limit sets, from the default bound `bound`.
    pub(crate) fn apply(&self, bound: u64) -> u64 {
        match self {
            WorkLimit::Default => bound,
            WorkLimit::Raised(factor) => bound.saturating_mul(u64::from(*factor)),
            WorkLimit::Unlimited => u64::MAX,
            // Only the sample limit takes it (`check_options`), and as it is.
            WorkLimit::AtMost(samples) => *samples,
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

    /// These options with [`region`](Self::region) set to `region`.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Region, Window};
    ///
    /// let options = DecodeOptions::default().with_region(Region::Tile(3));
    /// assert_eq!(options.region, Region::Tile(3));
    /// let window = Region::Window(Window::new(0, 0, 64, 64));
    /// assert_eq!(DecodeOptions::default().with_region(window.clone()).region, window);
    /// ```
    #[must_use]
    pub fn with_region(mut self, region: Region) -> Self {
        self.region = region;
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
    /// // 3 times the default: 3 x 2^29 samples for a small file, enough for a
    /// // 20000 x 20000 RGB image, and more past 64 MiB of input.
    /// let options = DecodeOptions::default().with_sample_limit(WorkLimit::Raised(3));
    /// assert_eq!(options.sample_limit, WorkLimit::Raised(3));
    /// ```
    #[must_use]
    pub fn with_sample_limit(mut self, limit: WorkLimit) -> Self {
        self.sample_limit = limit;
        self
    }

    /// These options with the limit that refused a decode set to the one
    /// it gave to retry with, or `None` if no option raises it or the
    /// refusal gave none ([`LimitDetail::retry_with`]). Only that option
    /// changes.
    ///
    /// It is safe to call in a loop: after a work bound refusal, the next
    /// factor is at least twice the last, so a file that needs a factor of
    /// `n` takes about log2(`n`) retries, not `n`. A retry still costs a
    /// decode. The factor can be up to twice what the file needs, so a
    /// caller that keeps a cap `c` on the factor should, when the factor
    /// passes it, try once more with `WorkLimit::Raised(c)` rather than stop:
    /// a file that needs between about `c / 2` and `c` is admitted by that.
    ///
    /// ```
    /// use rust_j2k::{DecodeOptions, Error, WorkLimit, decode};
    ///
    /// // A file that asks for far more work than its size allows.
    /// # let bytes = include_bytes!(concat!(
    /// #     env!("CARGO_MANIFEST_DIR"),
    /// #     "/tests/fixtures/tier1_work_bound/t1bomb_1024_64_88.j2k"
    /// # )).to_vec();
    /// # /*
    /// let bytes = std::fs::read("image.j2k")?;
    /// # */
    /// let Err(Error::Limit(limit)) = decode(&bytes) else { panic!() };
    /// let options = DecodeOptions::default().with_retry(&limit).unwrap();
    /// assert_eq!(options.code_block_work_limit, WorkLimit::Raised(2));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn with_retry(&self, limit: &LimitDetail) -> Option<Self> {
        if !limit.kind().is_adjustable() {
            return None;
        }
        let retry = limit.retry_with()?;
        let options = self.clone();
        // Every kind is named, so a new one must be given its option here,
        // or `None`; `with_retry_is_some_exactly_for_the_adjustable_kinds`
        // holds this to `is_adjustable`.
        match limit.kind() {
            LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts => {
                Some(options.with_sample_limit(retry))
            }
            LimitKind::PacketWork => Some(options.with_packet_work_limit(retry)),
            LimitKind::CodeBlockWork => Some(options.with_code_block_work_limit(retry)),
            LimitKind::AddressSpace | LimitKind::PocVolumes | LimitKind::BitPlanes => None,
        }
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
/// # let bytes = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/jp2_cdef_rgba.jp2"
/// # )).to_vec();
/// # /*
/// let bytes = std::fs::read("image.jp2")?;
/// # */
/// let options = DecodeOptions::default()
///     .with_resolution_reduction(1)
///     .with_color(ColorOutput::Srgb);
/// match decode_with(&bytes, &options) {
///     Ok(image) => {
///         // For a 16x12 file, one level discarded halves each side.
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
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Failures are the same [`Error`] variants as [`decode`], plus
/// [`Error::InvalidOptions`] for options that are invalid on their own
/// ([`WorkLimit::Raised`]`(0)`, a [`max_quality_layers`](DecodeOptions::max_quality_layers)
/// of `Some(0)`), contradict each other, or do not fit the file.
pub fn decode_with(bytes: &[u8], options: &DecodeOptions) -> Result<Image> {
    pipeline::decode::decode_metered(bytes, options, None)
}

// The Tier-1 work-bound tests still name `tile_bytes` at the root.
#[cfg(test)]
pub(crate) use pipeline::decode::tile_bytes;

// The synthetic-bytes builders the unit tests share with `tests/` and `fuzz/`.
// Std only, so one `#[path]` include here serves every unit test.
#[cfg(test)]
#[path = "../tests/support/synth.rs"]
mod synth;
