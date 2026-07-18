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

    /// The marker's name, for an error message. `"POC"` reads a great deal
    /// better than `0xFF5F` in a rejection a caller has to act on, and a
    /// conformance report that lists which feature blocks each entry is only
    /// useful if it names the feature.
    pub fn name(m: u16) -> Option<&'static str> {
        Some(match m {
            SOC => "SOC",
            SOT => "SOT",
            SOD => "SOD",
            EOC => "EOC",
            CAP => "CAP",
            SIZ => "SIZ",
            COD => "COD",
            COC => "COC",
            RGN => "RGN",
            QCD => "QCD",
            QCC => "QCC",
            POC => "POC",
            TLM => "TLM",
            PLM => "PLM",
            PLT => "PLT",
            PPM => "PPM",
            PPT => "PPT",
            CRG => "CRG",
            SOP => "SOP",
            EPH => "EPH",
            COM => "COM",
            _ => return None,
        })
    }

    /// The marker's name and code, for an error message: `"POC (0xFF5F)"`, or
    /// just the code when the marker is not one this decoder knows.
    pub fn describe(m: u16) -> String {
        match name(m) {
            Some(name) => format!("{name} ({m:#06X})"),
            None => format!("{m:#06X}"),
        }
    }

    /// Whether `m` is a marker code at all: the high byte is `0xFF` and the
    /// low byte is neither `0x00` nor `0xFF` — those two values are assigned
    /// to no marker. Meeting one in marker position is lost sync (a stuffed
    /// byte or fill read as structure), not an unfamiliar segment to walk over.
    pub fn is_marker(m: u16) -> bool {
        m >> 8 == 0xFF && !matches!(m & 0xFF, 0x00 | 0xFF)
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
/// decodes the wrong one. `decode_cod` rejects the two HTJ2K bits (`0x40`,
/// `0x80`); the six Part 1 flags below are decoded.
pub mod code_block_style {
    /// Bit 0: selective arithmetic coding bypass (lazy). From the fifth coded
    /// bit-plane on, the significance-propagation and magnitude-refinement passes
    /// are coded as raw bits instead of MQ-coded decisions, and each raw run and
    /// each MQ cleanup is a separately terminated codeword segment (D.5). Part of
    /// the codeword: a decoder that runs the whole block as one MQ codeword
    /// reconstructs garbage.
    pub const LAZY: u8 = 0x01;

    /// Bit 3: vertically causal context. The 3×3 significance context of a
    /// sample never reaches into the *next* stripe — a sample on a stripe's
    /// bottom row treats the row below it (which belongs to the following stripe)
    /// as insignificant. This *is* part of the codeword: it changes which context
    /// every affected decision is coded under, so a decoder that ignores the flag
    /// desynchronises the MQ coder (D.3).
    pub const VCAUSAL: u8 = 0x08;

    /// Bit 1: reset the MQ context probability estimates to their initial states
    /// at the end of every coding pass, so each pass starts from the same context
    /// model. Like [`VCAUSAL`], this is part of the codeword — it changes the
    /// probability every later decision is coded against (D.3).
    pub const RESET: u8 = 0x02;

    /// Bit 2: the MQ coder terminates after every coding pass, so each pass is
    /// an independently decodable codeword segment.
    pub const TERMALL: u8 = 0x04;

    /// Bit 4: the encoder used a predictable MQ termination, so a decoder can
    /// check that each codeword segment ends where it should. It constrains the
    /// encoder, not the decoder: the coded symbols, and so the reconstructed
    /// coefficients, are the same either way.
    pub const PTERM: u8 = 0x10;

    /// Bit 5: every cleanup pass ends with a four-symbol `1010` segmentation
    /// symbol, coded in the uniform context. Unlike [`PTERM`] this *is* part of
    /// the codeword: a decoder that does not consume the four decisions falls
    /// out of step with the MQ coder for every later pass.
    pub const SEGSYM: u8 = 0x20;

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

impl Progression {
    /// The progression order for a `SGcod`/`Ppoc` code (Table A-16), or `None`
    /// for a reserved value.
    pub fn from_u8(code: u8) -> Option<Progression> {
        Some(match code {
            0 => Progression::Lrcp,
            1 => Progression::Rlcp,
            2 => Progression::Rpcl,
            3 => Progression::Pcrl,
            4 => Progression::Cprl,
            _ => return None,
        })
    }
}

/// One progression volume of a `POC` marker (ISO/IEC 15444-1 A.6.6): a
/// progression order applied to a sub-range of the layer, resolution, and
/// component axes. A `POC` marker carries a sequence of these, and the packet
/// stream is their concatenation with each packet emitted once, by the first
/// volume that reaches it.
///
/// The bounds are stored raw as the marker carries them; the layer end is
/// clamped to the layer count and the component end to the component count where
/// they are used, since neither is known for certain at the marker's position in
/// the header (Table A-33). The layer range always starts at 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PocVolume {
    /// `RSpoc`: first resolution (inclusive).
    pub res_start: u8,
    /// `REpoc`: last resolution (exclusive).
    pub res_end: u8,
    /// `CSpoc`: first component (inclusive).
    pub comp_start: u16,
    /// `CEpoc`: last component (exclusive), clamped to the component count.
    pub comp_end: u16,
    /// `LYEpoc`: last layer (exclusive), clamped to the layer count.
    pub layer_end: u16,
    /// `Ppoc`: the progression order over this volume.
    pub progression: Progression,
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

/// One TLM entry (ISO A.7.1): the length in bytes of one tile-part — first
/// byte of its SOT to the end of its data — and the tile it belongs to. TLM is
/// informational, a pointer into the codestream rather than a decode
/// parameter, so decoding neither needs nor uses it; the entries are recorded
/// so callers and tests can check them against the tile-parts themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlmEntry {
    /// `Ttlm` — the tile index, explicit or implied by entry order (`ST = 0`).
    pub tile_index: u32,
    /// `Ptlm` — the tile-part's length in bytes.
    pub length: u32,
}

/// A half-open rectangle `[x0, x1) × [y0, y1)` on some sample grid: the
/// reference grid for [`Siz::tile_rect`], a component's own grid for
/// [`Siz::tile_component_rect`] and [`Siz::component_rect`].
///
/// The standard writes every geometric bound this way (`tx0 … tx1`, `tcx0 …
/// tcx1`, `trx0 … trx1`, ISO/IEC 15444-1 B.3–B.5), and the whole tiled decode
/// is bookkeeping over these: a tile-component's rect says where in the canvas
/// its reconstructed samples belong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    /// Width, or 0 when the rectangle is empty.
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    /// Height, or 0 when the rectangle is empty.
    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }

    /// Sample count.
    pub fn area(&self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }

    /// True when the rectangle encloses no samples.
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// The same rectangle on the grid `reduction` resolution levels coarser:
    /// **each bound** divided by `2^reduction`, rounding up (ISO B.5's
    /// `trx0 = ceil(tcx0 / 2^r)`). Rounding the bounds rather than the extent is
    /// what keeps neighbouring tiles abutting exactly: a shared edge rounds the
    /// same way from both sides, so the reduced tiles still partition the
    /// reduced canvas.
    pub fn reduced(&self, reduction: u8) -> Rect {
        let scale = 1u64 << reduction.min(32);
        let reduce = |v: u32| u64::from(v).div_ceil(scale) as u32;
        Rect {
            x0: reduce(self.x0),
            y0: reduce(self.y0),
            x1: reduce(self.x1),
            y1: reduce(self.y1),
        }
    }
}

