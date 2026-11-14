//! sRGB output ([`ColorOutput::Srgb`](crate::ColorOutput::Srgb)): sYCC to
//! RGB, and a restricted ICC profile (I.5.3.3, `METH` 2) applied to give sRGB.
//!
//! The step runs after the palette and the channel definitions, on the color
//! channels: the channels labeled color 1, 2, 3 (one for grayscale). Other
//! channels, such as opacity, pass through untouched. OpenJPEG picks the
//! first channels by position instead; the labels follow the standard
//! (`docs/srgb-output.md`). [`plan`] checks
//! everything from the header before any tile is decoded; [`SrgbPlan::apply`]
//! converts the decoded samples. No conversion changes a channel's depth.
//!
//! - **sRGB and grayscale** (`EnumCS` 16, 17) pass through, except on a
//!   three-channel 4:2:0 or 4:2:2 image ([`converted_as_sycc`]).
//! - **sYCC** (`EnumCS` 18) is converted with the IEC 61966-2-1 Amd 1 inverse
//!   matrix (1.402, 0.344136, 0.714136, 1.772) after taking `2^(depth-1)` off
//!   the chroma, rounded to the nearest integer and clamped to the depth.
//!   OpenJPEG uses 0.344 and 0.714 and truncates each product. The ISO
//!   reference images match this formula much more closely, so it is a
//!   documented deviation (`docs/srgb-output.md`). Chroma sub-sampled 2x1
//!   (4:2:2) or 2x2 (4:2:0) is replicated to the luma grid first. The
//!   replication follows `sycc422_to_rgb` and `sycc420_to_rgb` in the
//!   oracle build, Ubuntu's OpenJPEG 2.5.0-2ubuntu0.5, sample for sample,
//!   including the odd image origin: there the first column (and, for 4:2:0,
//!   the first row) takes chroma samples of 0 rather than a neighbor's. That
//!   build carries the `comp12w` guard from upstream master, which tag v2.5.0
//!   does not have.
//! - **Restricted ICC** profiles of the two forms Annex I allows are applied
//!   as OpenJPEG applies them, through a port of LittleCMS 2.14
//!   ([`super::lcms`]): a monochrome profile (`kTRC`) and a three-component
//!   matrix/TRC profile (`rXYZ`, `gXYZ`, `bXYZ`, `rTRC`, `gTRC`, `bTRC`) with
//!   the XYZ connection space, of the input, display, output or color space
//!   class (or class 0), at every rendering intent LittleCMS accepts. On
//!   three color channels (RGB profile), on one gray channel, or on one
//!   gray channel plus opacity, the output matches
//!   `opj_decompress -force-rgb` sample for sample. A monochrome profile
//!   gives one gray channel when its output is neutral, as it usually is; if
//!   its sRGB output is not neutral, it gives R, G and B in place of the gray
//!   channel. A monochrome profile on more than two channels is refused:
//!   OpenJPEG passes the planes to a gray transform as if they were RGB.
//!
//! Some inputs pass through as coded, as OpenJPEG leaves them: one or two
//! channels in any color space other than a monochrome profile, a profile
//! LittleCMS refuses to open or to build a transform from, a profile in a
//! color space other than RGB, gray or YCbCr, and an RGB profile whose first
//! four channels differ in depth, sign or sub-sampling ([`first_four_differ`]).
//! These come first, before any check on the samples, as in OpenJPEG. Three
//! channels that are 4:2:0 or 4:2:2 are refused whatever the color space,
//! since `opj_decompress` converts them as sYCC before it reads any profile
//! ([`converted_as_sycc`]). The profile reader follows LittleCMS's,
//! including where it reads past a tag's size.
//!
//! Everything else is [`Error::Unsupported`]. The table in
//! `docs/srgb-output.md` ("Every input and its result") gives every case, what
//! OpenJPEG does with it, and the rule behind the choice.

use super::lcms::{Format, InputProfile, ToneCurve, Transform};
use super::metadata::{Association, ChannelDefinition, ChannelType, ColorSpace, Jp2Metadata};
use crate::codestream::markers::Siz;
use crate::samples;
use crate::{Component, ComponentInfo, Error, Result};

/// The ICC header and the tag count that follows it.
const PROFILE_HEADER: usize = 132;

/// What the sRGB step does to a file's channels, worked out before any tile
/// is decoded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SrgbPlan {
    /// The color channels already are sRGB or gray.
    PassThrough,
    /// sYCC to RGB on the channels at these output positions (Y, Cb, Cr).
    Sycc {
        channels: [usize; 3],
        layout: ChromaLayout,
        /// The image origin's parity on the reference grid (`XOsiz & 1`,
        /// `YOsiz & 1`), which OpenJPEG's replication keys on.
        odd_x: bool,
        odd_y: bool,
    },
    /// A matrix/TRC profile applied to the R, G, B channels at these positions.
    IccRgb {
        channels: [usize; 3],
        transform: Box<Transform>,
    },
    /// A monochrome profile applied to the gray channel at this position,
    /// through the RGB each 8-bit gray level maps to. When every level maps
    /// to a gray (`neutral`), the channel stays one gray channel; otherwise
    /// it becomes R, G, B in its place.
    IccGray {
        channel: usize,
        table: Box<[[u8; 3]; 256]>,
        neutral: bool,
    },
}

/// How sYCC chroma is sampled relative to luma (always at full resolution).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChromaLayout {
    /// 4:4:4: chroma at full resolution.
    Full,
    /// 4:2:2: chroma sub-sampled 2x1.
    Horizontal,
    /// 4:2:0: chroma sub-sampled 2x2.
    Both,
}

