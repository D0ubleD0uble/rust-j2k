//! sRGB output ([`ColourOutput::Srgb`](crate::ColourOutput::Srgb)): sYCC to
//! RGB, and a restricted ICC profile (I.5.3.3, `METH` 2) applied to give sRGB.
//!
//! The step runs after the palette and the channel definitions, on the colour
//! channels: the channels labelled colour 1, 2, 3 (one for greyscale). Other
//! channels, such as opacity, pass through untouched. [`plan`] checks
//! everything from the header before any tile is decoded; [`SrgbPlan::apply`]
//! converts the decoded samples. No conversion changes a channel's depth.
//!
//! - **sRGB and greyscale** (`EnumCS` 16, 17) pass through.
//! - **sYCC** (`EnumCS` 18) is converted with the IEC 61966-2-1 Amd 1 inverse
//!   matrix (1.402, 0.344136, 0.714136, 1.772) after taking `2^(depth-1)` off
//!   the chroma, rounded to the nearest integer and clamped to the depth.
//!   OpenJPEG uses 0.344 and 0.714 and truncates each product. The ISO
//!   reference images match this formula much more closely, so it is a
//!   documented deviation (`docs/correctness.md`). Chroma sub-sampled 2x1
//!   (4:2:2) or 2x2 (4:2:0) is replicated to the luma grid first. The
//!   replication follows OpenJPEG 2.5.0's `sycc422_to_rgb` and
//!   `sycc420_to_rgb` sample for sample, including the odd image origin: there
//!   the first column (and, for 4:2:0, the first row) takes chroma samples of
//!   0 rather than a neighbour's.
//! - **Restricted ICC** profiles of the two forms Annex I allows are applied
//!   in closed form: a monochrome input profile (`kTRC`) gives one grey
//!   channel, and a three-component matrix/TRC input profile (`rXYZ`, `gXYZ`,
//!   `bXYZ`, `rTRC`, `gTRC`, `bTRC`) gives RGB. Each channel is linearised by
//!   its curve, the matrix takes it to XYZ (D50), the inverse of the
//!   D50-adapted sRGB matrix takes it to linear sRGB, and the sRGB curve
//!   encodes it. This is the transform LittleCMS builds for these profiles,
//!   in floating point: the result is within one level of OpenJPEG + lcms.
//!
//! Everything else is [`Error::Unsupported`]: another enumerated colour
//! space, no colour space, a profile of another class or form, and colour
//! channels that differ in depth, sign or sub-sampling (except the sYCC
//! layouts above). OpenJPEG converts some of those anyway and gets them
//! wrong; `docs/correctness.md` lists each case.

use super::metadata::{Association, ChannelDefinition, ChannelType, ColourSpace};
use crate::codestream::MAX_IMAGE_SAMPLES;
use crate::codestream::markers::Siz;
use crate::{Component, ComponentInfo, Error, Result};

/// The XYZ (D50) to linear sRGB matrix: the inverse of the sRGB primaries
/// adapted to D50 with the Bradford transform, as LittleCMS builds its sRGB
/// profile (`cmsCreate_sRGBProfile`: Rec. 709 primaries, the white point of
/// `cmsWhitePointFromTemp(6504)`, D50 = 0.9642, 1.0, 0.8249). Computed in
/// double precision; each row of the forward matrix sums to D50.
const XYZ_D50_TO_SRGB: [[f64; 3]; 3] = [
    [3.135866755, -1.6186701432, -0.4908868735],
    [-0.9786373621, 1.9160069766, 0.0334527432],
    [0.0720010749, -0.229141816, 1.4058896589],
];

/// The ICC header and the tag count that follows it.
const PROFILE_HEADER: usize = 132;

/// What the sRGB step does to a file's channels, worked out before any tile
/// is decoded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SrgbPlan {
    /// The colour channels already are sRGB or grey.
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
        curves: Box<[Curve; 3]>,
        /// Linear RGB of the profile to linear sRGB, in one matrix.
        matrix: [[f64; 3]; 3],
    },
    /// A monochrome profile applied to the grey channel at this position.
    IccGrey { channel: usize, curve: Curve },
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

/// A tone reproduction curve: device value in `0..=1` to linear.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Curve {
    /// `curv` with no entries.
    Identity,
    /// `curv` with one entry: a `u8.8` gamma.
    Gamma(f64),
    /// `curv` with two or more entries, sampled evenly over `0..=1`.
    Table(Vec<u16>),
    /// `para` (ICC v4): the function type and its parameters, unused ones 0.
    Parametric(u16, [f64; 7]),
}

