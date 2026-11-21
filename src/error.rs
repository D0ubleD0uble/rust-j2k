//! Error type for the decoder. One flat enum; each failure maps to the variant
//! that names its class, so a caller can tell a malformed input from an
//! unsupported feature: [`Error::Codestream`] for structural damage,
//! [`Error::Incomplete`] for input that ends before the main header is whole,
//! [`Error::Marker`] for a marker segment that says something invalid (a
//! field, a length or a conflict with another segment), [`Error::FileFormat`] for a
//! malformed JP2 box structure, [`Error::Unsupported`] for
//! a valid-but-out-of-subset feature, [`Error::Limit`] for an input past a
//! decoder resource guard, [`Error::InvalidOptions`] for bad caller options,
//! and [`Error::Inconsistent`] for a violated internal invariant. Structural
//! corruption anywhere in the pipeline — including packet headers and coded
//! data — reports as [`Error::Codestream`].

use core::fmt;

use crate::WorkLimit;

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Error>;

/// A decode failure, tagged by the class of fault.
///
/// Match on the variant to decide what to do. The [`ErrorDetail`] inside
/// holds a diagnostic for a log; its wording can change between releases, so
/// do not parse it or compare it. [`Error::Limit`] holds a [`LimitDetail`]
/// instead, which also says which limit refused the input and, when an
/// option raises that limit, the [`WorkLimit`] to retry with. `Error` does
/// not implement `PartialEq`: test the variant with `matches!`, and a limit
/// by its [`LimitKind`]. Input that is not JPEG 2000 at all, plain text
/// included, is [`Error::Codestream`]; empty input is [`Error::Incomplete`].
///
/// The variant also says what to tell a user:
///
/// | Variant | A message for a user |
/// | ------- | -------------------- |
/// | `Codestream` | The file is damaged, cut short or not a JPEG 2000 image. |
/// | `Marker` | The file is damaged: a header field or segment length holds a value the standard does not allow. |
/// | `FileFormat` | The file is damaged: its JP2 boxes are malformed or disagree with the image. |
/// | `Incomplete` | The file is cut short before its header ends. If it is still arriving, try again with more of it, or feed it to an [`IncrementalDecoder`](crate::IncrementalDecoder). |
/// | `Unsupported` | The file uses a JPEG 2000 feature this decoder does not read, or a valid request it does not do: sRGB output from sub-sampled sYCC chroma on an area smaller than the image (`DecodeOptions::region`). |
/// | `Limit` | The file is larger or costlier than the decoder allows by default. If [`LimitDetail::retry_with`] gives a [`WorkLimit`], it can be retried with that limit raised. |
/// | `InvalidOptions` | The decode options are invalid, for any file (`WorkLimit::Raised(0)`, `WorkLimit::AtMost(0)`, `AtMost` on a work bound, `max_quality_layers` of `Some(0)`, sRGB output with `raw_channels` or `components`, an empty window, no component or one named twice) or for this one (a reduction past the file's levels, a window outside the image, a tile or component the image does not have, an area that holds no sample of a returned component at the reduction). |
/// | `Inconsistent` | An internal error; please file a bug. |
///
/// Damage does not always show as the first row. A flipped bit in a header
/// field can read as an absurd image size (`Limit`), an unknown marker or a
/// feature the decoder does not read (`Unsupported`), so a corrupt file can
/// come back as any of those. Do not retry a `Limit` with
/// [`WorkLimit::Unlimited`] unless the source
/// is trusted.
///
/// When an input has more than one fault, which one is reported can change in
/// any release. What holds is that when
/// [`read_header_with`](crate::read_header_with) returns an error,
/// [`decode_with`](crate::decode_with) returns the same one, and a codestream
/// and its JP2 wrapper report the same one. The exceptions are:
///
/// - sRGB output, whose check comes at a different point for a raw codestream
///   and for a JP2 file.
/// - The sample limit and the tile caps, which count a JP2 file's own bytes.
///   Past 64 MiB the wrapper can admit what the bare codestream refuses, or
///   refuse it with a different message: a different sample count, or a
///   smaller factor to retry with.
/// - A [`Error::Limit`] or [`Error::InvalidOptions`] the header read gives for
///   a tile, where the decode first refuses the file for something only it
///   checks: a fault or a work bound in a lower tile's data, or, with
///   `strict` off, the work charged for tiles that have no tile-part.
///
/// Separately, the header read can return `Ok` for a fault only decoding
/// finds.
///
/// ```
/// use rust_j2k::{Error, decode};
///
/// fn explain(error: &Error) -> &'static str {
///     match error {
///         Error::Codestream(_) | Error::Marker(_) | Error::FileFormat(_) => {
///             "the file is damaged, truncated or not a JPEG 2000 image"
///         }
///         Error::Incomplete(_) => "the file is cut short before its header ends",
///         Error::Unsupported(_) => "the file uses a feature this decoder does not read",
///         Error::Limit(_) => "the file is larger or costlier than the decoder allows",
///         Error::InvalidOptions(_) => "the decode options do not fit this file",
///         // The enum is `#[non_exhaustive]`: a later version can add variants.
///         _ => "the file could not be decoded",
///     }
/// }
///
/// let error = decode(b"hello").unwrap_err();
/// assert!(matches!(error, Error::Codestream(_)));
/// assert_eq!(
///     explain(&error),
///     "the file is damaged, truncated or not a JPEG 2000 image"
/// );
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    /// Not a JPEG 2000 codestream, or its structure is broken: how the
    /// markers and the data are laid out, not what one segment says. That
    /// is a marker that is missing, repeated or out of place (a `PPT` beside
    /// a `PPM` included); the tile-parts that `SOT` lays out (its tile
    /// index, `Psot`, `TPsot` and `TNsot`) and the `PPM` chunks they take;
    /// a tile-part header that runs past its `Psot`, or anything that runs
    /// past the end of the input; packets the `POC` volumes leave out; and
    /// corrupt packet or coded data, `SOP` sequence numbers and `EPH`
    /// included. Input that ends before the main header is whole is
    /// [`Error::Incomplete`] instead. Any input that is not JPEG 2000 at all
    /// (text, another image format) is this variant, since it has no `SOC`
    /// marker.
    ///
    /// A tile-part header is read no further than its `Psot`, so a segment
    /// that runs past `Psot` is this variant, whatever its fields hold; a
    /// bad field in a segment within `Psot` is [`Error::Marker`], unless the
    /// header runs past its `Psot`, which is refused first. A later
    /// tile-part that restates `COD`, `COC`, `QCD` or `QCC` with
    /// values other than those in force for the tile is out of place, so it
    /// is this variant too.
    ///
    /// A main header of more than 65,536 marker segments is this variant
    /// too, not [`Error::Limit`]: no valid main header comes near it, and no
    /// option raises it.
    Codestream(ErrorDetail),
    /// The input ends before the codestream's main header is whole, that is,
    /// before its first `SOT` marker (the start of the first
    /// [tile-part](https://github.com/D0ubleD0uble/rust-j2k/blob/main/README.md#what-a-jpeg-2000-file-holds)) is complete. No top-level box or segment
    /// the input holds in full was found damaged, so more bytes may still give
    /// a valid header. The boxes inside a superbox (such as `ihdr` in `jp2h`)
    /// are judged only once the superbox is whole, so a cut `jp2h` is this
    /// variant even when its `ihdr` is already damaged.
    ///
    /// It begins with the empty input. A raw codestream is this variant from
    /// 0 bytes up to its first `SOT` marker. A JP2 file is this variant from
    /// 0 bytes as well: inside its signature box, at or inside any top-level
    /// box before the `jp2c` box, inside the `jp2c` box header, and then in
    /// the codestream's main header. A box inside a superbox, such as
    /// `ihdr` in `jp2h`, is only read once the superbox is whole, so its faults
    /// stay [`Error::FileFormat`].
    ///
    /// This is the "wait for more bytes" signal for a caller whose input
    /// streams in. [`read_header`](crate::read_header) and
    /// [`decode_with`](crate::decode_with) return it whether or not
    /// [`strict`](crate::DecodeOptions::strict) is set, since every decode
    /// needs the whole main header.
    ///
    /// The boxes and segments the input holds in full are judged first: a
    /// damaged one gives an error, not this variant, usually the one the
    /// whole file gives ([`Error::FileFormat`], [`Error::Codestream`],
    /// [`Error::Marker`], [`Error::Unsupported`] or [`Error::Limit`]). So is
    /// a box cut short whose type is already wrong, such as a second box that
    /// is not `ftyp`. The whole file can give another one when a later
    /// part of its header is broken too. The checks that tie segments to each
    /// other or to the JP2 boxes wait for the whole header (for example, a
    /// missing `COD` or `QCD`, quantization coverage, 9/7 without
    /// quantization, MCT geometry, `PPM` order, and `ihdr` against SIZ).
    ///
    /// For a file that is still arriving, an
    /// [`IncrementalDecoder`](crate::IncrementalDecoder) waits for the header
    /// itself: its `refine` waits with
    /// [`Waiting::HeaderIncomplete`](crate::Waiting::HeaderIncomplete) while
    /// the header is not whole.
    ///
    /// If no more bytes are coming, the file is truncated. A box or segment
    /// length that is damaged so that it reaches past the end of the input
    /// looks the same as a cut, so it is this variant too. Input that can no
    /// longer become JPEG 2000 is [`Error::Codestream`]: a byte that is not
    /// the start of the `SOC` marker or of the JP2 signature box, for example.
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
    /// // The first 40 bytes end inside the main header: wait for more.
    /// assert!(matches!(read_header(&bytes[..40]), Err(Error::Incomplete(_))));
    /// let options = DecodeOptions::default().with_strict(false);
    /// assert!(matches!(
    ///     decode_with(&bytes[..40], &options),
    ///     Err(Error::Incomplete(_))
    /// ));
    /// // Once the main header is there, the header reads, and a decode with
    /// // `strict` off gives the image the data holds.
    /// assert!(read_header(&bytes[..600]).is_ok());
    /// assert!(decode_with(&bytes[..600], &options).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    Incomplete(ErrorDetail),
    /// A marker segment says something invalid, wherever it is: a field with
    /// a reserved or out-of-range value, a length that does not fit the
    /// fields the segment carries (too short, bytes left over, not a whole
    /// number of entries, or below the segment's minimum), or a coding
    /// parameter that another segment rules out. That last covers a
    /// component index past `SIZ`'s count, a `TLM` tile index past the
    /// grid, an `RGN` or a MIXED code-block style that `CAP` rules out,
    /// quantization steps too few for the decomposition levels, a color
    /// transform over components that differ. A `SIZ` whose image holds no
    /// sample of a component is `Marker` too, though `SIZ` alone decides
    /// it. A segment whose length runs
    /// past the end of the input is [`Error::Codestream`] or
    /// [`Error::Incomplete`] instead, since the input may be cut short.
    Marker(ErrorDetail),
    /// The JP2 box structure or a box field is malformed: a box that runs past
    /// the superbox holding it, a truncated box after the codestream, a
    /// missing or misplaced required box, an invalid field value, or a box
    /// that disagrees with the codestream.
    FileFormat(ErrorDetail),
    /// A valid-but-not-decoded feature: an HTJ2K code-block that needs more
    /// than one HT coding pass set (`MULTIHT` in the `CAP` marker), a Part 2
    /// extension, a marker or coding style outside the decoded subset, a JP2
    /// box this decoder cannot apply, a file that is not JP2-compatible. It
    /// is also a request the decoder does not do, such as sRGB output from a
    /// file it cannot convert.
    Unsupported(ErrorDetail),
    /// The input exceeded one of the decoder's resource guards: a hostile or
    /// absurd declaration (code-block count, precinct count, bit-plane depth)
    /// past a cap, an image of more samples than the sample limit allows, or
    /// work past the bound for the file's size (see [`decode`](crate::decode)).
    /// It is not a missing feature (that is [`Error::Unsupported`]), and the
    /// file may be valid.
    ///
    /// Three of these can be changed, each with a [`WorkLimit`]:
    /// the sample limit
    /// ([`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit)),
    /// which holds the image area and the samples that palette expansion, sYCC
    /// chroma replication and a monochrome ICC conversion produce, and with
    /// them the code-blocks and precincts a tile may hold (one code-block for
    /// each 512 samples it allows and one precinct for each 2048, and at least
    /// 2^20 and 2^18); reading the packet headers
    /// ([`DecodeOptions::packet_work_limit`](crate::DecodeOptions::packet_work_limit));
    /// and decoding the code-blocks
    /// ([`DecodeOptions::code_block_work_limit`](crate::DecodeOptions::code_block_work_limit)).
    /// For one of them, [`LimitDetail::kind`] names the limit, and
    /// [`LimitDetail::retry_with`] gives the `WorkLimit` to retry with, which
    /// the message names too. It is a hint (see
    /// [`WorkLimit`]), not a measure of the file. A
    /// refusal under a sample limit of
    /// [`WorkLimit::AtMost`] gives none: the
    /// caller set that ceiling.
    ///
    /// The other guards have no option, so a file past one of them is refused
    /// whatever the options. A tile may hold at most 32 progression order
    /// change (POC) volumes; a code-block may have at most 30 bit-planes;
    /// and under a `sample_limit` of `WorkLimit::Unlimited`, an image too
    /// large for the target to address is still refused. For these,
    /// `retry_with` is `None`. [`LimitDetail`] has an example that retries.
    Limit(LimitDetail),
    /// The caller passed [`DecodeOptions`](crate::DecodeOptions) that are
    /// invalid for this codestream, or that are invalid whatever the input
    /// (sRGB output with `raw_channels`, a limit of `WorkLimit::Raised(0)` or
    /// `WorkLimit::AtMost(0)`, or `AtMost` on a work bound).
    /// The input itself may be fine.
    InvalidOptions(ErrorDetail),
    /// A decoder invariant was violated (declared geometry and decoded state
    /// disagreed where they never should); please file a bug.
    Inconsistent(ErrorDetail),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Codestream(d) => write!(f, "codestream: {d}"),
            Error::Incomplete(d) => write!(f, "incomplete: {d}"),
            Error::Marker(d) => write!(f, "marker: {d}"),
            Error::FileFormat(d) => write!(f, "file format: {d}"),
            Error::Unsupported(d) => write!(f, "unsupported: {d}"),
            Error::Limit(d) => write!(f, "limit: {d}"),
            Error::InvalidOptions(d) => write!(f, "invalid options: {d}"),
            Error::Inconsistent(d) => write!(f, "decoder bug (please report it): {d}"),
        }
    }
}