/// Plan the sRGB step for a JP2 file's output channels: their formats
/// (`outputs`), what each one is (`channels`), and the color space the file
/// declares. `siz` and `reduction` size the output, which is held to `cap`
/// samples (see [`crate::samples`]).
pub(crate) fn plan(
    color: &ColorSpace,
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    siz: &Siz,
    reduction: u8,
    cap: u64,
) -> Result<SrgbPlan> {
    match color {
        // OpenJPEG reads any image of two or fewer components as gray (and
        // gray plus opacity) and passes it through, whatever the file
        // declares, so this does too.
        ColorSpace::Srgb | ColorSpace::Grayscale if outputs.len() <= 2 => Ok(SrgbPlan::PassThrough),
        ColorSpace::Srgb | ColorSpace::Grayscale => {
            // `opj_decompress` converts a three-component 4:2:0 or 4:2:2
            // image as sYCC whatever the file declares and whatever its
            // `cdef` labels, so a file declared sRGB or gray with that
            // layout is refused. Otherwise any channel past the color
            // space's colors is left as coded.
            if converted_as_sycc(outputs) {
                return Err(converted_as_sycc_error());
            }
            uniform(&color_channels(channels, outputs, 0), outputs)?;
            Ok(SrgbPlan::PassThrough)
        }
        // OpenJPEG converts sYCC and an RGB profile only on three or more
        // components; one or two pass through as declared.
        ColorSpace::Sycc if outputs.len() <= 2 => Ok(SrgbPlan::PassThrough),
        ColorSpace::Sycc => plan_sycc(channels, outputs, siz, reduction, cap),
        ColorSpace::RestrictedIcc(profile) => {
            plan_icc(profile, channels, outputs, siz, reduction, cap)
        }
        // One or two channels with no color space this step converts:
        // OpenJPEG reads any image of two or fewer components as gray (and
        // gray plus opacity) and passes it through, so this does too.
        ColorSpace::Enumerated(_) | ColorSpace::Unknown if outputs.len() <= 2 => {
            Ok(SrgbPlan::PassThrough)
        }
        ColorSpace::Enumerated(value) => Err(Error::Unsupported(format!(
            "sRGB output from the enumerated color space {value}; only sRGB, grayscale, \
             sYCC and restricted ICC profiles are converted"
        ))),
        ColorSpace::Unknown => Err(Error::Unsupported(
            "sRGB output from a file with no color specification".into(),
        )),
    }
}

/// Whether `opj_decompress` converts the image as sYCC whatever the file
/// declares, before it looks at any ICC profile. It guesses sYCC for three
/// components whose second is sub-sampled in x and whose first has equal x
/// and y sampling (`opj_decompress.c:1620-1623`), but `color_sycc_to_rgb`
/// (`color.c:403-436`) converts only luma at 1x1 with both chroma channels
/// at 2x2 (4:2:0) or 2x1 (4:2:2). Any other layout prints "CAN NOT
/// CONVERT" and stays as coded.
fn converted_as_sycc(outputs: &[ComponentInfo]) -> bool {
    let [y, cb, cr] = outputs else {
        return false;
    };
    let sampling = |c: &ComponentInfo| (c.x_sampling, c.y_sampling);
    sampling(y) == (1, 1) && sampling(cb) == sampling(cr) && matches!(sampling(cb), (2, 1) | (2, 2))
}

/// The refusal of a file [`converted_as_sycc`] holds for.
fn converted_as_sycc_error() -> Error {
    Error::Unsupported(
        "sRGB output from a 4:2:0 or 4:2:2 file, which OpenJPEG converts as sYCC".into(),
    )
}

/// OpenJPEG's "AFL test" (`color.c:495-519`): before it builds a transform
/// for an RGB profile, it compares the first `min(numcomps, 4)` components
/// with the first, by sub-sampling, depth and sign, and skips the
/// conversion if any differs. The positions are the output's, after the
/// palette and the channel definitions, whatever each channel is.
fn first_four_differ(outputs: &[ComponentInfo]) -> bool {
    let Some((first, rest)) = outputs.split_first() else {
        return false;
    };
    let format = |c: &ComponentInfo| (c.x_sampling, c.y_sampling, c.bit_depth, c.signed);
    rest.iter().take(3).any(|c| format(c) != format(first))
}

/// The output positions of the channels labeled color 1, 2, ... in color
/// order, up to the first color number with no channel. `need` colors must
/// be present; 0 takes whatever is there.
fn color_channels(
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    need: u16,
) -> Vec<usize> {
    let mut found = Vec::new();
    for color in 1..=u16::MAX {
        let at = channels.iter().position(|c| {
            c.kind == ChannelType::Color && c.association == Association::Color(color)
        });
        match at {
            Some(at) if at < outputs.len() => found.push(at),
            _ => break,
        }
        if need != 0 && color == need {
            break;
        }
    }
    found
}

/// The color channels must share depth, sign and sub-sampling.
fn uniform(colors: &[usize], outputs: &[ComponentInfo]) -> Result<()> {
    same_format(colors, outputs)?;
    let Some(&first) = colors.first() else {
        return Ok(());
    };
    let first = &outputs[first];
    for &at in &colors[1..] {
        let other = &outputs[at];
        if (other.x_sampling, other.y_sampling) != (first.x_sampling, first.y_sampling) {
            return Err(Error::Unsupported(format!(
                "sRGB output from color channels of different sub-sampling ({}x{} and {}x{})",
                first.x_sampling, first.y_sampling, other.x_sampling, other.y_sampling
            )));
        }
    }
    Ok(())
}

/// The color channels must share depth and sign.
fn same_format(colors: &[usize], outputs: &[ComponentInfo]) -> Result<()> {
    let Some(&first) = colors.first() else {
        return Ok(());
    };
    let first = &outputs[first];
    for &at in &colors[1..] {
        let other = &outputs[at];
        if (other.bit_depth, other.signed) != (first.bit_depth, first.signed) {
            return Err(Error::Unsupported(format!(
                "sRGB output from color channels of different formats ({}-bit {} and \
                 {}-bit {})",
                first.bit_depth,
                sign(first.signed),
                other.bit_depth,
                sign(other.signed)
            )));
        }
    }
    Ok(())
}

fn sign(signed: bool) -> &'static str {
    if signed { "signed" } else { "unsigned" }
}

/// Exactly three color channels for sYCC or an RGB profile.
fn three_colors(
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    what: &str,
) -> Result<[usize; 3]> {
    let found = color_channels(channels, outputs, 3);
    <[usize; 3]>::try_from(found.as_slice()).map_err(|_| {
        Error::Unsupported(format!(
            "sRGB output from {what} needs color channels 1, 2 and 3; the file has {}",
            found.len()
        ))
    })
}