impl Curve {
    /// The curve at `x` in `0..=1`, clamped to `0..=1`.
    pub(crate) fn eval(&self, x: f64) -> f64 {
        let y = match self {
            Curve::Identity => x,
            Curve::Gamma(g) => x.powf(*g),
            Curve::Table(table) => {
                let last = table.len() - 1;
                let at = x.clamp(0.0, 1.0) * last as f64;
                let i = (at.floor() as usize).min(last);
                let j = (i + 1).min(last);
                let frac = at - i as f64;
                let (a, b) = (f64::from(table[i]), f64::from(table[j]));
                (a + (b - a) * frac) / 65535.0
            }
            Curve::Parametric(kind, p) => parametric(*kind, p, x),
        };
        if y.is_nan() { 0.0 } else { y.clamp(0.0, 1.0) }
    }
}

/// ICC.1:2010 10.18 `parametricCurveType`, in LittleCMS's form: a power of a
/// non-positive base is 0.
fn parametric(kind: u16, p: &[f64; 7], x: f64) -> f64 {
    let [g, a, b, c, d, e, f] = *p;
    let power = |base: f64| if base > 0.0 { base.powf(g) } else { 0.0 };
    match kind {
        0 => power(x),
        1 => power(a * x + b),
        2 => power(a * x + b) + c,
        3 if x >= d => power(a * x + b),
        3 => c * x,
        4 if x >= d => power(a * x + b) + e,
        _ => c * x + f,
    }
}