impl Error {
    /// The diagnostic, without the prefix that `Display` adds for the
    /// variant. Its wording can change between releases, so do not parse it
    /// or compare it.
    ///
    /// ```
    /// let error = rust_j2k::decode(b"hello").unwrap_err();
    /// assert_eq!(error.to_string(), format!("codestream: {}", error.message()));
    /// ```
    pub fn message(&self) -> &str {
        match self {
            Error::Codestream(d)
            | Error::Incomplete(d)
            | Error::Marker(d)
            | Error::FileFormat(d)
            | Error::Unsupported(d)
            | Error::InvalidOptions(d)
            | Error::Inconsistent(d) => d.message(),
            Error::Limit(d) => d.message(),
        }
    }
}

impl std::error::Error for Error {}

/// The constructors the decoder uses, one for each variant.
impl Error {
    pub(crate) fn codestream(message: impl Into<String>) -> Error {
        Error::Codestream(ErrorDetail::new(message))
    }

    pub(crate) fn incomplete(message: impl Into<String>) -> Error {
        Error::Incomplete(ErrorDetail::new(message))
    }

    pub(crate) fn marker(message: impl Into<String>) -> Error {
        Error::Marker(ErrorDetail::new(message))
    }

    pub(crate) fn file_format(message: impl Into<String>) -> Error {
        Error::FileFormat(ErrorDetail::new(message))
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Error {
        Error::Unsupported(ErrorDetail::new(message))
    }

    pub(crate) fn invalid_options(message: impl Into<String>) -> Error {
        Error::InvalidOptions(ErrorDetail::new(message))
    }

    pub(crate) fn inconsistent(message: impl Into<String>) -> Error {
        Error::Inconsistent(ErrorDetail::new(message))
    }

    /// An [`Error::Limit`] for a `kind` that no option raises.
    pub(crate) fn limit_fixed(kind: LimitKind, message: impl Into<String>) -> Error {
        debug_assert!(!kind.is_adjustable(), "{kind:?} has an option");
        Error::limit(kind, None, message.into())
    }

    /// An [`Error::Limit`] for an adjustable `kind`, where `retry_with` is
    /// the limit that may admit the input.
    pub(crate) fn limit_retry(
        kind: LimitKind,
        retry_with: WorkLimit,
        message: impl Into<String>,
    ) -> Error {
        debug_assert!(kind.is_adjustable(), "{kind:?} has no option");
        Error::limit(kind, Some(retry_with), message.into())
    }

    /// An [`Error::Limit`] for an adjustable `kind` whose refusal shows no
    /// limit to retry with.
    pub(crate) fn limit_unhinted(kind: LimitKind, message: impl Into<String>) -> Error {
        debug_assert!(kind.is_adjustable(), "{kind:?} has no option");
        Error::limit(kind, None, message.into())
    }

    fn limit(kind: LimitKind, retry_with: Option<WorkLimit>, message: String) -> Error {
        debug_assert!(
            names_its_data(kind, retry_with.as_ref(), &message),
            "{kind:?}, {retry_with:?}: {message}"
        );
        Error::Limit(LimitDetail {
            kind,
            retry_with,
            refused_at: None,
            message,
        })
    }

    /// This [`Error::Limit`] of a kind that carries it, with the tile it
    /// refused and the amount that tile needs as data: the figures its
    /// message names. An error of another kind is returned as it is.
    pub(crate) fn refused_at(self, tile: u32, amount: u64) -> Error {
        match self {
            Error::Limit(mut limit) => {
                let named = match limit.kind {
                    LimitKind::CodeBlockWork => {
                        format!("tile {tile} needs {amount} sample-passes ")
                    }
                    LimitKind::TileCodeBlocks => {
                        format!("tile {tile} holds {amount} code-blocks, ")
                    }
                    LimitKind::TilePrecincts => format!("tile {tile} holds {amount} precincts, "),
                    kind => {
                        debug_assert!(
                            matches!(kind, LimitKind::AddressSpace),
                            "{kind:?} carries no refused_at"
                        );
                        return Error::Limit(limit);
                    }
                };
                debug_assert!(limit.message.starts_with(&named), "{}", limit.message);
                limit.refused_at = Some(RefusedAt { tile, amount });
                Error::Limit(limit)
            }
            other => other,
        }
    }
}

/// Whether a limit's `message` says what its data does: the `retry_with`
/// limit, if there is one, and then the option `kind` names. A limit with
/// no option names none.
fn names_its_data(kind: LimitKind, retry_with: Option<&WorkLimit>, message: &str) -> bool {
    let option = match kind {
        LimitKind::Samples | LimitKind::TileCodeBlocks | LimitKind::TilePrecincts => {
            Some("`DecodeOptions::sample_limit`")
        }
        LimitKind::PacketWork => Some("`DecodeOptions::packet_work_limit`"),
        LimitKind::CodeBlockWork => Some("`DecodeOptions::code_block_work_limit`"),
        _ => None,
    };
    let retry = match retry_with {
        Some(WorkLimit::Raised(n)) => message.contains(&format!("`WorkLimit::Raised({n})`")),
        Some(WorkLimit::Unlimited) => message.contains("only `WorkLimit::Unlimited`"),
        Some(_) => false,
        None => !message.contains("`WorkLimit::Raised("),
    };
    retry
        && match option {
            Some(option) => retry_with.is_none() || message.contains(option),
            None => retry_with.is_none() && !message.contains("DecodeOptions::"),
        }
}

/// What an [`Error`] other than [`Error::Limit`] carries: a diagnostic for a
/// log.
///
/// [`Display`](fmt::Display) and [`message`](Self::message) give the text.
/// Its wording can change between releases, so do not parse it or compare
/// it.
// The fields are private so that a source error can join later without a
// break: held in an `Arc` it keeps `Clone`, and the auto traits hold as long
// as the field is unwind-safe (an `AssertUnwindSafe` around it, as `anyhow`
// does, since an error is read through `&` only).
#[derive(Clone)]
#[non_exhaustive]
pub struct ErrorDetail {
    message: String,
}

impl ErrorDetail {
    fn new(message: impl Into<String>) -> Self {
        ErrorDetail {
            message: message.into(),
        }
    }