impl Siz {
    /// The tile grid: how many tiles span the image horizontally and vertically
    /// (ISO/IEC 15444-1 B.3, Eq. B-5):
    ///
    /// ```text
    /// numXtiles = ceil((Xsiz - XTOsiz) / XTsiz)
    /// numYtiles = ceil((Ysiz - YTOsiz) / YTsiz)
    /// ```
    ///
    /// Returns `(0, 0)` if a tile dimension is zero, which `decode_siz` rejects.
    pub fn tile_grid(&self) -> (u32, u32) {
        if self.tile_width == 0 || self.tile_height == 0 {
            return (0, 0);
        }
        let across = u64::from(self.x_size).saturating_sub(u64::from(self.tile_x_offset));
        let down = u64::from(self.y_size).saturating_sub(u64::from(self.tile_y_offset));
        (
            across.div_ceil(u64::from(self.tile_width)) as u32,
            down.div_ceil(u64::from(self.tile_height)) as u32,
        )
    }

    /// Total tile count — the number of tile indices a conformant codestream
    /// carries, each as one or more tile-parts.
    pub fn num_tiles(&self) -> u32 {
        let (across, down) = self.tile_grid();
        across.saturating_mul(down)
    }

    /// Tile `index`'s area on the reference grid, clipped to the image area
    /// (ISO B.3, Eq. B-7). Tiles are numbered raster-order from the top-left.
    ///
    /// ```text
    /// tx0 = max(XTOsiz + p * XTsiz, XOsiz)   tx1 = min(XTOsiz + (p+1) * XTsiz, Xsiz)
    /// ```
    ///
    /// The clip is what makes the right and bottom tiles of a grid that does not
    /// divide the image evenly come out partial. Returns `None` if `index` is
    /// past the grid.
    pub fn tile_rect(&self, index: u32) -> Option<Rect> {
        let (across, _) = self.tile_grid();
        if across == 0 || index >= self.num_tiles() {
            return None;
        }
        let (p, q) = (u64::from(index % across), u64::from(index / across));
        let (xt, yt) = (u64::from(self.tile_width), u64::from(self.tile_height));
        let (xto, yto) = (u64::from(self.tile_x_offset), u64::from(self.tile_y_offset));
        // Every term is bounded by Xsiz/Ysiz after the min, so the u64
        // arithmetic lands back in u32 without a lossy cast.
        Some(Rect {
            x0: (xto + p * xt).max(u64::from(self.x_offset)) as u32,
            y0: (yto + q * yt).max(u64::from(self.y_offset)) as u32,
            x1: (xto + (p + 1) * xt).min(u64::from(self.x_size)) as u32,
            y1: (yto + (q + 1) * yt).min(u64::from(self.y_size)) as u32,
        })
    }

