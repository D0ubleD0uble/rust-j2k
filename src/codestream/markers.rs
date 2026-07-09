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
    pub const CAP: u16 = 0xFF50; // extended capabilities (beyond Part 1)
    pub const SIZ: u16 = 0xFF51; // image and tile size
    pub const COD: u16 = 0xFF52; // coding style default
    pub const COC: u16 = 0xFF53; // coding style component
    pub const RGN: u16 = 0xFF5E; // region of interest
    pub const QCD: u16 = 0xFF5C; // quantization default
    pub const QCC: u16 = 0xFF5D; // quantization component
    pub const POC: u16 = 0xFF5F; // progression order change
    pub const TLM: u16 = 0xFF55; // tile-part lengths
    pub const PLM: u16 = 0xFF57; // packet lengths, main header
    pub const PLT: u16 = 0xFF58; // packet lengths, tile-part
    pub const PPM: u16 = 0xFF60; // packed packet headers, main header
    pub const PPT: u16 = 0xFF61; // packed packet headers, tile-part
    pub const CRG: u16 = 0xFF63; // component registration
    pub const SOP: u16 = 0xFF91; // start of packet
    pub const EPH: u16 = 0xFF92; // end of packet header
    pub const COM: u16 = 0xFF64; // comment

    /// Reserved markers that carry no segment (ISO Table A-1). A walker that
    /// reads a length after one of these consumes the bytes that follow it;
    /// conformance codestream `p0_02` puts `0xFF30` in its main header to catch
    /// exactly that bug.
    pub const RESERVED_NO_SEGMENT: std::ops::RangeInclusive<u16> = 0xFF30..=0xFF3F;

    /// Whether `m` is a marker code at all: every marker's high byte is `0xFF`.
    pub fn is_marker(m: u16) -> bool {
        m >> 8 == 0xFF
    }

    /// Whether `m` is followed by a length field and a segment body.
    ///
    /// The delimiting markers and the reserved `0xFF30..=0xFF3F` range stand
    /// alone. `SOP` is not in this list: it does carry an `Lsop` length.
    pub fn has_segment(m: u16) -> bool {
        !matches!(m, SOC | SOD | EOC | EPH) && !RESERVED_NO_SEGMENT.contains(&m)
    }
}

/// The code-block style flags of `SPcod`/`SPcoc` (ISO Table A-19), plus the two
/// high-throughput flags the HTJ2K amendment adds in the same byte.
///
/// Each flag changes how Tier-1 reads a code-block's coded segments, so a
/// decoder that ignores one does not decode a slightly different image — it
/// decodes the wrong one. None are decoded yet; `decode_cod` rejects any that
/// are set.
pub mod code_block_style {
    /// Bit 2: the MQ coder terminates after every coding pass, so each pass is
    /// an independently decodable codeword segment.
    pub const TERMALL: u8 = 0x04;

    /// Every flag of the style byte, low bit first, with the name used in error
    /// messages. All eight bits are allocated, so no value goes unnamed.
    ///
    /// Bits `0x40` and `0x80` select the HTJ2K block coder (ISO/IEC 15444-15;
    /// OpenJPEG's `J2K_CCP_CBLKSTY_HT` and `J2K_CCP_CBLKSTY_HTMIXED`). A
    /// codestream that sets them also carries a `CAP` marker, which is rejected
    /// on its own, but naming them here keeps the message accurate either way.
    pub const FLAGS: [(u8, &str); 8] = [
        (0x01, "selective arithmetic coding bypass"),
        (0x02, "reset context probabilities"),
        (0x04, "termination on each coding pass"),
        (0x08, "vertically causal context"),
        (0x10, "predictable termination"),
        (0x20, "segmentation symbols"),
        (0x40, "HTJ2K high-throughput block coding"),
        (0x80, "HTJ2K mixed-mode block coding"),
    ];

    /// Name the flags set in `style`, comma-separated. Returns an empty string
    /// for the default style, which `decode_cod` never asks about.
    pub fn describe(style: u8) -> String {
        FLAGS
            .iter()
            .filter(|(bit, _)| style & bit != 0)
            .map(|(_, name)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    }
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
/// All five are decoded. Under maximal precincts the position axis has one
/// value, so `Pcrl` and `Cprl` enumerate the same packet sequence; see
/// `tier2::for_each_packet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progression {
    /// Layer / resolution / component / position.
    Lrcp,
    /// Resolution / layer / component / position.
    Rlcp,
    /// Resolution / position / component / layer.
    Rpcl,
    /// Position / component / resolution / layer.
    Pcrl,
    /// Component / position / resolution / layer.
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

/// SIZ — image and tile size (ISO A.5.1), including every declared component.
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

/// The largest `Csiz` the standard allows (ISO A.5.1, Table A-9).
pub const MAX_COMPONENTS: u16 = 16384;

impl Siz {
    /// The image area on the reference grid: `Xsiz - XOsiz` by `Ysiz - YOsiz`.
    pub fn image_extent(&self) -> (u32, u32) {
        (
            self.x_size.saturating_sub(self.x_offset),
            self.y_size.saturating_sub(self.y_offset),
        )
    }

    /// Component `index`'s own sample-grid dimensions (ISO/IEC 15444-1 §B.2):
    ///
    /// ```text
    /// width  = ceil(Xsiz / XRsiz) - ceil(XOsiz / XRsiz)
    /// height = ceil(Ysiz / YRsiz) - ceil(YOsiz / YRsiz)
    /// ```
    ///
    /// which reduces to [`image_extent`](Self::image_extent) under unit
    /// sub-sampling.
    ///
    /// Returns `None` if `index` names no component, or if that component
    /// declares a zero sub-sampling factor. `decode_siz` rejects a zero factor,
    /// so the second case is unreachable for any `Siz` the parser produced —
    /// but the division is guarded here rather than by a `debug_assert` that
    /// would compile out and leave a release-mode panic behind.
    pub fn component_extent(&self, index: usize) -> Option<(u32, u32)> {
        let comp = self.components.get(index)?;
        let (xr, yr) = (comp.x_sampling as u32, comp.y_sampling as u32);
        if xr == 0 || yr == 0 {
            return None;
        }
        Some((
            self.x_size
                .div_ceil(xr)
                .saturating_sub(self.x_offset.div_ceil(xr)),
            self.y_size
                .div_ceil(yr)
                .saturating_sub(self.y_offset.div_ceil(yr)),
        ))
    }
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
    /// `Scod` bit 1: SOP marker segments *may* precede each packet. The standard
    /// makes them optional even when the bit is set (A.8.1), so a decoder must
    /// tolerate their absence rather than demand them.
    pub use_sop: bool,
    /// `Scod` bit 2: an EPH marker *shall* follow every packet header (A.8.2),
    /// including an empty packet's.
    pub use_eph: bool,
    /// `SGcod` multiple-component transform: whether the first three components
    /// were decorrelated before the wavelet. *Which* transform follows from
    /// [`transform`](Self::transform) -- 5/3 means RCT, 9/7 means ICT -- not
    /// from a flag of its own.
    pub multiple_component_transform: bool,
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