    /// The diagnostic, without the prefix that the [`Error`]'s `Display`
    /// adds for the variant.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Debug for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.message, f)
    }
}

impl fmt::Display for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// What an [`Error::Limit`] carries: which limit refused the input, the
/// [`WorkLimit`] to retry with when an option raises that limit, and a
/// diagnostic for a log.
///
/// ```
/// use rust_j2k::{DecodeOptions, Error, LimitKind, decode, decode_with};
///
/// // A file that asks for far more work than its size allows.
/// # let bytes = include_bytes!(concat!(
/// #     env!("CARGO_MANIFEST_DIR"),
/// #     "/tests/fixtures/tier1_work_bound/t1bomb_1024_64_88.j2k"
/// # )).to_vec();
/// # /*
/// let bytes = std::fs::read("image.j2k")?;
/// # */
/// let Err(Error::Limit(limit)) = decode(&bytes) else {
///     panic!("expected the code-block work bound to refuse this file");
/// };
/// assert_eq!(limit.kind(), LimitKind::CodeBlockWork);
/// # assert!(DecodeOptions::default().with_retry(&limit).is_some());
/// match DecodeOptions::default().with_retry(&limit) {
///     // An option raises this limit: retry with it raised.
///     Some(options) => {
///         let image = decode_with(&bytes, &options)?;
///         println!("{}x{}", image.width, image.height);
///     }
///     // No option raises this limit (`limit.kind().is_adjustable()` is
///     // false), or the refusal gave no factor: the file is refused
///     // whatever the options.
///     None => eprintln!("refused: {limit}"),
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LimitDetail {
    kind: LimitKind,
    retry_with: Option<WorkLimit>,
    refused_at: Option<RefusedAt>,
    message: String,
}

