//! Stage-neutral pipeline data models.
//!
//! These types flow *between* stages, so they live apart from any one stage
//! that happens to produce them. The coefficient pyramid ([`Band`],
//! [`DetailBands`], [`Bands`], [`SubbandCoeffs`]) is produced by a block coder
//! and consumed by dequant and the inverse DWT; keeping it here lets a future
//! HT block decoder produce it without depending on the EBCOT module purely for
//! its types. [`Samples`] carries one component's reconstructed values from the
//! inverse DWT through the colour transform to image assembly.

/// One subband (or the coarsest LL): a row-major coefficient grid plus its
/// tile-component origin. `data.len() == width * height`, addressed as
/// `data[y * width + x]`.
///
/// `origin` is the `(x, y)` tile-component coordinate of the top-left sample —
/// the band's `tbx0`/`tby0` (ISO Eq. B-15), not an offset within the tile. The
/// inverse DWT reads the interleave parity straight off it (see [`crate::dwt`]),
/// so it is load-bearing, not bookkeeping: a tile away from the canvas origin,
/// or a sub-sampled component, puts a band at an odd origin, and a band at an
/// odd origin starts on a high-pass sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Band<T> {
    pub origin: (u32, u32),
    pub width: usize,
    pub height: usize,
    pub data: Vec<T>,
}

/// The three detail subbands added at one resolution level (ISO xob/yob from
/// Table F.1): `hl` is high-pass horizontally / low-pass vertically, `lh` the
/// reverse, `hh` high-pass in both.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailBands<T> {
    pub hl: Band<T>,
    pub lh: Band<T>,
    pub hh: Band<T>,
}

/// All subbands of one tile-component: the coarsest `ll` plus the detail bands
/// added at each resolution level, **coarsest first**. `levels.len()` equals the
/// COD decomposition-level count; an empty `levels` means no transform was
/// applied and `ll` is the image itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Bands<T> {
    pub ll: Band<T>,
    pub levels: Vec<DetailBands<T>>,
}

/// Quantized wavelet coefficients flowing Tier-1 → dequant → inverse DWT. The
/// reversible (5/3) path stays integer so it can reconstruct bit-exactly; the
/// irreversible (9/7) path is real-valued once dequantized, so it carries
/// `f32`. The arm matches the COD transform.
#[derive(Debug, Clone, PartialEq)]
pub enum SubbandCoeffs {
    /// 5/3 reversible: exact integer coefficients.
    Reversible(Bands<i32>),
    /// 9/7 irreversible: real-valued coefficients.
    Irreversible(Bands<f32>),
}

/// One component's reconstructed samples, still on the arithmetic the wavelet
/// used. Reversible 5/3 reconstructs in exact integers; irreversible 9/7
/// reconstructs in `f32` and is *not* rounded here.
///
/// The rounding is deferred to [`crate::image::assemble`] so that it happens
/// exactly once, after the inverse colour transform. ICT mixes the first three
/// components arithmetically (Annex G.3), and rounding each of them beforehand
/// would inject up to half a unit of error into that mix. OpenJPEG defers it the
/// same way: `opj_tcd_mct_decode` runs on `OPJ_FLOAT32*`, and
/// `opj_tcd_dc_level_shift_decode` is what calls `opj_lrintf`.
#[derive(Debug, Clone, PartialEq)]
pub enum Samples {
    /// 5/3 reversible: exact integers.
    Reversible(Vec<i32>),
    /// 9/7 irreversible: real-valued, unrounded.
    Irreversible(Vec<f32>),
}

impl Samples {
    /// Number of samples, whichever arm carries them.
    pub fn len(&self) -> usize {
        match self {
            Samples::Reversible(v) => v.len(),
            Samples::Irreversible(v) => v.len(),
        }
    }
}