fn plan_sycc(
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    siz: &Siz,
    reduction: u8,
    cap: u64,
) -> Result<SrgbPlan> {
    let colors = three_colors(channels, outputs, "sYCC")?;
    same_format(&colors, outputs)?;
    let [y, cb, cr] = colors.map(|at| &outputs[at]);
    if y.signed {
        return Err(Error::Unsupported(
            "sRGB output from signed sYCC samples".into(),
        ));
    }
    let sampling = |c: &ComponentInfo| (c.x_sampling, c.y_sampling);
    let layout = match (sampling(y), sampling(cb), sampling(cr)) {
        ((1, 1), (1, 1), (1, 1)) => ChromaLayout::Full,
        ((1, 1), (2, 1), (2, 1)) => ChromaLayout::Horizontal,
        ((1, 1), (2, 2), (2, 2)) => ChromaLayout::Both,
        (y, cb, cr) => {
            return Err(Error::Unsupported(format!(
                "sRGB output from sYCC sampled {}x{}, {}x{}, {}x{}; only luma at full \
                 resolution with chroma at 1x1, 2x1 or 2x2 is converted",
                y.0, y.1, cb.0, cb.1, cr.0, cr.1
            )));
        }
    };
    check_budget(outputs, &colors, siz, reduction, cap)?;
    Ok(SrgbPlan::Sycc {
        channels: colors,
        layout,
        odd_x: siz.x_offset & 1 == 1,
        odd_y: siz.y_offset & 1 == 1,
    })
}

/// The output after chroma replication must stay under the sample limit: two
/// chroma channels grow to the luma grid. Every other channel keeps its size.
fn check_budget(
    outputs: &[ComponentInfo],
    colors: &[usize; 3],
    siz: &Siz,
    reduction: u8,
    cap: u64,
) -> Result<()> {
    let luma = siz.image_extent_at(reduction);
    let mut total: u64 = 0;
    for (at, info) in outputs.iter().enumerate() {
        let (width, height) = if colors.contains(&at) {
            luma
        } else {
            siz.sampled_extent_at(info.x_sampling, info.y_sampling, reduction)
                .ok_or_else(|| Error::Inconsistent("a channel has a zero sub-sampling".into()))?
        };
        total = total.saturating_add(u64::from(width) * u64::from(height));
    }
    if total > cap {
        let what = format!(
            "sRGB output with the sYCC chroma replicated{}",
            samples::at(reduction)
        );
        return Err(samples::over(&what, total, cap));
    }
    Ok(())
}

fn plan_icc(
    profile: &[u8],
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    siz: &Siz,
    reduction: u8,
    cap: u64,
) -> Result<SrgbPlan> {
    // OpenJPEG converts a 4:2:0 or 4:2:2 image as sYCC before it opens the
    // profile, so this holds whatever the profile is, even one LittleCMS
    // refuses. That guess is not made here, so the file is refused.
    if converted_as_sycc(outputs) {
        // `uniform` always fails here, since the chroma sampling differs
        // from the luma's; the error below is a backstop that cannot run.
        uniform(&[0, 1, 2], outputs)?;
        return Err(converted_as_sycc_error());
    }
    match plan_icc_or_refuse(profile, channels, outputs, siz, reduction, cap) {
        Ok(plan) => Ok(plan),
        // OpenJPEG skips the conversion when LittleCMS refuses the profile
        // and writes the samples as coded, and so does this step. The
        // metadata keeps the profile, so a caller can tell.
        Err(Icc::Refused) => Ok(SrgbPlan::PassThrough),
        Err(Icc::Error(e)) => Err(e),
    }
}

/// [`plan_icc`], with a profile OpenJPEG does not convert as
/// [`Icc::Refused`]. The order follows OpenJPEG's: LittleCMS opens the
/// profile, OpenJPEG picks the transform by color space, and LittleCMS
/// builds it. Every refusal of those three steps comes before this step's
/// own checks on the profile and the channels, so a profile OpenJPEG leaves
/// as coded passes through whatever the channels are.
fn plan_icc_or_refuse(
    profile: &[u8],
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    siz: &Siz,
    reduction: u8,
    cap: u64,
) -> std::result::Result<SrgbPlan, Icc> {
    let icc = Profile::parse(profile)?;
    match &icc.space {
        b"GRAY" => {
            let input = icc.input_profile(Form::Gray)?;
            let colors = color_channels(channels, outputs, 0);
            let [channel] = colors[..] else {
                return Err(Error::Unsupported(format!(
                    "sRGB output from a monochrome ICC profile needs one color channel; \
                     the file has {}",
                    colors.len()
                ))
                .into());
            };
            // OpenJPEG feeds three or more channels to a gray transform as if
            // they were RGB, so there is no real conversion to match.
            if outputs.len() > 2 {
                return Err(Error::Unsupported(format!(
                    "sRGB output from a monochrome ICC profile on {} channels; only one \
                     channel, or one plus opacity, is converted",
                    outputs.len()
                ))
                .into());
            }
            check_icc_format(&outputs[channel], &[8], "monochrome")?;
            let transform = Transform::new(&input, Format::Gray8);
            let mut table = Box::new([[0u8; 3]; 256]);
            for (gray, rgb) in (0..=255u8).zip(table.iter_mut()) {
                *rgb = transform.gray8(gray);
            }
            let neutral = table.iter().all(|&[r, g, b]| r == g && g == b);
            if !neutral {
                check_gray_budget(outputs, channel, siz, reduction, cap)?;
            }
            Ok(SrgbPlan::IccGray {
                channel,
                table,
                neutral,
            })
        }
        // OpenJPEG applies an RGB profile only to three or more components.
        b"RGB " if outputs.len() <= 2 => Ok(SrgbPlan::PassThrough),
        b"RGB " => {
            // OpenJPEG's AFL test sits after LittleCMS opens the profile and
            // before it builds the transform, so it comes before this step's
            // own checks too. A 4:2:0 or 4:2:2 image never gets here:
            // `plan_icc` refused it first.
            if first_four_differ(outputs) {
                return Err(Icc::Refused);
            }
            let input = icc.input_profile(Form::Rgb)?;
            let colors = three_colors(channels, outputs, "an RGB ICC profile")?;
            uniform(&colors, outputs)?;
            let info = &outputs[colors[0]];
            check_icc_format(info, &[8, 16], "RGB")?;
            let format = if info.bit_depth == 8 {
                Format::Rgb8
            } else {
                Format::Rgb16
            };
            Ok(SrgbPlan::IccRgb {
                channels: colors,
                transform: Box::new(Transform::new(&input, format)),
            })
        }
        // OpenJPEG applies a YCbCr profile only to three or more components,
        // and LittleCMS refuses first, as for RGB.
        b"YCbr" if outputs.len() <= 2 => Ok(SrgbPlan::PassThrough),
        b"YCbr" => {
            icc.input_profile(Form::Rgb)?;
            // OpenJPEG runs the transform as 16-bit YCbCr whatever the
            // depth: it converts 16-bit samples and crashes on 8-bit ones.
            // YCbCr is not one of Annex I's restricted forms, so this step
            // does not apply it at any depth.
            Err(unsupported_profile("for the `YCbr` color space"))
        }
        // OpenJPEG converts no other color space.
        _ => Err(Icc::Refused),
    }
}