impl LimitDetail {
    /// Which limit refused the input.
    pub fn kind(&self) -> LimitKind {
        self.kind
    }

    /// What to set the option that [`kind`](Self::kind) names to, for a
    /// retry that may admit the input; `None` when no option raises this
    /// limit.
    ///
    /// It is the [`WorkLimit`] the message names: a
    /// [`Raised`](WorkLimit::Raised) factor, or
    /// [`Unlimited`](WorkLimit::Unlimited) for an image past every factor.
    /// It is worked out from what the refusal shows, so a retry with it can
    /// still be refused, by this limit later in the file or by another
    /// limit (see [`WorkLimit`]), or fail with another error once past it,
    /// such as damage the limit stopped the decode before. A work bound
    /// refusal names at least twice the factor it ran under, so retries in a
    /// loop take about log2 of the factor the file needs. It is also `None`
    /// for a packet work refusal that shows no factor, such as one while
    /// reading the header, and for a sample limit refusal under
    /// [`AtMost`](WorkLimit::AtMost), a ceiling the caller set.
    ///
    /// [`DecodeOptions::with_retry`](crate::DecodeOptions::with_retry)
    /// applies it to the option `kind` names. For
    /// [`TileCodeBlocks`](LimitKind::TileCodeBlocks) and
    /// [`TilePrecincts`](LimitKind::TilePrecincts) that option is the sample
    /// limit, so a retry raises the limit on the whole image's samples, not
    /// only the tile's count.
    pub fn retry_with(&self) -> Option<WorkLimit> {
        self.retry_with.clone()
    }