    /// Tile `index`'s area on component `comp`'s own sample grid (ISO B.3,
    /// Eq. B-12): the reference-grid tile rect with every bound divided by that
    /// component's sub-sampling factors, rounding up.
    ///
    /// Dividing the bounds rather than the extent is again what keeps the tiles
    /// abutting: a sub-sampled component's tiles still partition its grid
    /// exactly. Returns `None` if `index` names no tile or `comp` no component.
    pub fn tile_component_rect(&self, index: u32, comp: usize) -> Option<Rect> {
        let tile = self.tile_rect(index)?;
        let c = self.components.get(comp)?;
        let (xr, yr) = (u64::from(c.x_sampling), u64::from(c.y_sampling));
        if xr == 0 || yr == 0 {
            return None;
        }
        let div = |v: u32, r: u64| u64::from(v).div_ceil(r) as u32;
        Some(Rect {
            x0: div(tile.x0, xr),
            y0: div(tile.y0, yr),
            x1: div(tile.x1, xr),
            y1: div(tile.y1, yr),
        })
    }

    /// The whole image area on component `comp`'s sample grid (ISO B.2) — the
    /// canvas each tile-component is placed into. Its extent is
    /// [`component_extent`](Self::component_extent).
    pub fn component_rect(&self, comp: usize) -> Option<Rect> {
        let c = self.components.get(comp)?;
        let (xr, yr) = (u64::from(c.x_sampling), u64::from(c.y_sampling));
        if xr == 0 || yr == 0 {
            return None;
        }
        let div = |v: u32, r: u64| u64::from(v).div_ceil(r) as u32;
        Some(Rect {
            x0: div(self.x_offset, xr),
            y0: div(self.y_offset, yr),
            x1: div(self.x_size, xr),
            y1: div(self.y_size, yr),
        })
    }

    /// The image area on the reference grid at a resolution reduction:
    /// `Xsiz - XOsiz` by `Ysiz - YOsiz` at reduction 0, each bound scaled by
    /// `ceil(x / 2^r)` *before* the difference for every dropped level (ISO
    /// B.5's resolution coordinates).
    ///
    /// A reduction only ever reaches 32 — one per possible decomposition
    /// level (Table A-15) — and the clamp is exact even past it: for `u32`
    /// bounds, `ceil(x / 2^r)` is identical for every `r >= 32`.
    pub fn image_extent_at(&self, reduction: u8) -> (u32, u32) {
        let scale = 1u64 << reduction.min(32);
        let reduce = |size: u32, offset: u32| {
            (u64::from(size).div_ceil(scale)).saturating_sub(u64::from(offset).div_ceil(scale))
                as u32
        };
        (
            reduce(self.x_size, self.x_offset),
            reduce(self.y_size, self.y_offset),
        )
    }

    /// Component `index`'s own sample-grid dimensions (ISO/IEC 15444-1 §B.2):
    ///
    /// ```text
    /// width  = ceil(Xsiz / XRsiz) - ceil(XOsiz / XRsiz)
    /// height = ceil(Ysiz / YRsiz) - ceil(YOsiz / YRsiz)
    /// ```
    ///
    /// which reduces to [`image_extent_at`](Self::image_extent_at)`(0)` under
    /// unit sub-sampling.
    ///
    /// Returns `None` if `index` names no component, or if that component
    /// declares a zero sub-sampling factor. `decode_siz` rejects a zero factor,
    /// so the second case is unreachable for any `Siz` the parser produced —
    /// but the division is guarded here rather than by a `debug_assert` that
    /// would compile out and leave a release-mode panic behind.
    pub fn component_extent(&self, index: usize) -> Option<(u32, u32)> {
        self.component_extent_at(index, 0)
    }