/// A gray channel that becomes R, G, B grows the output by two channels of
/// its size, which must stay under the sample limit.
fn check_gray_budget(
    outputs: &[ComponentInfo],
    channel: usize,
    siz: &Siz,
    reduction: u8,
    cap: u64,
) -> Result<()> {
    let mut total: u64 = 0;
    for (at, info) in outputs.iter().enumerate() {
        let (width, height) = siz
            .sampled_extent_at(info.x_sampling, info.y_sampling, reduction)
            .ok_or_else(|| Error::Inconsistent("a channel has a zero sub-sampling".into()))?;
        let copies = if at == channel { 3 } else { 1 };
        total = total.saturating_add(copies * u64::from(width) * u64::from(height));
    }
    if total > cap {
        let what = format!(
            "sRGB output from a monochrome ICC profile that maps gray to color{}",
            samples::at(reduction)
        );
        return Err(samples::over(&what, total, cap));
    }
    Ok(())
}

/// `_validatedVersion` (cmsio0.c): the header version with each BCD digit
/// capped at 9 and the reserved bytes cleared, as a big-endian word.
fn validated_version([major, minor, _, _]: [u8; 4]) -> u32 {
    let minor = (minor & 0xf0).min(0x90) | (minor & 0x0f).min(0x09);
    u32::from_be_bytes([major.min(9), minor, 0, 0])
}

/// The depths a profile is applied at: those where OpenJPEG + lcms read the
/// samples at their own depth (8-bit and 16-bit RGB, 8-bit gray). At other
/// depths OpenJPEG hands lcms the samples as 8- or 16-bit values and the
/// result is wrong, so this decoder refuses them.
fn check_icc_format(info: &ComponentInfo, depths: &[u8], form: &str) -> Result<()> {
    if info.signed || !depths.contains(&info.bit_depth) {
        return Err(Error::Unsupported(format!(
            "sRGB output through a {form} ICC profile from {}-bit {} samples",
            info.bit_depth,
            sign(info.signed)
        )));
    }
    Ok(())
}

/// LittleCMS's limit on the tag count (`MAX_TABLE_TAG`).
const MAX_TAGS: u32 = 100;

/// The longest `curv` table LittleCMS reads (`Type_Curve_Read`).
const MAX_CURVE_ENTRIES: u32 = 0x7fff;

/// Tag types LittleCMS reads as another type: `XYZ ` written by an old Corbis
/// tool (`cmsCorbisBrokenXYZtype`), and `curv` written by an old Monaco tool
/// (`cmsMonacoBrokenCurveType`, allowed for `rTRC`, `gTRC` and `bTRC` only).
const CORBIS_XYZ: [u8; 4] = 0x17A5_05B8u32.to_be_bytes();
const MONACO_CURVE: [u8; 4] = 0x9478_EE00u32.to_be_bytes();

/// Why a profile is not applied.
#[derive(Debug)]
enum Icc {
    /// OpenJPEG writes the samples as coded: LittleCMS refuses to open the
    /// profile or to build a transform from it, OpenJPEG has no transform
    /// for its color space, or its AFL test fails ([`first_four_differ`]).
    Refused,
    /// This step refuses the input.
    Error(Error),
}

impl From<Error> for Icc {
    fn from(e: Error) -> Self {
        Icc::Error(e)
    }
}

/// A profile of a form this step does not apply.
fn unsupported_profile(what: &str) -> Icc {
    Icc::Error(Error::Unsupported(format!(
        "sRGB output through an ICC profile {what}; only the monochrome and \
         three-component matrix/TRC input profiles of Annex I are applied"
    )))
}

/// The color space of a profile this step applies.
#[derive(Clone, Copy, PartialEq)]
enum Form {
    Gray,
    Rgb,
}

/// The tags LittleCMS reads a lookup table from, by rendering intent 0 to 3
/// (`Device2PCS16`, `Device2PCSFloat` in cmsio1.c). Absolute colorimetric
/// reads the relative colorimetric `A2B1`.
const A2B: [&[u8; 4]; 4] = [b"A2B0", b"A2B1", b"A2B2", b"A2B1"];
const D2B: [&[u8; 4]; 4] = [b"D2B0", b"D2B1", b"D2B2", b"D2B3"];

/// A restricted ICC profile (ICC.1, as Annex I restricts it): the header
/// fields that matter and the tag table.
struct Profile<'a> {
    bytes: &'a [u8],
    class: [u8; 4],
    space: [u8; 4],
    pcs: [u8; 4],
    /// The header version, as `_validatedVersion` reads it.
    version: u32,
    /// The header's rendering intent, as stored.
    intent: u32,
    /// The entries LittleCMS keeps: signature, offset and size. At most
    /// [`MAX_TAGS`].
    tags: Vec<([u8; 4], u32, u32)>,
}