    /// The tile the limit refused and the amount it needs, where the refusal
    /// knows both exactly; `None` otherwise.
    ///
    /// The refusals that carry it, by [`kind`](Self::kind):
    ///
    /// - [`CodeBlockWork`](LimitKind::CodeBlockWork): the refusals of one
    ///   tile by the code-block work bound, the one
    ///   [`decode_with`](crate::decode_with) gives, and the one an
    ///   [`IncrementalDecoder`](crate::IncrementalDecoder) waits on as
    ///   [`Waiting::WorkLimit`](crate::Waiting::WorkLimit), or as
    ///   [`Waiting::BoundSpent`](crate::Waiting::BoundSpent) where the whole
    ///   bound refuses a tile it counted. The amount is in sample-passes.
    /// - [`TileCodeBlocks`](LimitKind::TileCodeBlocks): the tile that holds
    ///   more code-blocks than a tile may. The amount is its code-blocks.
    ///   It counts the whole tile: every component and resolution, whatever
    ///   the reduction, region or components of this decode, so reducing
    ///   the resolution does not get the tile past the cap.
    /// - [`TilePrecincts`](LimitKind::TilePrecincts): the tile that holds
    ///   more precincts than a tile may. The amount is its precincts, counted
    ///   the same way.
    ///
    /// For other kinds it is `None` today, and a later release may set it.
    /// So is it for a `BoundSpent` whose count ended within the whole bound.
    /// On a target that cannot address the tile's samples, a tile cap
    /// refusal is [`AddressSpace`](LimitKind::AddressSpace) and gives `None`.
    ///
    /// The tile pairs with [`TileGrid::tile`](crate::TileGrid::tile) and
    /// [`Region::Tile`](crate::Region::Tile), so a caller can decode around
    /// the tile refused. It names the first tile refused; later tiles may be
    /// refused too, so a caller that decodes the others may get another
    /// refusal. The example only shows the calls (it does not run):
    ///
    /// ```no_run
    /// use rust_j2k::{DecodeOptions, Error, Region, decode_with};
    ///
    /// fn decode_tiles_but_the_refused(
    ///     bytes: &[u8],
    ///     tile_count: u32,
    /// ) -> Result<Vec<rust_j2k::Image>, Error> {
    ///     let options = DecodeOptions::default();
    ///     let refused = match decode_with(bytes, &options) {
    ///         Err(Error::Limit(limit)) => limit.refused_at().map(|at| at.tile()),
    ///         _ => None,
    ///     };
    ///     (0..tile_count)
    ///         .filter(|&tile| Some(tile) != refused)
    ///         .map(|tile| decode_with(bytes, &options.clone().with_region(Region::Tile(tile))))
    ///         .collect()
    /// }
    /// ```
    pub fn refused_at(&self) -> Option<RefusedAt> {
        self.refused_at
    }

