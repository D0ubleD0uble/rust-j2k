//! Error type for the decoder. One flat enum; each failure maps to the variant
//! that names its class, so a caller can tell a malformed input from an
//! unsupported feature: [`Error::Codestream`] for structural damage,
//! [`Error::Marker`] for an invalid field value, [`Error::FileFormat`] for a
//! malformed JP2 box structure, [`Error::Unsupported`] for
//! a valid-but-out-of-subset feature, [`Error::Limit`] for an input past a
//! decoder resource guard, [`Error::InvalidOptions`] for bad caller options,
//! and [`Error::Inconsistent`] for a violated internal invariant. Structural
//! corruption anywhere in the pipeline — including packet headers and coded
//! data — reports as [`Error::Codestream`].

use core::fmt;

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Error>;

/// A decode failure, tagged by the class of fault.
///
/// Match on the variant to decide what to do; the `String` inside is a
/// diagnostic for a log. Its wording can change between releases, so do not
/// parse it or compare it. Input that is not JPEG 2000 at all, empty input
/// and plain text included, is [`Error::Codestream`].
///
/// The variant also says what to tell a user:
///
/// | Variant | A message for a user |
/// | ------- | -------------------- |
/// | `Codestream`, `Marker`, `FileFormat` | The file is damaged, truncated or not a JPEG 2000 image. |
/// | `Unsupported` | The file uses a JPEG 2000 feature this decoder does not read. |
/// | `Limit` | The file is larger or costlier than the decoder allows by default. If the message names a `DecodeOptions` limit, it can be retried with a higher [`WorkLimit`](crate::WorkLimit). |
/// | `InvalidOptions` | The decode options are invalid, for any file (`WorkLimit::Raised(0)`, sRGB output with `raw_channels`) or for this one (a reduction past the file's levels). |
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
///         Error::Unsupported(_) => "the file uses a feature this decoder does not read",
///         Error::Limit(_) => "the file is larger or costlier than the decoder allows",
///         Error::InvalidOptions(_) => "the decode options do not fit this file",
///         // The enum is `#[non_exhaustive]`: a later version can add variants.
///         _ => "the file could not be decoded",
///     }
/// }
///
/// let error = decode(b"").unwrap_err();
/// assert!(matches!(error, Error::Codestream(_)));
/// assert_eq!(
///     explain(&error),
///     "the file is damaged, truncated or not a JPEG 2000 image"
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Not a JPEG 2000 codestream, or its structure is broken: a truncated or
    /// missing segment, lost sync, or corrupt packet / coded data. Any input
    /// that is not JPEG 2000 at all (empty input, text, another image
    /// format) is this variant, since it has no `SOC` marker.
    Codestream(String),
    /// A marker segment has an invalid field: a reserved value, a bad length
    /// or an out-of-range value.
    Marker(String),
    /// The JP2 box structure or a box field is malformed: a truncated box, a
    /// missing or misplaced required box, an invalid field value, or a box
    /// that disagrees with the codestream.
    FileFormat(String),
    /// A valid-but-not-decoded feature: an HTJ2K codestream, a Part 2
    /// extension, a JP2 box this decoder cannot apply, a file that is not
    /// JP2-compatible. A newer version of the decoder may accept the same
    /// input.
    Unsupported(String),
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
    /// The message of one of them names the option and a `WorkLimit::Raised`
    /// factor; the factor is a hint (see [`WorkLimit`](crate::WorkLimit)), not
    /// a measure of the file.
    ///
    /// The other guards have no option, so a file past one of them is refused
    /// whatever the options. A tile may hold at most 2^19 code-blocks and
    /// 2^18 precincts, and 32 progression order change (POC) volumes; a
    /// code-block may have at most 30 bit-planes; and under a `sample_limit`
    /// of `WorkLimit::Unlimited`, an image too large for the target to
    /// address is still refused. [`WorkLimit`](crate::WorkLimit) has an
    /// example that retries.
    Limit(String),
    /// The caller passed [`DecodeOptions`](crate::DecodeOptions) that are
    /// invalid for this codestream, or that are invalid whatever the input
    /// (sRGB output with `raw_channels`, or a limit of `WorkLimit::Raised(0)`).
    /// The input itself may be fine.
    InvalidOptions(String),
    /// A decoder invariant was violated (declared geometry and decoded state
    /// disagreed where they never should); please file a bug.
    Inconsistent(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Codestream(m) => write!(f, "codestream: {m}"),
            Error::Marker(m) => write!(f, "marker: {m}"),
            Error::FileFormat(m) => write!(f, "file format: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Limit(m) => write!(f, "limit: {m}"),
            Error::InvalidOptions(m) => write!(f, "invalid options: {m}"),
            Error::Inconsistent(m) => write!(f, "decoder bug (please report it): {m}"),
        }
    }
}

impl std::error::Error for Error {}