impl<'a> Profile<'a> {
    /// The checks LittleCMS makes when it opens a profile
    /// (`_cmsReadHeader`, cmsio0.c): the 128-byte header and the tag count,
    /// the `acsp` signature, a device class it knows, a version of at most
    /// 5.0, at most 100 tags, the table inside the profile, and no two kept
    /// entries with one signature.
    fn parse(bytes: &'a [u8]) -> std::result::Result<Self, Icc> {
        if bytes.len() < PROFILE_HEADER {
            return Err(Icc::Refused);
        }
        let four = |at: usize| -> [u8; 4] {
            let mut out = [0; 4];
            out.copy_from_slice(&bytes[at..at + 4]);
            out
        };
        let class = four(12);
        // `validDeviceClass` takes a zero class too, as older LittleCMS
        // versions wrote it.
        let known = matches!(
            &class,
            b"scnr" | b"mntr" | b"prtr" | b"link" | b"abst" | b"spac" | b"nmcl" | [0, 0, 0, 0]
        );
        let version = validated_version(four(8));
        if &four(36) != b"acsp" || !known || version > 0x0500_0000 {
            return Err(Icc::Refused);
        }
        let count = u32::from_be_bytes(four(128));
        if count > MAX_TAGS {
            return Err(Icc::Refused);
        }
        let table = usize::try_from(count)
            .ok()
            .and_then(|n| n.checked_mul(12))
            .and_then(|len| bytes.get(PROFILE_HEADER..PROFILE_HEADER.checked_add(len)?))
            .ok_or(Icc::Refused)?;
        // LittleCMS skips an entry with a zero offset or size, or one that
        // ends past the declared size (capped at the profile's length). Only
        // the entries it keeps count as duplicates.
        let declared = u32::from_be_bytes(four(0));
        let declared = u32::try_from(bytes.len()).map_or(declared, |len| declared.min(len));
        let mut tags = Vec::new();
        for e in table.chunks_exact(12) {
            let word = |at: usize| u32::from_be_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
            let (offset, size) = (word(4), word(8));
            if offset == 0 || size == 0 || offset.checked_add(size).is_none_or(|end| end > declared)
            {
                continue;
            }
            let signature = [e[0], e[1], e[2], e[3]];
            if tags.iter().any(|&(s, _, _)| s == signature) {
                return Err(Icc::Refused);
            }
            tags.push((signature, offset, size));
        }
        Ok(Profile {
            bytes,
            class,
            space: four(16),
            pcs: four(20),
            version,
            intent: u32::from_be_bytes(four(64)),
            tags,
        })
    }

    /// What `cmsCreateTransform` reads to build OpenJPEG's transform from
    /// this profile to sRGB, as the input of this step's port. Every way
    /// LittleCMS refuses comes first, as [`Icc::Refused`]; then the forms the
    /// port does not apply, as [`Error::Unsupported`].
    fn input_profile(&self, form: Form) -> std::result::Result<InputProfile, Icc> {
        // A named color profile takes one index channel, not RGB or gray
        // samples (`GetXFormColorSpaces`, `IsProperColorSpace`).
        if &self.class == b"nmcl" {
            return Err(Icc::Refused);
        }
        let (intent, table_intent) = self.intent()?;
        // The connection space must lead on to the sRGB profile's
        // (`ColorSpaceIsCompatible`).
        if &self.pcs != b"XYZ " && &self.pcs != b"Lab " {
            return Err(Icc::Refused);
        }
        // A device link or abstract profile is only a lookup table; any
        // other class reads one when it has one, and is a matrix/TRC
        // profile otherwise (`DefaultICCintents`, `_cmsReadInputLUT`,
        // `_cmsReadDevicelinkLUT`).
        let link = &self.class == b"link" || &self.class == b"abst";
        if let Some(lut) = self.lut_tag(table_intent, link) {
            self.check_lut(lut, form)?;
            return Err(unsupported_profile(&format!(
                "with a `{}` lookup table",
                String::from_utf8_lossy(lut)
            )));
        }
        if link {
            return Err(Icc::Refused);
        }
        let (curves, colorants) = match form {
            Form::Gray => (vec![self.curve(b"kTRC")?], [[0.0; 3]; 3]),
            Form::Rgb => {
                let columns = [self.xyz(b"rXYZ")?, self.xyz(b"gXYZ")?, self.xyz(b"bXYZ")?];
                let curves = vec![
                    self.curve(b"rTRC")?,
                    self.curve(b"gTRC")?,
                    self.curve(b"bTRC")?,
                ];
                (
                    curves,
                    [0, 1, 2].map(|row| columns.map(|column| column[row])),
                )
            }
        };
        if &self.pcs != b"XYZ " {
            return Err(unsupported_profile("with the `Lab ` connection space"));
        }
        Ok(InputProfile {
            intent,
            white: self.media_white(intent),
            curves,
            colorants,
        })
    }

    /// The rendering intent the transform runs at, and the intent whose
    /// lookup table LittleCMS reads. LittleCMS builds no transform for an
    /// intent it does not know. It reads its black-preserving intents (10 to
    /// 15) on a profile that is not CMYK as perceptual, relative colorimetric
    /// or saturation (`TranslateNonICCIntents`), without the black point
    /// compensation it forces for perceptual and saturation, so a matrix/TRC
    /// profile converts as relative colorimetric (1) in every case.
    fn intent(&self) -> std::result::Result<(u32, usize), Icc> {
        match self.intent {
            0..=3 => Ok((self.intent, self.intent as usize)),
            10..=15 => Ok((1, (self.intent as usize - 10) % 3)),
            _ => Err(Icc::Refused),
        }
    }