    /// The diagnostic, without the `limit: ` prefix that the [`Error`]'s
    /// `Display` adds. Do not parse it: [`kind`](Self::kind),
    /// [`retry_with`](Self::retry_with) and [`refused_at`](Self::refused_at)
    /// hold its data.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LimitDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Where a limit refused the input, from
/// [`LimitDetail::refused_at`]: the tile, and the amount it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RefusedAt {
    tile: u32,
    amount: u64,
}

impl RefusedAt {
    /// The index of the tile refused, in raster order from 0, as
    /// [`TileGrid::tile`](crate::TileGrid::tile) takes it.
    pub fn tile(&self) -> u32 {
        self.tile
    }

    /// The amount the tile needs, in the unit of the limit's
    /// [`kind`](LimitDetail::kind): sample-passes for
    /// [`CodeBlockWork`](LimitKind::CodeBlockWork) (the work of the
    /// code-blocks this decode reads under its options, the reduction,
    /// region and components, not of the whole tile), code-blocks for
    /// [`TileCodeBlocks`](LimitKind::TileCodeBlocks) and precincts for
    /// [`TilePrecincts`](LimitKind::TilePrecincts). The last two count the
    /// whole tile, every component and resolution, whatever the reduction,
    /// region or components.
    pub fn amount(&self) -> u64 {
        self.amount
    }
}

