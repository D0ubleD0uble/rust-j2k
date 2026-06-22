//! JPEG 2000 codestream marker codes and the segment structs we parse.
//!
//! ISO/IEC 15444-1 Annex A. Only the markers the GRIB2 subset needs are parsed
//! in full; the rest are recognised so they can be skipped or rejected cleanly.

/// Delimiting and fixed-information markers (ISO Table A-1).
pub mod marker {
    pub const SOC: u16 = 0xFF4F; // start of codestream
    pub const SOT: u16 = 0xFF90; // start of tile-part
    pub const SOD: u16 = 0xFF93; // start of data
    pub const EOC: u16 = 0xFFD9; // end of codestream
    pub const SIZ: u16 = 0xFF51; // image and tile size
    pub const COD: u16 = 0xFF52; // coding style default
    pub const COC: u16 = 0xFF53; // coding style component
    pub const RGN: u16 = 0xFF5E; // region of interest
    pub const QCD: u16 = 0xFF5C; // quantization default
    pub const QCC: u16 = 0xFF5D; // quantization component
    pub const POC: u16 = 0xFF5F; // progression order change
    pub const TLM: u16 = 0xFF55; // tile-part lengths
    pub const PLT: u16 = 0xFF58; // packet lengths, tile-part
    pub const SOP: u16 = 0xFF91; // start of packet
    pub const EPH: u16 = 0xFF92; // end of packet header
    pub const COM: u16 = 0xFF64; // comment
}

/// Wavelet transform (COD/COC, byte "SPcod transformation").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    /// 9/7 irreversible — lossy.
    Irreversible97,
    /// 5/3 reversible — lossless.
    Reversible53,
}

/// Progression order (COD, "SGcod progression order"). ISO Table A-16.
///
/// Only [`Lrcp`](Progression::Lrcp) is decoded today; `decode_cod` rejects the
/// others. The full set is kept so the parser names each code and the remaining
/// orders have a home when they come online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progression {
    /// Layer / resolution / component / position.
    Lrcp,
    /// Resolution / layer / component / position. Not yet decoded.
    #[allow(dead_code)]
    Rlcp,
    /// Resolution / position / component / layer. Not yet decoded.
    #[allow(dead_code)]
    Rpcl,
    /// Position / component / resolution / layer. Not yet decoded.
    #[allow(dead_code)]
    Pcrl,
    /// Component / position / resolution / layer. Not yet decoded.
    #[allow(dead_code)]
    Cprl,
}

/// Quantization style (QCD/QCC, low 5 bits of Sqcd). ISO Table A-28.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantStyle {
    /// No quantization — reversible path (5/3).
    None,
    /// Scalar, single step derived for all subbands.
    ScalarDerived,
    /// Scalar, explicit step per subband.
    ScalarExpounded,
}

/// One component's geometry from SIZ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizComponent {
    pub bit_depth: u8,
    pub signed: bool,
    pub x_sampling: u8,
    pub y_sampling: u8,
}

/// SIZ — image and tile size (ISO A.5.1). The GRIB2 subset expects exactly one
/// component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Siz {
    pub x_size: u32,
    pub y_size: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub tile_x_offset: u32,
    pub tile_y_offset: u32,
    pub components: Vec<SizComponent>,
}

/// COD — coding style default (ISO A.6.1): the parameters Tier-2 and the DWT
/// need (decomposition levels, code-block size + style, transform, precincts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cod {
    pub progression: Progression,
    pub layers: u16,
    pub decomposition_levels: u8,
    pub code_block_width: u8,  // exponent: width  = 2^(value + 2)
    pub code_block_height: u8, // exponent: height = 2^(value + 2)
    pub code_block_style: u8,  // bit flags: bypass, reset, restart, vcausal, segsym, …
    pub transform: Transform,
    /// Per-level precinct sizes when explicit; empty = maximal (PPx=PPy=15).
    pub precinct_sizes: Vec<(u8, u8)>,
}

/// QCD — quantization default (ISO A.6.4): the step sizes / exponents the
/// dequant stage applies, with the guard-bit count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qcd {
    pub style: QuantStyle,
    pub guard_bits: u8,
    /// (exponent, mantissa) per subband; mantissa is 0 for the reversible style.
    pub steps: Vec<(u8, u16)>,
}

impl Qcd {
    /// The `(exponent, mantissa)` quantization pair for subband index `band`
    /// (0 = LL, then `HL, LH, HH` per resolution level coarsest-first), or
    /// `None` if the QCD does not carry it. Reversible (`None` style) and
    /// expounded QCDs store one pair per subband; the derived style stores only
    /// subband 0 and drops the exponent by one per resolution level finer
    /// (E-5: `ε_b = max(ε_0 − ⌊(b−1)/3⌋, 0)`), keeping the single mantissa.
    ///
    /// Both the Tier-1 bit-plane count (`Mb`) and the dequant step read this, so
    /// the per-band mapping lives here once rather than in two places that must
    /// stay numerically identical.
    pub fn subband_step(&self, band: usize) -> Option<(u8, u16)> {
        match self.style {
            QuantStyle::None | QuantStyle::ScalarExpounded => self.steps.get(band).copied(),
            QuantStyle::ScalarDerived => {
                let (exp0, mant0) = *self.steps.first()?;
                let drop = u8::try_from(band.saturating_sub(1) / 3).unwrap_or(u8::MAX);
                Some((exp0.saturating_sub(drop), mant0))
            }
        }
    }
}