    /// The lookup table tag LittleCMS reads at `intent` (0 to 3), if any:
    /// `D2Bx` before `A2Bx`, and `A2B0` when the intent's own is missing. A
    /// device link also falls back to `D2B0`.
    fn lut_tag(&self, intent: usize, link: bool) -> Option<&'static [u8; 4]> {
        let has = |signature: &[u8; 4]| self.entry(signature).is_some();
        let mut order = vec![D2B[intent]];
        if link {
            order.push(D2B[0]);
        }
        order.extend([A2B[intent], A2B[0]]);
        order.into_iter().find(|&signature| has(signature))
    }

    /// The checks of `cmsReadTag` and of the lookup table readers
    /// (`Type_LUT8_Read`, `Type_LUT16_Read`, `Type_LUTA2B_Read`,
    /// `Type_MPE_Read`) that make LittleCMS refuse a table, and the channel
    /// counts the transform needs: the profile's color channels in, the
    /// three of the connection space out. A table that passes may still not
    /// read, but this step does not apply one.
    fn check_lut(&self, signature: &[u8; 4], form: Form) -> std::result::Result<(), Icc> {
        let colors = if form == Form::Gray { 1 } else { 3 };
        if signature.starts_with(b"D2B") {
            let (_, body) = self.tag(signature, &[*b"mpet"])?;
            // Input and output channels (two bytes each), then the element
            // count.
            let channels = |at: usize| usize::from(u16::from_be_bytes([body[at], body[at + 1]]));
            if body.len() < 8 || channels(0) != colors || channels(2) != 3 {
                return Err(Icc::Refused);
            }
            return Ok(());
        }
        let (kind, body) = self.tag(signature, &[*b"mft1", *b"mft2", *b"mAB "])?;
        let &[inputs, outputs, grid, ..] = body else {
            return Err(Icc::Refused);
        };
        if usize::from(inputs) != colors || outputs != 3 {
            return Err(Icc::Refused);
        }
        if &kind == b"mAB " {
            // The channels, two padding bytes and five offsets.
            return if body.len() < 24 {
                Err(Icc::Refused)
            } else {
                Ok(())
            };
        }
        if grid == 1 {
            return Err(Icc::Refused);
        }
        // The 3x3 matrix follows the four header bytes.
        let clut = 3 * usize::from(grid).pow(u32::from(inputs));
        let need = if &kind == b"mft1" {
            40 + 256 * colors + clut + 256 * 3
        } else {
            let entries = body.get(40..44).ok_or(Icc::Refused)?;
            let input = usize::from(u16::from_be_bytes([entries[0], entries[1]]));
            let output = usize::from(u16::from_be_bytes([entries[2], entries[3]]));
            if input > 0x7fff || output > 0x7fff || input == 1 || output == 1 {
                return Err(Icc::Refused);
            }
            44 + 2 * (colors * input + clut + 3 * output)
        };
        if body.len() < need {
            return Err(Icc::Refused);
        }
        Ok(())
    }

    /// The tag table entry LittleCMS keeps for `signature`: its offset and
    /// size.
    fn entry(&self, signature: &[u8; 4]) -> Option<(u32, u32)> {
        self.tags
            .iter()
            .find(|(s, _, _)| s == signature)
            .map(|&(_, offset, size)| (offset, size))
    }

    /// Tag `signature` as `cmsReadTag` reads it: its type, which must be one
    /// of `types`, and the bytes after the 8-byte type header up to the end
    /// of the profile. LittleCMS reads a value from there without checking
    /// the tag's size, so a value that runs past its tag takes the bytes
    /// that follow it.
    fn tag(
        &self,
        signature: &[u8; 4],
        types: &[[u8; 4]],
    ) -> std::result::Result<([u8; 4], &'a [u8]), Icc> {
        let (offset, size) = self.entry(signature).ok_or(Icc::Refused)?;
        if size < 8 {
            return Err(Icc::Refused);
        }
        let at = usize::try_from(offset).map_err(|_| Icc::Refused)?;
        let data = self.bytes.get(at..).ok_or(Icc::Refused)?;
        let kind: [u8; 4] = data
            .get(..4)
            .and_then(|k| k.try_into().ok())
            .ok_or(Icc::Refused)?;
        let body = data.get(8..).ok_or(Icc::Refused)?;
        if !types.contains(&kind) {
            return Err(Icc::Refused);
        }
        Ok((kind, body))
    }

    /// An `XYZType` tag's one XYZ value (`Type_XYZ_Read`).
    fn xyz(&self, signature: &[u8; 4]) -> std::result::Result<[f64; 3], Icc> {
        let (_, body) = self.tag(signature, &[*b"XYZ ", CORBIS_XYZ])?;
        let value = body.get(..12).ok_or(Icc::Refused)?;
        Ok([
            s15_16(&value[0..]),
            s15_16(&value[4..]),
            s15_16(&value[8..]),
        ])
    }

    /// The media white point that absolute colorimetric scales by, or `None`
    /// for D50. LittleCMS takes D50 when the `wtpt` tag is missing or does
    /// not read, and for a display profile before version 4
    /// (`_cmsReadMediaWhitePoint`); it reads it at intent 3 only.
    fn media_white(&self, intent: u32) -> Option<[f64; 3]> {
        if intent != 3 || (&self.class == b"mntr" && self.version < 0x0400_0000) {
            return None;
        }
        self.xyz(b"wtpt").ok()
    }

    /// A `curveType` or `parametricCurveType` tag, as LittleCMS reads it
    /// (`Type_Curve_Read`, `Type_ParametricCurve_Read`).
    fn curve(&self, signature: &[u8; 4]) -> std::result::Result<ToneCurve, Icc> {
        let types: &[[u8; 4]] = if signature == b"kTRC" {
            &[*b"curv", *b"para"]
        } else {
            &[*b"curv", *b"para", MONACO_CURVE]
        };
        let (kind, body) = self.tag(signature, types)?;
        let word = |at: usize| -> std::result::Result<u32, Icc> {
            let b = body.get(at..at + 4).ok_or(Icc::Refused)?;
            Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let mut params = [0.0; 10];
        if &kind == b"para" {
            // The function type and a reserved word, then its parameters.
            let (function, used) = match word(0)? >> 16 {
                0 => (1, 1),
                1 => (2, 3),
                2 => (3, 4),
                3 => (4, 5),
                4 => (5, 7),
                _ => return Err(Icc::Refused),
            };
            let raw = body.get(4..4 + 4 * used).ok_or(Icc::Refused)?;
            for (param, bytes) in params.iter_mut().zip(raw.chunks_exact(4)) {
                *param = s15_16(bytes);
            }
            return Ok(ToneCurve::parametric(function, params));
        }
        let count = word(0)?;
        if count > MAX_CURVE_ENTRIES {
            return Err(Icc::Refused);
        }
        let entries = usize::try_from(count)
            .ok()
            .and_then(|n| body.get(4..4 + 2 * n))
            .ok_or(Icc::Refused)?;
        let values: Vec<u16> = entries
            .chunks_exact(2)
            .map(|v| u16::from_be_bytes([v[0], v[1]]))
            .collect();
        Ok(match values[..] {
            [] => {
                params[0] = 1.0;
                ToneCurve::parametric(1, params)
            }
            [gamma] => {
                params[0] = f64::from(gamma) / 256.0;
                ToneCurve::parametric(1, params)
            }
            _ => ToneCurve::tabulated(values),
        })
    }
}