/// Which limit an [`Error::Limit`] names.
///
/// [`is_adjustable`](Self::is_adjustable) says which a
/// [`DecodeOptions`](crate::DecodeOptions) limit raises, with a
/// [`WorkLimit`]. No option raises the others, so they refuse the input
/// whatever the options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// The sample limit,
    /// [`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit):
    /// the image area, and the samples that palette expansion, sYCC chroma
    /// replication and a monochrome ICC conversion produce.
    Samples,
    /// The packet work bound,
    /// [`DecodeOptions::packet_work_limit`](crate::DecodeOptions::packet_work_limit),
    /// on reading the packet headers.
    PacketWork,
    /// The code-block work bound,
    /// [`DecodeOptions::code_block_work_limit`](crate::DecodeOptions::code_block_work_limit),
    /// on decoding the code-blocks.
    CodeBlockWork,
    /// An image of more samples than the target can address, or a tile whose
    /// code-blocks or precincts need a sample limit past what it can address
    /// to admit them: refused under any sample limit.
    AddressSpace,
    /// A tile of more code-blocks than the sample limit,
    /// [`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit),
    /// allows a tile: one for each 512 samples it allows, and at least 2^20.
    TileCodeBlocks,
    /// A tile of more precincts than the sample limit,
    /// [`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit),
    /// allows a tile: one for each 2048 samples it allows, and at least 2^18.
    TilePrecincts,
    /// A tile's progression order change (`POC`) volumes past a fixed cap:
    /// more than 32 of them.
    PocVolumes,
    /// A code-block of more than 30 bit-planes.
    BitPlanes,
}

