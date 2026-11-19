//! Error type for the decoder. One flat enum; each failure maps to the variant
//! that names its class, so a caller can tell a malformed input from an
//! unsupported feature: [`Error::Codestream`] for structural damage,
//! [`Error::Incomplete`] for input that ends before the main header is whole,
//! [`Error::Marker`] for an invalid field value, [`Error::FileFormat`] for a
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
/// | `Marker` | The file is damaged: a header field holds a value the standard does not allow. |
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
/// [`WorkLimit::Unlimited`](crate::WorkLimit::Unlimited) unless the source
/// is trusted.
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
    /// Not a JPEG 2000 codestream, or its structure is broken: a missing
    /// segment, a segment too short for its fields, lost sync, a tile-part or
    /// packet data cut short, or corrupt packet / coded data. Input that ends
    /// before the main header is whole is [`Error::Incomplete`] instead. Any
    /// input that is not JPEG 2000 at all (text, another image format) is
    /// this variant, since it has no `SOC` marker.
    Codestream(ErrorDetail),
    /// The input ends before the codestream's main header is whole, that is,
    /// before its first `SOT` marker (the start of the first
    /// [tile-part](https://github.com/D0ubleD0uble/rust-j2k/blob/v0.5.0/README.md#what-a-jpeg-2000-file-holds)) is complete. No top-level box or segment
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
    /// A marker segment has an invalid field: a reserved value, a bad length
    /// or an out-of-range value.
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
    /// Three of these can be changed, each with a [`WorkLimit`](crate::WorkLimit):
    /// the sample limit
    /// ([`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit)),
    /// which holds the image area and the samples that palette expansion, sYCC
    /// chroma replication and a monochrome ICC conversion produce; reading the
    /// packet headers
    /// ([`DecodeOptions::packet_work_limit`](crate::DecodeOptions::packet_work_limit));
    /// and decoding the code-blocks
    /// ([`DecodeOptions::code_block_work_limit`](crate::DecodeOptions::code_block_work_limit)).
    /// For one of them, [`LimitDetail::kind`] names the limit, and
    /// [`LimitDetail::retry_with`] gives the `WorkLimit` to retry with, which
    /// the message names too. It is a hint (see
    /// [`WorkLimit`](crate::WorkLimit)), not a measure of the file. A
    /// refusal under a sample limit of
    /// [`WorkLimit::AtMost`](crate::WorkLimit::AtMost) gives none: the
    /// caller set that ceiling.
    ///
    /// The other guards have no option, so a file past one of them is refused
    /// whatever the options. A tile may hold at most 2^20 code-blocks and
    /// 2^18 precincts, and 32 progression order change (POC) volumes; a
    /// code-block may have at most 30 bit-planes; and under a `sample_limit`
    /// of `WorkLimit::Unlimited`, an image too large for the target to
    /// address is still refused. For these, `retry_with` is `None`.
    /// [`LimitDetail`] has an example that retries.
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
            message,
        })
    }
}

/// Whether a limit's `message` says what its data does: the `retry_with`
/// limit, if there is one, and then the option `kind` names. A limit with
/// no option names none.
fn names_its_data(kind: LimitKind, retry_with: Option<&WorkLimit>, message: &str) -> bool {
    let option = match kind {
        LimitKind::Samples => Some("`DecodeOptions::sample_limit`"),
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
    /// applies it to the option `kind` names.
    pub fn retry_with(&self) -> Option<WorkLimit> {
        self.retry_with.clone()
    }

    /// The diagnostic, without the `limit: ` prefix that the [`Error`]'s
    /// `Display` adds. Do not parse it: [`kind`](Self::kind) and
    /// [`retry_with`](Self::retry_with) hold its data.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LimitDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Which limit an [`Error::Limit`] names.
///
/// The first three are [`DecodeOptions`](crate::DecodeOptions) limits that
/// a [`WorkLimit`] raises. No option raises the others, so they refuse the
/// input whatever the options.
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
    /// An image of more samples than the target can address, refused under
    /// any sample limit.
    AddressSpace,
    /// A tile of more than 2^20 code-blocks.
    TileCodeBlocks,
    /// A tile of more than 2^18 precincts.
    TilePrecincts,
    /// A tile's progression order change (`POC`) volumes past a fixed cap:
    /// more than 32 of them, or more packets than the decoder tracks for
    /// them.
    PocVolumes,
    /// A code-block of more than 30 bit-planes.
    BitPlanes,
}

impl LimitKind {
    /// Whether a [`DecodeOptions`](crate::DecodeOptions) limit raises this
    /// one: true for [`Samples`](Self::Samples),
    /// [`PacketWork`](Self::PacketWork) and
    /// [`CodeBlockWork`](Self::CodeBlockWork).
    pub fn is_adjustable(self) -> bool {
        matches!(
            self,
            LimitKind::Samples | LimitKind::PacketWork | LimitKind::CodeBlockWork
        )
    }
}

/// `result` with its error as its `Debug` text, so that two results compare
/// by variant, data and message in a test: [`Error`] has no `PartialEq`.
#[cfg(test)]
pub(crate) fn keyed<T>(result: Result<T>) -> core::result::Result<T, String> {
    result.map_err(|error| format!("{error:?}"))
}

/// Whether `error` is an [`Error::Limit`] for `kind` that no option raises,
/// so that it gives no limit to retry with.
#[cfg(test)]
pub(crate) fn is_fixed_limit(error: &Error, kind: LimitKind) -> bool {
    matches!(error, Error::Limit(limit) if limit.kind() == kind && limit.retry_with().is_none())
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
            "Limit(LimitDetail { kind: BitPlanes, retry_with: None, message: \"31 bit-planes\" })"
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
    fn the_three_options_are_the_adjustable_kinds() {
        use LimitKind::*;
        for kind in [Samples, PacketWork, CodeBlockWork] {
            assert!(kind.is_adjustable(), "{kind:?}");
        }
        for kind in [
            AddressSpace,
            TileCodeBlocks,
            TilePrecincts,
            PocVolumes,
            BitPlanes,
        ] {
            assert!(!kind.is_adjustable(), "{kind:?}");
        }
    }
}