/// An ICC `s15Fixed16Number` from the first four bytes of `bytes`.
fn s15_16(bytes: &[u8]) -> f64 {
    f64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])) / 65536.0
}

impl SrgbPlan {
    /// Open `profile` and read the input profile for both forms, as planning
    /// does, but build no transform. The fuzz target for the ICC reader
    /// calls this for every input; building the transform costs far more.
    pub(crate) fn read_profile(profile: &[u8]) {
        if let Ok(icc) = Profile::parse(profile) {
            for form in [Form::Gray, Form::Rgb] {
                let _ = icc.input_profile(form);
            }
        }
    }

    /// Describe the converted output in `metadata`: sRGB, or gray for a
    /// monochrome profile that keeps gray, with no ICC profile. A gray
    /// channel that becomes R, G, B gains two channel definitions, colors 2
    /// and 3, after it, and a non-color channel associated with the gray
    /// one is now associated with the whole image. Nothing changes when the
    /// samples pass through.
    pub(crate) fn describe(&self, metadata: &mut Jp2Metadata) {
        match self {
            SrgbPlan::PassThrough => {}
            SrgbPlan::Sycc { .. } | SrgbPlan::IccRgb { .. } => metadata.color = ColorSpace::Srgb,
            SrgbPlan::IccGray { neutral: true, .. } => metadata.color = ColorSpace::Grayscale,
            SrgbPlan::IccGray { channel, .. } => {
                metadata.color = ColorSpace::Srgb;
                for definition in &mut metadata.channels {
                    if definition.kind != ChannelType::Color
                        && definition.association == Association::Color(1)
                    {
                        definition.association = Association::WholeImage;
                    }
                }
                let at = (*channel + 1).min(metadata.channels.len());
                for color in [3, 2] {
                    metadata.channels.insert(
                        at,
                        ChannelDefinition {
                            kind: ChannelType::Color,
                            association: Association::Color(color),
                        },
                    );
                }
            }
        }
    }

    /// Describe the converted output channels' formats and sizes, from the
    /// formats before the conversion: sYCC chroma grows to the luma grid, and
    /// a monochrome profile whose output is not neutral gains two channels.
    pub(crate) fn describe_outputs(&self, outputs: &mut Vec<ComponentInfo>) {
        match self {
            SrgbPlan::PassThrough | SrgbPlan::IccRgb { .. } => {}
            SrgbPlan::Sycc { channels, .. } => {
                let luma = outputs[channels[0]].clone();
                for &at in channels {
                    let own = &outputs[at];
                    outputs[at] = ComponentInfo {
                        bit_depth: own.bit_depth,
                        signed: own.signed,
                        ..luma.clone()
                    };
                }
            }
            SrgbPlan::IccGray {
                channel, neutral, ..
            } => {
                if !neutral {
                    let gray = outputs[*channel].clone();
                    outputs.insert(*channel + 1, gray.clone());
                    outputs.insert(*channel + 1, gray);
                }
            }
        }
    }

    /// Convert the decoded output channels.
    pub(crate) fn apply(&self, mut components: Vec<Component>) -> Result<Vec<Component>> {
        match self {
            SrgbPlan::PassThrough => {}
            SrgbPlan::Sycc {
                channels,
                layout,
                odd_x,
                odd_y,
            } => {
                let [y, cb, cr] = channels.map(|at| &components[at]);
                let rgb = sycc_to_rgb(y, cb, cr, *layout, *odd_x, *odd_y)?;
                for (at, out) in channels.iter().zip(rgb) {
                    components[*at] = out;
                }
            }
            SrgbPlan::IccRgb {
                channels,
                transform,
            } => {
                let [r, g, b] = channels.map(|at| &components[at]);
                let rgb = icc_rgb(r, g, b, transform)?;
                for (at, out) in channels.iter().zip(rgb) {
                    components[*at] = out;
                }
            }
            SrgbPlan::IccGray {
                channel,
                table,
                neutral,
            } => {
                let gray = &components[*channel];
                let mut rgb = [gray.clone(), gray.clone(), gray.clone()];
                for (i, &sample) in gray.samples.iter().enumerate() {
                    let out = table[clamp_index(sample, 255)];
                    for (plane, value) in rgb.iter_mut().zip(out) {
                        plane.samples[i] = i32::from(value);
                    }
                }
                let [r, g, b] = rgb;
                components[*channel] = r;
                if !neutral {
                    components.insert(*channel + 1, b);
                    components.insert(*channel + 1, g);
                }
            }
        }
        Ok(components)
    }
}

/// `sample` as an index into a table of `max + 1` entries.
fn clamp_index(sample: i32, max: usize) -> usize {
    usize::try_from(sample.max(0)).map_or(max, |s| s.min(max))
}

/// An RGB profile on 8 or 16-bit samples (the plan checked the depth).
fn icc_rgb(
    r: &Component,
    g: &Component,
    b: &Component,
    transform: &Transform,
) -> Result<[Component; 3]> {
    same_grid(&[r, g, b])?;
    let mut out = [r.clone(), g.clone(), b.clone()];
    let eight = r.bit_depth == 8;
    let max = if eight { 255 } else { 65535 };
    for i in 0..r.samples.len() {
        let [rv, gv, bv] = [r, g, b].map(|c| clamp_index(c.samples[i], max));
        let rgb = if eight {
            transform.rgb8([rv, gv, bv].map(|v| v as u8)).map(i32::from)
        } else {
            transform
                .rgb16([rv, gv, bv].map(|v| v as u16))
                .map(i32::from)
        };
        for (channel, value) in out.iter_mut().zip(rgb) {
            channel.samples[i] = value;
        }
    }
    Ok(out)
}