impl LimitKind {
    /// Whether a [`DecodeOptions`](crate::DecodeOptions) limit raises this
    /// one: true for [`Samples`](Self::Samples),
    /// [`PacketWork`](Self::PacketWork),
    /// [`CodeBlockWork`](Self::CodeBlockWork), and
    /// [`TileCodeBlocks`](Self::TileCodeBlocks) and
    /// [`TilePrecincts`](Self::TilePrecincts), which the sample limit raises.
    /// A tile whose geometry needs a sample limit past what the target can
    /// address is [`AddressSpace`](Self::AddressSpace) instead, which is not.
    pub fn is_adjustable(self) -> bool {
        matches!(
            self,
            LimitKind::Samples
                | LimitKind::PacketWork
                | LimitKind::CodeBlockWork
                | LimitKind::TileCodeBlocks
                | LimitKind::TilePrecincts
        )
    }
}

/// `result` with its error as its `Debug` text, so that two results compare
/// by variant, data and message in a test: [`Error`] has no `PartialEq`.
#[cfg(test)]
pub(crate) fn keyed<T>(result: Result<T>) -> core::result::Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

/// Whether `error` is an [`Error::Limit`] for `kind`.
#[cfg(test)]
pub(crate) fn is_limit(error: &Error, kind: LimitKind) -> bool {
    matches!(error, Error::Limit(limit) if limit.kind() == kind)
}

/// Whether `error` is an [`Error::Limit`] for `kind` that no option raises,
/// so that it gives no limit to retry with.
#[cfg(test)]
pub(crate) fn is_fixed_limit(error: &Error, kind: LimitKind) -> bool {
    matches!(error, Error::Limit(limit) if limit.kind() == kind && limit.retry_with().is_none())
}

/// A [`LimitDetail`] of any `kind` and `retry_with`, past the checks the
/// constructors make, for a test of what a caller does with one.
#[cfg(test)]
pub(crate) fn any_limit(kind: LimitKind, retry_with: Option<WorkLimit>) -> LimitDetail {
    LimitDetail {
        kind,
        retry_with,
        refused_at: None,
        message: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `keyed` compares errors by their `Debug` text, so that text must hold
    // the variant, the message and a limit's data.
    #[test]
    fn debug_shows_the_variant_the_data_and_the_message() {
        assert_eq!(
            format!("{:?}", crate::decode(b"hello").unwrap_err()),
            "Codestream(\"does not start with the `SOC` marker\")"
        );
        let limit = Error::limit_fixed(LimitKind::BitPlanes, "31 bit-planes");
        assert_eq!(
            format!("{limit:?}"),
            "Limit(LimitDetail { kind: BitPlanes, retry_with: None, refused_at: None, message: \"31 \
             bit-planes\" })"
        );
    }

    #[test]
    fn message_drops_the_prefix() {
        let error = Error::marker("reserved");
        assert_eq!(error.to_string(), "marker: reserved");
        assert_eq!(error.message(), "reserved");
        let limit = Error::limit_fixed(LimitKind::PocVolumes, "33 volumes");
        assert_eq!(limit.message(), "33 volumes");
    }

    #[test]
    fn the_three_options_raise_the_adjustable_kinds() {
        use LimitKind::*;
        for kind in [
            Samples,
            PacketWork,
            CodeBlockWork,
            TileCodeBlocks,
            TilePrecincts,
        ] {
            assert!(kind.is_adjustable(), "{kind:?}");
        }
        for kind in [AddressSpace, PocVolumes, BitPlanes] {
            assert!(!kind.is_adjustable(), "{kind:?}");
        }
    }
}