/// The sRGB encoding curve (IEC 61966-2-1) on a linear value, clamped first.
fn srgb_encode(linear: f64) -> f64 {
    let x = if linear.is_nan() {
        0.0
    } else {
        linear.clamp(0.0, 1.0)
    };
    if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// Plan the sRGB step for a JP2 file's output channels: their formats
/// (`outputs`), what each one is (`channels`), and the colour space the file
/// declares. `siz` and `reduction` size the output for the sample budget.
pub(crate) fn plan(
    colour: &ColourSpace,
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    siz: &Siz,
    reduction: u8,
) -> Result<SrgbPlan> {
    match colour {
        ColourSpace::Srgb | ColourSpace::Greyscale => {
            uniform(&colour_channels(channels, outputs, 0), outputs)?;
            Ok(SrgbPlan::PassThrough)
        }
        ColourSpace::Sycc => plan_sycc(channels, outputs, siz, reduction),
        ColourSpace::RestrictedIcc(profile) => plan_icc(profile, channels, outputs),
        ColourSpace::Enumerated(value) => Err(Error::Unsupported(format!(
            "sRGB output from the enumerated colour space {value}; only sRGB, greyscale, \
             sYCC and restricted ICC profiles are converted"
        ))),
        ColourSpace::Unknown => Err(Error::Unsupported(
            "sRGB output from a file with no colour specification".into(),
        )),
    }
}

/// The output positions of the channels labelled colour 1, 2, ... in colour
/// order, up to the first colour number with no channel. `need` colours must
/// be present; 0 takes whatever is there.
fn colour_channels(
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    need: u16,
) -> Vec<usize> {
    let mut found = Vec::new();
    for colour in 1..=u16::MAX {
        let at = channels.iter().position(|c| {
            c.kind == ChannelType::Colour && c.association == Association::Colour(colour)
        });
        match at {
            Some(at) if at < outputs.len() => found.push(at),
            _ => break,
        }
        if need != 0 && colour == need {
            break;
        }
    }
    found
}

/// The colour channels must share depth, sign and sub-sampling.
fn uniform(colours: &[usize], outputs: &[ComponentInfo]) -> Result<()> {
    same_format(colours, outputs)?;
    let Some(&first) = colours.first() else {
        return Ok(());
    };
    let first = outputs[first];
    for &at in &colours[1..] {
        let other = outputs[at];
        if (other.x_sampling, other.y_sampling) != (first.x_sampling, first.y_sampling) {
            return Err(Error::Unsupported(format!(
                "sRGB output from colour channels of different sub-sampling ({}x{} and {}x{})",
                first.x_sampling, first.y_sampling, other.x_sampling, other.y_sampling
            )));
        }
    }
    Ok(())
}

/// The colour channels must share depth and sign.
fn same_format(colours: &[usize], outputs: &[ComponentInfo]) -> Result<()> {
    let Some(&first) = colours.first() else {
        return Ok(());
    };
    let first = outputs[first];
    for &at in &colours[1..] {
        let other = outputs[at];
        if (other.bit_depth, other.signed) != (first.bit_depth, first.signed) {
            return Err(Error::Unsupported(format!(
                "sRGB output from colour channels of different formats ({}-bit {} and \
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

/// Exactly three colour channels for sYCC or an RGB profile.
fn three_colours(
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    what: &str,
) -> Result<[usize; 3]> {
    let found = colour_channels(channels, outputs, 3);
    <[usize; 3]>::try_from(found.as_slice()).map_err(|_| {
        Error::Unsupported(format!(
            "sRGB output from {what} needs colour channels 1, 2 and 3; the file has {}",
            found.len()
        ))
    })
}

fn plan_sycc(
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
    siz: &Siz,
    reduction: u8,
) -> Result<SrgbPlan> {
    let colours = three_colours(channels, outputs, "sYCC")?;
    same_format(&colours, outputs)?;
    let [y, cb, cr] = colours.map(|at| outputs[at]);
    if y.signed {
        return Err(Error::Unsupported(
            "sRGB output from signed sYCC samples".into(),
        ));
    }
    let sampling = |c: ComponentInfo| (c.x_sampling, c.y_sampling);
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
    check_budget(outputs, &colours, siz, reduction)?;
    Ok(SrgbPlan::Sycc {
        channels: colours,
        layout,
        odd_x: siz.x_offset & 1 == 1,
        odd_y: siz.y_offset & 1 == 1,
    })
}

/// The output after chroma replication must stay under the decode guard: two
/// chroma channels grow to the luma grid. Every other channel keeps its size.
fn check_budget(
    outputs: &[ComponentInfo],
    colours: &[usize; 3],
    siz: &Siz,
    reduction: u8,
) -> Result<()> {
    let luma = siz.image_extent_at(reduction);
    let mut total: u64 = 0;
    for (at, info) in outputs.iter().enumerate() {
        let (width, height) = if colours.contains(&at) {
            luma
        } else {
            siz.sampled_extent_at(info.x_sampling, info.y_sampling, reduction)
                .ok_or_else(|| Error::Inconsistent("a channel has a zero sub-sampling".into()))?
        };
        total = total.saturating_add(u64::from(width) * u64::from(height));
    }
    if total > MAX_IMAGE_SAMPLES {
        return Err(Error::Limit(format!(
            "sYCC chroma replication grows the output to over {MAX_IMAGE_SAMPLES} samples, \
             above the decode guard"
        )));
    }
    Ok(())
}

fn plan_icc(
    profile: &[u8],
    channels: &[ChannelDefinition],
    outputs: &[ComponentInfo],
) -> Result<SrgbPlan> {
    let icc = Profile::parse(profile)?;
    match &icc.space {
        b"GRAY" => {
            let colours = colour_channels(channels, outputs, 0);
            let [channel] = colours[..] else {
                return Err(Error::Unsupported(format!(
                    "sRGB output from a monochrome ICC profile needs one colour channel; \
                     the file has {}",
                    colours.len()
                )));
            };
            check_icc_format(outputs[channel], &[8], "monochrome")?;
            Ok(SrgbPlan::IccGrey {
                channel,
                curve: icc.curve(b"kTRC")?,
            })
        }
        _ => {
            let colours = three_colours(channels, outputs, "an RGB ICC profile")?;
            uniform(&colours, outputs)?;
            check_icc_format(outputs[colours[0]], &[8, 16], "RGB")?;
            let curves = Box::new([
                icc.curve(b"rTRC")?,
                icc.curve(b"gTRC")?,
                icc.curve(b"bTRC")?,
            ]);
            let columns = [icc.xyz(b"rXYZ")?, icc.xyz(b"gXYZ")?, icc.xyz(b"bXYZ")?];
            let mut matrix = [[0.0; 3]; 3];
            for (row, out) in XYZ_D50_TO_SRGB.iter().zip(&mut matrix) {
                for (k, cell) in out.iter_mut().enumerate() {
                    *cell = (0..3).map(|i| row[i] * columns[k][i]).sum();
                }
            }
            Ok(SrgbPlan::IccRgb {
                channels: colours,
                curves,
                matrix,
            })
        }
    }
}

/// The depths a profile is applied at: those where OpenJPEG + lcms read the
/// samples at their own depth (8-bit and 16-bit RGB, 8-bit grey). At other
/// depths OpenJPEG hands lcms the samples as 8- or 16-bit values and the
/// result is wrong, so this decoder refuses them.
fn check_icc_format(info: ComponentInfo, depths: &[u8], form: &str) -> Result<()> {
    if info.signed || !depths.contains(&info.bit_depth) {
        return Err(Error::Unsupported(format!(
            "sRGB output through a {form} ICC profile from {}-bit {} samples",
            info.bit_depth,
            sign(info.signed)
        )));
    }
    Ok(())
}

/// A restricted ICC profile (ICC.1, as Annex I restricts it): the header
/// fields that matter and the tag table.
struct Profile<'a> {
    bytes: &'a [u8],
    space: [u8; 4],
    table: &'a [u8],
}

impl<'a> Profile<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        let malformed = |what: String| Error::FileFormat(format!("the ICC profile {what}"));
        let unsupported = |what: String| {
            Error::Unsupported(format!(
                "sRGB output through an ICC profile {what}; only the monochrome and \
                 three-component matrix/TRC input profiles of Annex I are applied"
            ))
        };
        if bytes.len() < PROFILE_HEADER {
            return Err(malformed(format!(
                "is {} bytes, shorter than its 128-byte header and tag count",
                bytes.len()
            )));
        }
        let four = |at: usize| -> [u8; 4] {
            let mut out = [0; 4];
            out.copy_from_slice(&bytes[at..at + 4]);
            out
        };
        let class = four(12);
        if &class != b"scnr" {
            return Err(unsupported(format!(
                "of class `{}`, not an input profile",
                String::from_utf8_lossy(&class)
            )));
        }
        let space = four(16);
        if &space != b"RGB " && &space != b"GRAY" {
            return Err(unsupported(format!(
                "for the `{}` colour space",
                String::from_utf8_lossy(&space)
            )));
        }
        let pcs = four(20);
        if &pcs != b"XYZ " {
            return Err(unsupported(format!(
                "with the `{}` connection space",
                String::from_utf8_lossy(&pcs)
            )));
        }
        let intent = u32::from_be_bytes(four(64));
        if intent > 2 {
            return Err(unsupported(format!("with rendering intent {intent}")));
        }
        let count = u32::from_be_bytes(four(128));
        let table = usize::try_from(count)
            .ok()
            .and_then(|n| n.checked_mul(12))
            .and_then(|len| bytes.get(PROFILE_HEADER..PROFILE_HEADER.checked_add(len)?))
            .ok_or_else(|| malformed(format!("declares {count} tags, past its end")))?;
        let profile = Profile {
            bytes,
            space,
            table,
        };
        for lut in [
            b"A2B0", b"A2B1", b"A2B2", b"D2B0", b"D2B1", b"D2B2", b"D2B3",
        ] {
            if profile.entry(lut).is_some() {
                return Err(unsupported(format!(
                    "with a `{}` lookup table",
                    String::from_utf8_lossy(lut)
                )));
            }
        }
        Ok(profile)
    }

    /// The first tag table entry for `signature`: its offset and size.
    fn entry(&self, signature: &[u8; 4]) -> Option<(u32, u32)> {
        self.table.chunks_exact(12).find_map(|e| {
            (&e[..4] == signature).then(|| {
                let word = |at: usize| u32::from_be_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
                (word(4), word(8))
            })
        })
    }

    /// The bytes of tag `signature`, which must be present and inside the
    /// profile.
    fn tag(&self, signature: &[u8; 4]) -> Result<&'a [u8]> {
        let name = String::from_utf8_lossy(signature);
        let (offset, size) = self.entry(signature).ok_or_else(|| {
            Error::Unsupported(format!(
                "sRGB output through an ICC profile without a `{name}` tag; only the \
                 monochrome and three-component matrix/TRC input profiles of Annex I are \
                 applied"
            ))
        })?;
        let start = usize::try_from(offset).ok();
        let end = start
            .zip(usize::try_from(size).ok())
            .and_then(|(s, n)| s.checked_add(n));
        start
            .zip(end)
            .and_then(|(start, end)| self.bytes.get(start..end))
            .ok_or_else(|| {
                Error::FileFormat(format!(
                    "the ICC profile's `{name}` tag ({size} bytes at {offset}) runs past its \
                     end"
                ))
            })
    }

    /// An `XYZType` tag's one XYZ value.
    fn xyz(&self, signature: &[u8; 4]) -> Result<[f64; 3]> {
        let data = self.tag(signature)?;
        if data.len() < 20 || &data[..4] != b"XYZ " {
            return Err(Error::FileFormat(format!(
                "the ICC profile's `{}` tag is not an `XYZ ` value",
                String::from_utf8_lossy(signature)
            )));
        }
        Ok([s15_16(&data[8..]), s15_16(&data[12..]), s15_16(&data[16..])])
    }

    /// A `curveType` or `parametricCurveType` tag.
    fn curve(&self, signature: &[u8; 4]) -> Result<Curve> {
        let data = self.tag(signature)?;
        let name = String::from_utf8_lossy(signature);
        let short = || {
            Error::FileFormat(format!(
                "the ICC profile's `{name}` curve is cut short ({} bytes)",
                data.len()
            ))
        };
        let kind = data.get(..4).ok_or_else(short)?;
        match kind {
            b"curv" => {
                let count = data.get(8..12).ok_or_else(short)?;
                let count = u32::from_be_bytes([count[0], count[1], count[2], count[3]]);
                let entries = usize::try_from(count)
                    .ok()
                    .and_then(|n| n.checked_mul(2))
                    .and_then(|len| data.get(12..12usize.checked_add(len)?))
                    .ok_or_else(short)?;
                let values: Vec<u16> = entries
                    .chunks_exact(2)
                    .map(|v| u16::from_be_bytes([v[0], v[1]]))
                    .collect();
                Ok(match values[..] {
                    [] => Curve::Identity,
                    [gamma] => Curve::Gamma(f64::from(gamma) / 256.0),
                    _ => Curve::Table(values),
                })
            }
            b"para" => {
                let function = data.get(8..10).ok_or_else(short)?;
                let function = u16::from_be_bytes([function[0], function[1]]);
                let used = match function {
                    0 => 1,
                    1 => 3,
                    2 => 4,
                    3 => 5,
                    4 => 7,
                    _ => {
                        return Err(Error::Unsupported(format!(
                            "sRGB output through an ICC `{name}` curve of parametric \
                             function type {function}"
                        )));
                    }
                };
                let raw = data.get(12..12 + 4 * used).ok_or_else(short)?;
                let mut params = [0.0; 7];
                for (param, bytes) in params.iter_mut().zip(raw.chunks_exact(4)) {
                    *param = s15_16(bytes);
                }
                Ok(Curve::Parametric(function, params))
            }
            other => Err(Error::Unsupported(format!(
                "sRGB output through an ICC `{name}` curve of type `{}`",
                String::from_utf8_lossy(other)
            ))),
        }
    }
}

/// An ICC `s15Fixed16Number` from the first four bytes of `bytes`.
fn s15_16(bytes: &[u8]) -> f64 {
    f64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])) / 65536.0
}