/// The channels of a full-resolution conversion must share one grid.
fn same_grid(channels: &[&Component]) -> Result<()> {
    let first = channels[0];
    for other in &channels[1..] {
        if (other.width, other.height) != (first.width, first.height)
            || other.samples.len() != first.samples.len()
        {
            return Err(Error::Inconsistent(format!(
                "color channels decoded at {}x{} and {}x{}",
                first.width, first.height, other.width, other.height
            )));
        }
    }
    Ok(())
}

/// The IEC 61966-2-1 Amd 1 inverse on one sample. `cb` and `cr` are the raw
/// chroma samples; `offset` is `2^(depth-1)` and `max` is `2^depth - 1`.
pub(crate) fn sycc_sample(y: i32, cb: i32, cr: i32, offset: i64, max: i64) -> [i32; 3] {
    let y = f64::from(y);
    let cb = (i64::from(cb) - offset) as f64;
    let cr = (i64::from(cr) - offset) as f64;
    let clamp = |v: f64| v.round().clamp(0.0, max as f64) as i32;
    [
        clamp(y + 1.402 * cr),
        clamp(y - 0.344136 * cb - 0.714136 * cr),
        clamp(y + 1.772 * cb),
    ]
}

/// sYCC to RGB on the luma grid. The plan has checked the layout, depth and
/// sign; the grids are checked here against the layout.
fn sycc_to_rgb(
    y: &Component,
    cb: &Component,
    cr: &Component,
    layout: ChromaLayout,
    odd_x: bool,
    odd_y: bool,
) -> Result<[Component; 3]> {
    if (cb.width, cb.height, cb.samples.len()) != (cr.width, cr.height, cr.samples.len()) {
        return Err(Error::Inconsistent(format!(
            "sYCC chroma channels decoded at {}x{} and {}x{}",
            cb.width, cb.height, cr.width, cr.height
        )));
    }
    let depth = u32::from(y.bit_depth);
    let offset = 1i64 << (depth - 1);
    let max = (1i64 << depth) - 1;
    let full = |c: &Component| {
        Component::new(
            y.width,
            y.height,
            c.bit_depth,
            c.signed,
            y.x_sampling,
            y.y_sampling,
            vec![0; y.samples.len()],
        )
    };
    let mut out = [full(y), full(cb), full(cr)];
    let mut put = |at: usize, chroma: Option<usize>| {
        let (b, r) = chroma
            .and_then(|c| Some((*cb.samples.get(c)?, *cr.samples.get(c)?)))
            .unwrap_or((0, 0));
        let Some(&luma) = y.samples.get(at) else {
            return;
        };
        let rgb = sycc_sample(luma, b, r, offset, max);
        for (channel, value) in out.iter_mut().zip(rgb) {
            channel.samples[at] = value;
        }
    };
    replicate(
        y.width as usize,
        y.height as usize,
        cb.width as usize,
        layout,
        odd_x,
        odd_y,
        &mut put,
    );
    Ok(out)
}

/// Walk the luma grid in OpenJPEG's order (`sycc444_to_rgb`,
/// `sycc422_to_rgb`, `sycc420_to_rgb` in the oracle build, Ubuntu's
/// 2.5.0-2ubuntu0.5, which has upstream master's `comp12w` guard), calling `put(luma, chroma)`
/// with the index of each luma sample and of the chroma sample it takes, or
/// `None` where OpenJPEG uses chroma samples of 0. The chroma index is
/// OpenJPEG's running pointer; past the chroma plane it reads as `None` in
/// the caller. `chroma_width` is the chroma plane's width (OpenJPEG's
/// `comp12w`).
///
/// With an odd origin, OpenJPEG gives the first column of each row (4:2:2),
/// or the first row and the first column of the upper row of each pair and
/// of a last unpaired row (4:2:0), chroma of 0. A last odd column whose pair
/// index equals the chroma width also takes 0.
pub(crate) fn replicate(
    width: usize,
    height: usize,
    chroma_width: usize,
    layout: ChromaLayout,
    odd_x: bool,
    odd_y: bool,
    put: &mut impl FnMut(usize, Option<usize>),
) {
    let ox = usize::from(odd_x && width > 0);
    let loop_w = width - ox;
    // One row from `luma`, and the row below it from `below` for a 4:2:0
    // pair: the odd first column, the pairs, then a last odd column.
    let row = |put: &mut dyn FnMut(usize, Option<usize>),
               luma: &mut usize,
               mut below: Option<usize>,
               chroma: &mut usize| {
        if ox == 1 {
            put(*luma, None);
            *luma += 1;
            if let Some(b) = below.as_mut() {
                put(*b, Some(*chroma));
                *b += 1;
            }
        }
        let mut j = 0;
        while j + 1 < loop_w {
            put(*luma, Some(*chroma));
            put(*luma + 1, Some(*chroma));
            *luma += 2;
            if let Some(b) = below.as_mut() {
                put(*b, Some(*chroma));
                put(*b + 1, Some(*chroma));
                *b += 2;
            }
            *chroma += 1;
            j += 2;
        }
        if j < loop_w {
            let c = (j / 2 != chroma_width).then_some(*chroma);
            put(*luma, c);
            *luma += 1;
            if let Some(b) = below {
                put(b, c);
            }
            if j / 2 < chroma_width {
                *chroma += 1;
            }
        }
    };
    let (mut luma, mut chroma) = (0, 0);
    match layout {
        ChromaLayout::Full => {
            for i in 0..width * height {
                put(i, Some(i));
            }
        }
        ChromaLayout::Horizontal => {
            for _ in 0..height {
                row(put, &mut luma, None, &mut chroma);
            }
        }
        ChromaLayout::Both => {
            let oy = usize::from(odd_y && height > 0);
            let loop_h = height - oy;
            for _ in 0..oy * width {
                put(luma, None);
                luma += 1;
            }
            let mut i = 0;
            while i + 1 < loop_h {
                let below = luma + width;
                row(put, &mut luma, Some(below), &mut chroma);
                // Past the lower row too.
                luma += width;
                i += 2;
            }
            if i < loop_h {
                row(put, &mut luma, None, &mut chroma);
            }
        }
    }
}