    /// [`component_extent`](Self::component_extent) at a resolution reduction:
    /// each dropped level halves both component axes, rounding up — the same
    /// bound-then-difference rule as [`image_extent_at`](Self::image_extent_at),
    /// applied on the component's own grid.
    pub fn component_extent_at(&self, index: usize, reduction: u8) -> Option<(u32, u32)> {
        let comp = self.components.get(index)?;
        let (xr, yr) = (comp.x_sampling as u64, comp.y_sampling as u64);
        if xr == 0 || yr == 0 {
            return None;
        }
        let scale = 1u64 << reduction.min(32);
        let reduce = |size: u32, offset: u32, sampling: u64| {
            let hi = (u64::from(size)).div_ceil(sampling).div_ceil(scale);
            let lo = (u64::from(offset)).div_ceil(sampling).div_ceil(scale);
            hi.saturating_sub(lo) as u32
        };
        Some((
            reduce(self.x_size, self.x_offset, xr),
            reduce(self.y_size, self.y_offset, yr),
        ))
    }
}

/// The coding parameters that a component can override: `SPcod`'s tail, which
/// `SPcoc` repeats verbatim (A.6.1, A.6.2). OpenJPEG's `opj_tccp_t`.
///
/// `COD` supplies the default for every component and `COC` replaces it for one,
/// so these fields live here rather than on [`Cod`] — the fields that stay on
/// `Cod` are the ones no `COC` can touch (progression, layer count, the colour
/// transform, and the SOP/EPH flags, all of which are properties of the
/// codestream rather than of a component).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coding {
    pub decomposition_levels: u8,
    pub code_block_width: u8,  // exponent: width  = 2^(value + 2)
    pub code_block_height: u8, // exponent: height = 2^(value + 2)
    pub code_block_style: u8,  // bit flags: bypass, reset, restart, vcausal, segsym, …
    pub transform: Transform,
    /// `(PPx, PPy)` per resolution, coarsest first, as read from `SPcod`/`SPcoc`.
    /// Empty when the partition is maximal, which is the same as `(15, 15)`
    /// everywhere — [`precinct`](Self::precinct) hides the difference.
    pub precinct_sizes: Vec<(u8, u8)>,
}

impl Coding {
    /// The precinct exponents `(PPx, PPy)` at resolution `r`, on **that
    /// resolution's** grid (ISO/IEC 15444-1 B.6).
    ///
    /// A maximal partition — `Scod` bit 0 clear — is `(15, 15)` at every
    /// resolution, and 2^15 exceeds any tile-component the sample budget admits,
    /// so it yields the single precinct the pre-#61 decoder assumed. That makes
    /// the implicit case a special case of the explicit one rather than a
    /// separate path.
    pub fn precinct(&self, r: usize) -> (u32, u32) {
        self.precinct_sizes
            .get(r)
            .map_or((15, 15), |&(ppx, ppy)| (u32::from(ppx), u32::from(ppy)))
    }
}

/// A component's effective coding and quantization parameters, after `COC` has
/// been laid over `COD` and `QCC` over `QCD` (A.6.2, A.6.5). Resolved once at
/// parse time so no later stage has to remember which marker won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentParams {
    pub coding: Coding,
    pub quant: Qcd,
    /// `SPrgn`: the maxshift a region-of-interest coding applied to this
    /// component (A.6.3, Annex H). Zero when no `RGN` names the component,
    /// which is the ordinary case.
    ///
    /// Under maxshift the encoder lifted every coefficient of the region above
    /// every background coefficient by shifting it left this far. Nothing in the
    /// codestream says where the region *is*: the shift itself is the label, and
    /// undoing it is what separates the two sets again.
    pub roi_shift: u8,
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

impl Cod {
    /// COD's coding parameters, which every component takes unless a COC
    /// replaces them.
    pub fn coding(&self) -> Coding {
        Coding {
            decomposition_levels: self.decomposition_levels,
            code_block_width: self.code_block_width,
            code_block_height: self.code_block_height,
            code_block_style: self.code_block_style,
            transform: self.transform,
            precinct_sizes: self.precinct_sizes.clone(),
        }
    }
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