impl SrgbPlan {
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
                curves,
                matrix,
            } => {
                let [r, g, b] = channels.map(|at| &components[at]);
                let rgb = icc_rgb(r, g, b, curves, matrix)?;
                for (at, out) in channels.iter().zip(rgb) {
                    components[*at] = out;
                }
            }
            SrgbPlan::IccGrey { channel, curve } => {
                let grey = &mut components[*channel];
                let lut = linearise(curve, grey.bit_depth);
                let max = lut.len() - 1;
                let scale = max as f64;
                for sample in &mut grey.samples {
                    let linear = lut[clamp_index(*sample, max)];
                    *sample = (srgb_encode(linear) * scale).round() as i32;
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

/// The curve sampled at every code value of an unsigned `depth`-bit channel
/// (`depth` is 8 or 16: checked by the plan).
fn linearise(curve: &Curve, depth: u8) -> Vec<f64> {
    let max = (1u32 << depth) - 1;
    (0..=max)
        .map(|v| curve.eval(f64::from(v) / f64::from(max)))
        .collect()
}

fn icc_rgb(
    r: &Component,
    g: &Component,
    b: &Component,
    curves: &[Curve; 3],
    matrix: &[[f64; 3]; 3],
) -> Result<[Component; 3]> {
    same_grid(&[r, g, b])?;
    let luts = curves.each_ref().map(|curve| linearise(curve, r.bit_depth));
    let max = luts[0].len() - 1;
    let scale = max as f64;
    let mut out = [r.clone(), g.clone(), b.clone()];
    for i in 0..r.samples.len() {
        let linear = [
            luts[0][clamp_index(r.samples[i], max)],
            luts[1][clamp_index(g.samples[i], max)],
            luts[2][clamp_index(b.samples[i], max)],
        ];
        for (row, channel) in matrix.iter().zip(out.iter_mut()) {
            let value = row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2];
            channel.samples[i] = (srgb_encode(value) * scale).round() as i32;
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
                "colour channels decoded at {}x{} and {}x{}",
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
/// `sycc422_to_rgb`, `sycc420_to_rgb` in 2.5.0), calling `put(luma, chroma)`
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
