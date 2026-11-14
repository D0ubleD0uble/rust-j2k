//! The three EBCOT coding passes and their context formation
//! (ISO/IEC 15444-1 Annex D).
//!
//! A code-block is decoded bit-plane by bit-plane, MSB→LSB. Each plane runs, in
//! order, the passes that apply: significance propagation (D.3.1), magnitude
//! refinement (D.3.3), and cleanup (D.3.4) with its run-length mode. Contexts
//! come from the significance state of the 3×3 neighborhood (D.3.* tables),
//! and sign coding uses the neighbor-sign context (D.3.2). The cleanup pass
//! of the top plane runs first.
//!
//! The optional code-block style flags act at three points. `segmentation
//! symbols` appends a four-symbol marker to every cleanup pass (D.5).
//! `vertically causal context` narrows the significance context so a stripe's
//! bottom row ignores the row below it, and `reset context probabilities`
//! reinitializes the MQ context model after every pass — both change which
//! context a decision is coded under, threaded through [`BlockState`] and
//! [`decode_block`]. `restart` and `predictable termination` modulate
//! termination. `bypass` (lazy) codes the significance and refinement passes of
//! the low bit-planes as raw bits rather than MQ decisions (D.6); the raw reader
//! and the per-segment coder choice live in [`decode_block`].

use crate::tier1::mq::{Context, MqDecoder, RawDecoder};
use crate::{Error, Result};

#[cfg(test)]
#[path = "golden_vectors.rs"]
mod golden_vectors;

/// MQ context indices, laid out as ISO/IEC 15444-1 Table D-7 allocates them and
/// OpenJPEG's `T1_CTXNO_*` constants order them. A code-block's decoder owns one
/// [`Context`] per index (`NUM_CONTEXTS` total); the
/// context-formation routines below return an index into that array.
///
/// - `0..=8`   zero coding (the nine significance contexts, [`zc_table`])
/// - `9..=13`  sign coding ([`sign_context`])
/// - `14..=16` magnitude refinement ([`refinement_context`])
/// - `17`      run-length (the cleanup pass's aggregation context)
/// - `18`      uniform (equiprobable; raw bits in the cleanup run mode)
pub const CTX_ZC: u8 = 0;
pub const CTX_SC: u8 = 9;
pub const CTX_MR: u8 = 14;
pub const CTX_RUN: u8 = 17;
pub const CTX_UNI: u8 = 18;
/// Number of MQ contexts a code-block tracks (ISO Table D-7).
pub const NUM_CONTEXTS: usize = 19;

/// Largest coded bit-plane the double-scale reconstruction can hold: decoding
/// carries each magnitude at twice its weight (the mid-point half), so the top
/// plane's `1 << top` and the accumulated value must stay inside `i32`. At
/// `top == 30` the maximum magnitude `2^31 − 1` just fits; `top == 31` overflows
/// (this mirrors OpenJPEG rejecting `bpno_plus_one >= 31`). Callers reject any
/// subband whose `Mb − zero_bit_planes` exceeds this.
pub const MAX_BIT_PLANES: u32 = 30;

/// Subband orientation, which selects the zero-coding context table (D.3.1).
/// Per Table D-1, `LL` and `LH` share one table, `HL` swaps the horizontal and
/// vertical neighbor roles, and `HH` keys off the diagonal count instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Ll,
    Lh,
    Hl,
    Hh,
}

// Per-sample state, one `u16` per sample in `BlockState::state`.
//
// The low byte says which of the sample's eight neighbors are significant.
// Marking a sample significant sets its bit in each neighbor's word.
const NB_N: u16 = 1 << 0;
const NB_S: u16 = 1 << 1;
const NB_W: u16 = 1 << 2;
const NB_E: u16 = 1 << 3;
const NB_NW: u16 = 1 << 4;
const NB_NE: u16 = 1 << 5;
const NB_SW: u16 = 1 << 6;
const NB_SE: u16 = 1 << 7;
/// All eight neighbor bits.
const NB_ALL: u16 = 0xFF;
/// The three neighbors in the row below, which vertically causal coding drops
/// from a stripe's bottom row.
const NB_SOUTH: u16 = NB_S | NB_SW | NB_SE;
/// σ: the coefficient has become significant (a 1 bit has been coded).
const SIG: u16 = 1 << 8;
/// The coefficient was coded in the current bit-plane's significance
/// propagation pass, so the cleanup pass skips it.
const VISITED: u16 = 1 << 9;
/// Sign bit: set is negative. Meaningful only once significant.
const NEGATIVE: u16 = 1 << 10;
/// The coefficient has been through at least one magnitude-refinement pass,
/// which selects magnitude-refinement context 16 over 14/15 (D.3.3).
const REFINED: u16 = 1 << 11;
// The top four bits record the sign of the four direct neighbors, set with
// the neighbor's `NB_*` bit when that neighbor is negative. With the four
// direct `NB_*` bits they index the sign-context table.
const NEG_N: u16 = 1 << 12;
const NEG_S: u16 = 1 << 13;
const NEG_W: u16 = 1 << 14;
const NEG_E: u16 = 1 << 15;

/// One per-sample bit repeated for each of the four rows of a stripe column,
/// as [`column_word`] packs them.
const fn each_row(bits: u16) -> u64 {
    (bits as u64) * 0x0001_0001_0001_0001
}

/// The bits of a column word that hold the neighbor masks the contexts see:
/// all eight neighbors on every row, except the south ones on the bottom row
/// under vertically causal coding.
const fn column_neighbors(vcausal: bool) -> u64 {
    if vcausal {
        each_row(NB_ALL) & !((NB_SOUTH as u64) << 48)
    } else {
        each_row(NB_ALL)
    }
}

/// A code-block's decode state: the coefficient magnitudes plus the per-sample
/// state the passes consult and update.
///
/// `coeffs` is row-major, `width × height`. `state` holds one `u16` per sample
/// (the `NB_*`, [`SIG`], [`VISITED`], [`NEGATIVE`], [`REFINED`] and `NEG_*`
/// bits) in stripe order: the four rows of a stripe column are adjacent, so
/// [`column_word`] reads a whole column at once, and the stripes follow each
/// other. The plane is padded by one column on each side and one stripe above
/// and below, so the eight neighbors of any in-block sample are in range and
/// the padding reads as insignificant (D.3: out-of-block samples contribute
/// 0). The rows of a short last stripe past the block's height are never
/// coded, so they read as insignificant too. No context needs an edge test.
#[derive(Debug)]
pub struct BlockState {
    pub width: u32,
    pub height: u32,
    pub coeffs: Vec<i32>,
    /// Length of one padded stripe in `state`, `4 × (width + 2)`.
    stripe_len: usize,
    /// Per-sample state words, padded, in stripe order.
    state: Vec<u16>,
    /// Vertically causal context (the `VCAUSAL` style): the context of a sample
    /// on a stripe's bottom row (`y % 4 == 3`) excludes the row below it, which
    /// belongs to the next stripe. `false` for the default style, where the full
    /// 3×3 neighborhood is used. Set by [`decode_block`] from the style byte.
    vcausal: bool,
}

impl BlockState {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        let stripe_len = 4 * (width as usize + 2);
        let padded = stripe_len * (height.div_ceil(4) as usize + 2);
        BlockState {
            width,
            height,
            coeffs: vec![0; n],
            stripe_len,
            state: vec![0; padded],
            vcausal: false,
        }
    }

    /// Flat row-major index of an in-bounds coefficient in `coeffs`.
    fn idx(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }

    /// Index of an in-bounds sample in the padded `state`.
    #[inline]
    fn pad(&self, x: u32, y: u32) -> usize {
        (y as usize / 4 + 1) * self.stripe_len + (x as usize + 1) * 4 + y as usize % 4
    }
}

/// The four state words of the stripe column that starts at padded index
/// `p`, row 0 in the low 16 bits. One test of this word tells whether any
/// sample of the column needs a pass's attention.
#[inline(always)]
fn column_word(state: &[u16], p: usize) -> u64 {
    let c = &state[p..p + 4];
    u64::from(c[0]) | u64::from(c[1]) << 16 | u64::from(c[2]) << 32 | u64::from(c[3]) << 48
}

/// Padded index of the north neighbor of the sample at `p` on stripe row `r`:
/// the row above in the same stripe, or row 3 of the stripe above.
#[inline(always)]
fn north(p: usize, r: usize, stripe_len: usize) -> usize {
    if r == 0 { p - stripe_len + 3 } else { p - 1 }
}

/// Padded index of the south neighbor of the sample at `p` on stripe row `r`:
/// the row below in the same stripe, or row 0 of the stripe below.
#[inline(always)]
fn south(p: usize, r: usize, stripe_len: usize) -> usize {
    if r == 3 { p + stripe_len - 3 } else { p + 1 }
}

/// Mark the sample at padded index `p`, stripe row `r`, significant with the
/// given sign, and record it in the word of each of its eight neighbors, as
/// the direction that neighbor sees it in. The four direct neighbors also
/// record its sign. The padding absorbs the writes past the block edge.
#[inline(always)]
fn mark_significant(state: &mut [u16], p: usize, r: usize, stripe_len: usize, negative: bool) {
    let n = north(p, r, stripe_len);
    let s = south(p, r, stripe_len);
    let sign = |bit: u16| if negative { bit } else { 0 };
    state[p] |= SIG | sign(NEGATIVE);
    state[n - 4] |= NB_SE;
    state[n] |= NB_S | sign(NEG_S);
    state[n + 4] |= NB_SW;
    state[p - 4] |= NB_E | sign(NEG_E);
    state[p + 4] |= NB_W | sign(NEG_W);
    state[s - 4] |= NB_NE;
    state[s] |= NB_N | sign(NEG_N);
    state[s + 4] |= NB_NW;
}

/// The state word as the contexts see it: under vertically causal coding a
/// stripe's bottom row (`hide_south`) loses its three south significance
/// bits. Its `NEG_S` bit can stay, since the sign context counts a sign bit
/// only with its significance bit.
#[inline(always)]
fn visible(word: u16, hide_south: bool) -> u16 {
    if hide_south { word & !NB_SOUTH } else { word }
}

/// The significant-neighbor mask of a visible state word. Zero means no
/// significant neighbor, which is zero-coding label 0 in every orientation.
#[inline(always)]
fn neighbor_mask(word: u16) -> u8 {
    (word & NB_ALL) as u8
}

/// Sign-coding context (`CTX_SC..CTX_MR`) and the XOR bit (ISO Tables D-2,
/// D-3) for a visible state word. The MQ decision is XORed with the returned
/// bit to recover the sign (`0` positive, `1` negative).
#[inline(always)]
fn sign_context(word: u16) -> (u8, u8) {
    let key = (word & (NB_N | NB_S | NB_W | NB_E)) | ((word >> 8) & 0xF0);
    let entry = SIGN_TABLE[usize::from(key as u8)];
    (entry & 0x7F, entry >> 7)
}

/// Magnitude-refinement context (`CTX_MR..CTX_RUN`) for a visible state word
/// (ISO Table D-4 / D.3.3). After the first refinement the context is always
/// 16; on the first refinement it is 15 if any neighbor in its context is
/// significant, else 14.
#[inline(always)]
fn refinement_context(word: u16) -> u8 {
    if word & REFINED != 0 {
        CTX_MR + 2
    } else {
        CTX_MR + u8::from(neighbor_mask(word) != 0)
    }
}

/// Coordinate-based accessors over the padded plane, for the unit tests. They
/// form contexts with the same helpers the passes call.
#[cfg(test)]
impl BlockState {
    /// Whether the row below `(·, y)` lies in the next stripe and so is excluded
    /// from the context under vertically causal coding: it is the stripe's bottom
    /// row (`y % 4 == 3`) and the flag is set. Off, or on an interior row, the
    /// full neighborhood is used (D.3). The passes make the same choice per
    /// stripe row.
    fn excludes_south(&self, y: u32) -> bool {
        self.vcausal && y % 4 == 3
    }

    /// The state word of `(x, y)` as its contexts see it.
    fn word(&self, x: u32, y: u32) -> u16 {
        visible(self.state[self.pad(x, y)], self.excludes_south(y))
    }

    pub fn is_significant(&self, x: u32, y: u32) -> bool {
        self.state[self.pad(x, y)] & SIG != 0
    }

    pub fn set_significant(&mut self, x: u32, y: u32) {
        let p = self.pad(x, y);
        mark_significant(&mut self.state, p, y as usize % 4, self.stripe_len, false);
    }

    pub fn is_visited(&self, x: u32, y: u32) -> bool {
        self.state[self.pad(x, y)] & VISITED != 0
    }

    pub fn set_visited(&mut self, x: u32, y: u32, value: bool) {
        let p = self.pad(x, y);
        if value {
            self.state[p] |= VISITED;
        } else {
            self.state[p] &= !VISITED;
        }
    }

    pub fn is_negative(&self, x: u32, y: u32) -> bool {
        self.state[self.pad(x, y)] & NEGATIVE != 0
    }

    /// Set or clear the sign of `(x, y)` and the sign bit its four direct
    /// neighbors keep for it, as [`mark_significant`] would have.
    pub fn set_negative(&mut self, x: u32, y: u32, value: bool) {
        let p = self.pad(x, y);
        let (sl, r) = (self.stripe_len, y as usize % 4);
        let bits = [
            (p, NEGATIVE),
            (north(p, r, sl), NEG_S),
            (south(p, r, sl), NEG_N),
            (p - 4, NEG_E),
            (p + 4, NEG_W),
        ];
        for (i, bit) in bits {
            if value {
                self.state[i] |= bit;
            } else {
                self.state[i] &= !bit;
            }
        }
    }

    pub fn is_refined(&self, x: u32, y: u32) -> bool {
        self.state[self.pad(x, y)] & REFINED != 0
    }

    pub fn set_refined(&mut self, x: u32, y: u32) {
        let p = self.pad(x, y);
        self.state[p] |= REFINED;
    }

    /// (ΣH, ΣV, ΣD) of `(x, y)` as its contexts see them, south excluded on a
    /// stripe's bottom row under vertically causal coding.
    pub fn neighbor_sums(&self, x: u32, y: u32) -> (u8, u8, u8) {
        neighbor_sums(neighbor_mask(self.word(x, y)))
    }

    /// Zero-coding context label (`CTX_ZC..CTX_SC`) for `(x, y)` in a subband of
    /// the given orientation (ISO Table D-1).
    pub fn zc_context(&self, x: u32, y: u32, orient: Orientation) -> u8 {
        CTX_ZC + zc_table(orient)[usize::from(neighbor_mask(self.word(x, y)))]
    }

    /// Sign-coding context and XOR bit for `(x, y)`; see [`sign_context`].
    pub fn sc_context(&self, x: u32, y: u32) -> (u8, u8) {
        sign_context(self.word(x, y))
    }

    /// Magnitude-refinement context for `(x, y)`; see [`refinement_context`].
    pub fn mr_context(&self, x: u32, y: u32) -> u8 {
        refinement_context(self.word(x, y))
    }
}

/// Zero-coding label for the `LL`/`LH` table (ISO Table D-1). `HL` reuses this
/// with `h` and `v` exchanged by the caller.
const fn zc_label_lh(h: u8, v: u8, d: u8) -> u8 {
    match (h, v, d) {
        (2, _, _) => 8,
        (1, v, _) if v >= 1 => 7,
        (1, 0, d) if d >= 1 => 6,
        (1, 0, 0) => 5,
        (0, 2, _) => 4,
        (0, 1, _) => 3,
        (0, 0, d) if d >= 2 => 2,
        (0, 0, 1) => 1,
        _ => 0,
    }
}

/// Zero-coding label for the `HH` table (ISO Table D-1), keyed by the diagonal
/// count `d` and the combined horizontal+vertical count `hv`.
const fn zc_label_hh(hv: u8, d: u8) -> u8 {
    match (d, hv) {
        (d, _) if d >= 3 => 8,
        (2, hv) if hv >= 1 => 7,
        (2, _) => 6,
        (1, hv) if hv >= 2 => 5,
        (1, 1) => 4,
        (1, _) => 3,
        (0, hv) if hv >= 2 => 2,
        (0, 1) => 1,
        _ => 0,
    }
}

/// (ΣH, ΣV, ΣD) of a significant-neighbor mask: the significance sums of the
/// two horizontal, two vertical, and four diagonal neighbors, the inputs to
/// every D.3 table.
const fn neighbor_sums(mask: u8) -> (u8, u8, u8) {
    let mask = mask as u16;
    let h = (mask & NB_W != 0) as u8 + (mask & NB_E != 0) as u8;
    let v = (mask & NB_N != 0) as u8 + (mask & NB_S != 0) as u8;
    let d = (mask & (NB_NW | NB_NE | NB_SW | NB_SE)).count_ones() as u8;
    (h, v, d)
}

/// The zero-coding labels for every significant-neighbor mask, for one
/// orientation group (ISO Table D-1). `LL`/`LH` use the base assignment, `HL`
/// swaps the horizontal and vertical sums, and `HH` keys off the diagonal
/// count.
const fn build_zc_table(group: Orientation) -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut mask = 0;
    while mask < 256 {
        let (h, v, d) = neighbor_sums(mask as u8);
        table[mask] = match group {
            Orientation::Ll | Orientation::Lh => zc_label_lh(h, v, d),
            Orientation::Hl => zc_label_lh(v, h, d),
            Orientation::Hh => zc_label_hh(h + v, d),
        };
        mask += 1;
    }
    table
}

static ZC_LL_LH: [u8; 256] = build_zc_table(Orientation::Lh);
static ZC_HL: [u8; 256] = build_zc_table(Orientation::Hl);
static ZC_HH: [u8; 256] = build_zc_table(Orientation::Hh);

/// The zero-coding label table for a subband orientation. Index it with a
/// sample's [`neighbor_mask`] and add [`CTX_ZC`].
fn zc_table(orient: Orientation) -> &'static [u8; 256] {
    match orient {
        Orientation::Ll | Orientation::Lh => &ZC_LL_LH,
        Orientation::Hl => &ZC_HL,
        Orientation::Hh => &ZC_HH,
    }
}

/// The sign context of every combination of the four direct neighbors'
/// significance and sign (ISO Tables D-2, D-3). The key holds the `NB_N`,
/// `NB_S`, `NB_W`, `NB_E` bits in its low four bits and the matching `NEG_*`
/// bits in its high four, in the same order. An entry is the context in its
/// low seven bits and the XOR bit in its top bit. A sign bit counts only with
/// its significance bit.
const fn build_sign_table() -> [u8; 256] {
    // A neighbor's contribution (Table D-2): 0 if insignificant, -1 if
    // negative, +1 if positive.
    const fn contrib(key: usize, sig: u16) -> i32 {
        let neg = (sig as usize) << 4;
        if key & sig as usize == 0 {
            0
        } else if key & neg != 0 {
            -1
        } else {
            1
        }
    }
    const fn clamp(v: i32) -> i32 {
        if v > 1 {
            1
        } else if v < -1 {
            -1
        } else {
            v
        }
    }
    let mut table = [0u8; 256];
    let mut key = 0;
    while key < 256 {
        let h = clamp(contrib(key, NB_W) + contrib(key, NB_E));
        let v = clamp(contrib(key, NB_N) + contrib(key, NB_S));
        let (label, xor) = match (h, v) {
            (1, 1) => (4, 0),
            (1, 0) => (3, 0),
            (1, -1) => (2, 0),
            (0, 1) => (1, 0),
            (0, 0) => (0, 0),
            (0, -1) => (1, 1),
            (-1, 1) => (2, 1),
            (-1, 0) => (3, 1),
            _ => (4, 1),
        };
        table[key] = (CTX_SC + label) | (xor << 7);
        key += 1;
    }
    table
}

static SIGN_TABLE: [u8; 256] = build_sign_table();

/// Initial MQ states for a fresh code-block (ISO Table D-7 / Annex D.3): the
/// uniform context starts at state 46, the run-length context at 3, and the
/// all-insignificant zero-coding context at 4; every other context at 0. Using
/// the default zero state for these three would desynchronize the decoder.
pub(crate) fn init_contexts() -> [Context; NUM_CONTEXTS] {
    let mut cx = [Context::default(); NUM_CONTEXTS];
    cx[CTX_ZC as usize] = Context::new(4, 0);
    cx[CTX_RUN as usize] = Context::new(3, 0);
    cx[CTX_UNI as usize] = Context::new(46, 0);
    cx
}

/// The parts of a code-block's state a pass works on, split out of
/// [`BlockState`] so the pass loops hold them as separate borrows.
struct Plane<'s> {
    state: &'s mut [u16],
    coeffs: &'s mut [i32],
    width: usize,
    height: usize,
    stripe_len: usize,
}

impl<'s> Plane<'s> {
    fn new(block: &'s mut BlockState) -> Self {
        Plane {
            state: &mut block.state,
            coeffs: &mut block.coeffs,
            width: block.width as usize,
            height: block.height as usize,
            stripe_len: block.stripe_len,
        }
    }

    /// The number of stripes that have all four rows.
    fn full_stripes(&self) -> usize {
        self.height / 4
    }

    /// The rows of the short last stripe, `0..=3`.
    fn last_rows(&self) -> usize {
        self.height % 4
    }

    /// Padded index of row 0 of column `x` in stripe `k`.
    #[inline(always)]
    fn column(&self, k: usize, x: usize) -> usize {
        (k + 1) * self.stripe_len + (x + 1) * 4
    }

    /// Set a coefficient's magnitude when it first becomes significant at plane
    /// `bpno`, to the mid-point reconstruction value `2^bpno + 2^(bpno−1)`
    /// (ISO E.1.1.2 / OpenJPEG `oneplushalf`): the significant bit plus half the
    /// next plane's weight, placing it at the center of its interval. Then mark
    /// it significant with its sign.
    #[inline(always)]
    fn become_significant(&mut self, i: usize, p: usize, r: usize, bpno: u32, negative: bool) {
        let one = 1i32 << bpno;
        self.coeffs[i] = one | (one >> 1);
        mark_significant(self.state, p, r, self.stripe_len, negative);
    }

    /// Refine a significant coefficient's magnitude at plane `bpno`: a `1` bit
    /// pushes it up by half the plane weight, a `0` bit down (OpenJPEG `poshalf`,
    /// the running mid-point update). Decoding runs at double scale and stops at
    /// `bpno == 1`, so `poshalf` is always ≥ 1; the carried half is dropped by
    /// the final halving in [`decode_block`].
    #[inline(always)]
    fn refine_magnitude(&mut self, i: usize, p: usize, bpno: u32, bit: u8) {
        let poshalf = (1i32 << bpno) >> 1;
        self.coeffs[i] += if bit == 1 { poshalf } else { -poshalf };
        self.state[p] |= REFINED;
    }
}

/// Decode the sign of the coefficient whose visible state word is `word` (D.3.2):
/// the MQ decision under the neighbor-sign context is XORed with the
/// context's predicted sign. Returns whether it is negative.
#[inline(always)]
fn decode_sign(mq: &mut MqDecoder<'_>, cx: &mut [Context; NUM_CONTEXTS], word: u16) -> bool {
    let (ctx, xor) = sign_context(word);
    mq.decode(&mut cx[usize::from(ctx)]) ^ xor == 1
}

/// Zero-code one sample in the significance-propagation or cleanup pass, and on
/// a 1 decode its sign and mark it significant. `word` is its visible state
/// word; `i` its coefficient index, `p` its padded index, `r` its stripe row.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn zero_code(
    mq: &mut MqDecoder<'_>,
    cx: &mut [Context; NUM_CONTEXTS],
    plane: &mut Plane<'_>,
    table: &[u8; 256],
    word: u16,
    i: usize,
    p: usize,
    r: usize,
    bpno: u32,
) {
    let ctx = CTX_ZC + table[usize::from(neighbor_mask(word))];
    if mq.decode(&mut cx[usize::from(ctx)]) == 1 {
        let negative = decode_sign(mq, cx, word);
        plane.become_significant(i, p, r, bpno, negative);
    }
}

/// Run `$body` once for each row of a full stripe column, with `$r` bound to
/// the row, `0` to `3`. Spelled out so each copy sees its row as a constant.
macro_rules! each_stripe_row {
    ($r:ident => $body:expr) => {{
        {
            let $r: usize = 0;
            $body;
        }
        {
            let $r: usize = 1;
            $body;
        }
        {
            let $r: usize = 2;
            $body;
        }
        {
            let $r: usize = 3;
            $body;
        }
    }};
}

/// One sample of the significance-propagation pass: zero-code it if it is
/// insignificant with a significant neighbor in its context, and mark it
/// visited. `hide` says vertically causal coding hides its south neighbors.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn sig_prop_sample(
    mq: &mut MqDecoder<'_>,
    cx: &mut [Context; NUM_CONTEXTS],
    plane: &mut Plane<'_>,
    table: &[u8; 256],
    i: usize,
    p: usize,
    r: usize,
    hide: bool,
    bpno: u32,
) {
    let word = plane.state[p];
    if word & SIG != 0 {
        return;
    }
    let word = visible(word, hide);
    if neighbor_mask(word) == 0 {
        return; // no significant neighbor: deferred to cleanup
    }
    zero_code(mq, cx, plane, table, word, i, p, r, bpno);
    plane.state[p] |= VISITED;
}

/// One sample of the magnitude-refinement pass: refine it if it was
/// significant before this plane.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn mag_ref_sample(
    mq: &mut MqDecoder<'_>,
    cx: &mut [Context; NUM_CONTEXTS],
    plane: &mut Plane<'_>,
    i: usize,
    p: usize,
    hide: bool,
    bpno: u32,
) {
    let word = plane.state[p];
    if word & (SIG | VISITED) != SIG {
        return; // insignificant, or coded in this plane's first pass
    }
    let ctx = refinement_context(visible(word, hide));
    let bit = mq.decode(&mut cx[usize::from(ctx)]);
    plane.refine_magnitude(i, p, bpno, bit);
}

/// One sample of the cleanup pass outside a run: zero-code it if this plane
/// has not coded it yet, and clear its `VISITED` mark.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn cleanup_sample(
    mq: &mut MqDecoder<'_>,
    cx: &mut [Context; NUM_CONTEXTS],
    plane: &mut Plane<'_>,
    table: &[u8; 256],
    i: usize,
    p: usize,
    r: usize,
    hide: bool,
    bpno: u32,
) {
    let word = plane.state[p];
    if word & (SIG | VISITED) != 0 {
        plane.state[p] = word & !VISITED;
        return;
    }
    zero_code(mq, cx, plane, table, visible(word, hide), i, p, r, bpno);
}

/// Significance-propagation pass (D.3.1): visit each still-insignificant sample
/// that has at least one significant neighbor, zero-code it, and on a 1 decode
/// its sign. `bpno` is the current bit-plane; `1 << bpno` is its weight.
fn sig_prop_pass(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context; NUM_CONTEXTS],
    orient: Orientation,
    bpno: u32,
) {
    if state.vcausal {
        sig_prop_pass_impl::<true>(mq, state, cx, orient, bpno);
    } else {
        sig_prop_pass_impl::<false>(mq, state, cx, orient, bpno);
    }
}

/// [`sig_prop_pass`] for one setting of vertically causal coding, `VC`. A
/// column whose samples have no significant neighbor is skipped with one test.
#[inline(never)]
fn sig_prop_pass_impl<const VC: bool>(
    mq_out: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context; NUM_CONTEXTS],
    orient: Orientation,
    bpno: u32,
) {
    // The decoder's registers live in locals for the whole pass.
    let mut mq = mq_out.clone();
    let table = zc_table(orient);
    let mut plane = Plane::new(state);
    let w = plane.width;
    for k in 0..plane.full_stripes() {
        for x in 0..w {
            let p = plane.column(k, x);
            if column_word(plane.state, p) & column_neighbors(VC) == 0 {
                continue; // no sample of the column has a significant neighbor
            }
            let i = 4 * k * w + x;
            each_stripe_row!(r => sig_prop_sample(
                &mut mq, cx, &mut plane, table, i + r * w, p + r, r, VC && r == 3, bpno
            ));
        }
    }
    let (k, rows) = (plane.full_stripes(), plane.last_rows());
    for x in 0..w {
        let p = plane.column(k, x);
        let i = 4 * k * w + x;
        for r in 0..rows {
            sig_prop_sample(
                &mut mq,
                cx,
                &mut plane,
                table,
                i + r * w,
                p + r,
                r,
                false,
                bpno,
            );
        }
    }
    *mq_out = mq;
}

/// Magnitude-refinement pass (D.3.3): refine every sample that was already
/// significant before this plane (so not coded in this plane's significance
/// pass) by decoding one more magnitude bit under the refinement context.
fn mag_ref_pass(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context; NUM_CONTEXTS],
    bpno: u32,
) {
    if state.vcausal {
        mag_ref_pass_impl::<true>(mq, state, cx, bpno);
    } else {
        mag_ref_pass_impl::<false>(mq, state, cx, bpno);
    }
}

/// [`mag_ref_pass`] for one setting of vertically causal coding, `VC`. A column
/// with no significant sample is skipped with one test.
#[inline(never)]
fn mag_ref_pass_impl<const VC: bool>(
    mq_out: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context; NUM_CONTEXTS],
    bpno: u32,
) {
    let mut mq = mq_out.clone();
    let mut plane = Plane::new(state);
    let w = plane.width;
    for k in 0..plane.full_stripes() {
        for x in 0..w {
            let p = plane.column(k, x);
            if column_word(plane.state, p) & each_row(SIG) == 0 {
                continue; // no significant sample in the column
            }
            let i = 4 * k * w + x;
            each_stripe_row!(r => mag_ref_sample(
                &mut mq, cx, &mut plane, i + r * w, p + r, VC && r == 3, bpno
            ));
        }
    }
    let (k, rows) = (plane.full_stripes(), plane.last_rows());
    for x in 0..w {
        let p = plane.column(k, x);
        let i = 4 * k * w + x;
        for r in 0..rows {
            mag_ref_sample(&mut mq, cx, &mut plane, i + r * w, p + r, false, bpno);
        }
    }
    *mq_out = mq;
}

/// Visit every sample in EBCOT scan order (stripes of four rows top to bottom,
/// each stripe column by column; the bottom stripe may be shorter) and call
/// `f` with its coefficient index, padded index and stripe row, and whether
/// vertically causal coding hides its south neighbors.
#[inline(always)]
fn for_each_sample(
    state: &mut BlockState,
    mut f: impl FnMut(&mut Plane<'_>, usize, usize, usize, bool),
) {
    let vcausal = state.vcausal;
    let mut plane = Plane::new(state);
    let w = plane.width;
    let stripes = plane.height.div_ceil(4);
    for k in 0..stripes {
        let rows = (plane.height - 4 * k).min(4);
        for x in 0..w {
            let p = plane.column(k, x);
            let i = 4 * k * w + x;
            for r in 0..rows {
                f(&mut plane, i + r * w, p + r, r, vcausal && r == 3);
            }
        }
    }
}

/// Significance-propagation pass under `bypass` (ISO D.6): the same scan and the
/// same sample selection as [`sig_prop_pass`], but the significance decision and
/// the sign are read as raw bits instead of MQ-coded ones. The neighbor mask
/// still decides *which* samples are visited (a non-zero mask means a
/// significant neighbor), it just no longer selects a context — so the sign is
/// the raw bit itself, without the neighbor-sign context or its XOR.
fn sig_prop_pass_raw(raw: &mut RawDecoder<'_>, state: &mut BlockState, bpno: u32) {
    for_each_sample(state, |plane, i, p, r, hide| {
        let word = plane.state[p];
        if word & SIG != 0 {
            return;
        }
        if neighbor_mask(visible(word, hide)) == 0 {
            return; // no significant neighbor: deferred to cleanup
        }
        if raw.decode() == 1 {
            let negative = raw.decode() == 1;
            plane.become_significant(i, p, r, bpno, negative);
        }
        plane.state[p] |= VISITED;
    });
}

/// Magnitude-refinement pass under `bypass` (ISO D.6): identical to
/// [`mag_ref_pass`] except the refinement bit is a raw bit rather than an
/// MQ-coded decision under the refinement context.
fn mag_ref_pass_raw(raw: &mut RawDecoder<'_>, state: &mut BlockState, bpno: u32) {
    for_each_sample(state, |plane, i, p, _, _| {
        if plane.state[p] & (SIG | VISITED) != SIG {
            return;
        }
        let bit = raw.decode();
        plane.refine_magnitude(i, p, bpno, bit);
    });
}

/// Cleanup pass (D.3.4): code every sample not yet handled this plane. A full
/// column of four insignificant samples with no significant neighbors is
/// aggregated through the run-length context — a 0 leaves all four
/// insignificant, a 1 reads the first significant sample's position as two
/// uniform bits — after which the column finishes with ordinary zero coding.
/// The pass also clears the plane's `VISITED` marks, column by column.
///
/// Under `segmentation symbols` the pass ends by reading the `1010` symbol
/// (D.5); see [`read_segmentation_symbol`], which says what `terminated`
/// changes.
fn cleanup_pass(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context; NUM_CONTEXTS],
    orient: Orientation,
    bpno: u32,
    style: u8,
    terminated: bool,
) -> Result<()> {
    if state.vcausal {
        cleanup_pass_impl::<true>(mq, state, cx, orient, bpno);
    } else {
        cleanup_pass_impl::<false>(mq, state, cx, orient, bpno);
    }
    if style & crate::codestream::markers::code_block_style::SEGSYM != 0 {
        read_segmentation_symbol(mq, cx, terminated)?;
    }
    Ok(())
}

/// The scan of [`cleanup_pass`] for one setting of vertically causal coding,
/// `VC`. The run-length test of a full column is one test of its column word.
#[inline(never)]
fn cleanup_pass_impl<const VC: bool>(
    mq_out: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context; NUM_CONTEXTS],
    orient: Orientation,
    bpno: u32,
) {
    let mut mq = mq_out.clone();
    let table = zc_table(orient);
    let mut plane = Plane::new(state);
    let w = plane.width;
    // A column aggregates when no sample is significant or visited and none
    // has a significant neighbor in its context. The `SIG` term is implied by
    // the neighbor bits (a significant sample sets `NB_N` or `NB_S` in the
    // sample beside it in the column, which vertically causal coding never
    // hides), but it states the rule as D.3.4 does.
    let run_bits = each_row(SIG | VISITED) | column_neighbors(VC);
    for k in 0..plane.full_stripes() {
        for x in 0..w {
            let p = plane.column(k, x);
            let i = 4 * k * w + x;
            let mut first = 0;
            if column_word(plane.state, p) & run_bits == 0 {
                if mq.decode(&mut cx[CTX_RUN as usize]) == 0 {
                    continue; // run of four insignificant samples
                }
                let hi = mq.decode(&mut cx[CTX_UNI as usize]);
                let lo = mq.decode(&mut cx[CTX_UNI as usize]);
                let run = usize::from((hi << 1) | lo); // first significant
                let word = visible(plane.state[p + run], VC && run == 3);
                let negative = decode_sign(&mut mq, cx, word);
                plane.become_significant(i + run * w, p + run, run, bpno, negative);
                first = run + 1;
            }
            each_stripe_row!(r => if r >= first {
                cleanup_sample(
                    &mut mq, cx, &mut plane, table, i + r * w, p + r, r, VC && r == 3, bpno,
                )
            });
        }
    }
    let (k, rows) = (plane.full_stripes(), plane.last_rows());
    for x in 0..w {
        let p = plane.column(k, x);
        let i = 4 * k * w + x;
        for r in 0..rows {
            cleanup_sample(
                &mut mq,
                cx,
                &mut plane,
                table,
                i + r * w,
                p + r,
                r,
                false,
                bpno,
            );
        }
    }
    *mq_out = mq;
}

/// Read the segmentation symbol that closes a cleanup pass under `segmentation
/// symbols` (ISO D.5): four decisions in the uniform context, most significant
/// first, spelling `1010`.
///
/// The four decisions are part of the codeword whether or not anyone looks at
/// the value, so a decoder that skips them desynchronizes the MQ coder for every
/// later pass. That is why this runs before the value is judged.
///
/// D.5 makes the symbol an error-detection aid, not a requirement on the
/// decoder, and a wrong value is not always corruption. Rate control can cut a
/// pass that the encoder did not terminate, and its length is then the
/// encoder's estimate: `opj_compress` uses the bytes written so far plus three
/// (t1.c), which can be a few bytes short. The MQ decoder then reads the
/// symbol from the `0xFF` fill past the segment and gets a wrong value. So on
/// such a pass, once the decoder has run past the segment's bytes
/// ([`MqDecoder::ran_past_end`]), a wrong value decodes. OpenJPEG decodes every
/// wrong value, because its comparison is commented out.
///
/// A wrong value is an error in two cases. Before the decoder runs past the
/// end, the symbol depends only on the segment's bytes, so they are corrupt.
/// And on a `terminated` pass (see [`decode_block`]) the length is exact, not
/// estimated, and the MQ termination makes the codeword decode correctly from
/// the fill, so a wrong value is corrupt data there too.
fn read_segmentation_symbol(
    mq: &mut MqDecoder<'_>,
    cx: &mut [Context],
    terminated: bool,
) -> Result<()> {
    let mut symbol = 0u8;
    for _ in 0..4 {
        symbol = (symbol << 1) | mq.decode(&mut cx[CTX_UNI as usize]);
    }
    if symbol != 0b1010 && (terminated || !mq.ran_past_end()) {
        return Err(Error::Codestream(format!(
            "segmentation symbol at the end of a cleanup pass is {symbol:#06b}, expected 0b1010; \
             the coded data is corrupt"
        )));
    }
    Ok(())
}

/// Undo the maxshift on one coefficient magnitude (ISO/IEC 15444-1 H.2).
///
/// `mag` is the double-scale magnitude Tier-1 reconstructed, the value OpenJPEG
/// holds in `datap` before `opj_t1_clbl_decode_processor` halves it. A magnitude
/// **at or above** `2^roi_shift` was lifted there by the encoder and belongs to
/// the region; everything strictly below is background, already on the common
/// scale. Nothing else identifies the region — the shift is the label, which is
/// why the boundary is `>=` and not `>`: a coefficient of exactly `2^roi_shift`
/// is the smallest the encoder can have lifted.
///
/// Transliterated from `t1.c`:
///
/// ```text
/// if (roishift >= 31) { datap[...] = 0; }
/// else { thresh = 1 << roishift; if (mag >= thresh) { mag >>= roishift; } }
/// ```
///
/// The `>= 31` arm is OpenJPEG's guard for a shift `1i32` cannot express, and
/// it is reachable: `SPrgn` is a free byte, and a block whose zero bit-planes
/// exceed `Mb` keeps [`top_coded_plane`] under the [`MAX_BIT_PLANES`] check
/// however large the shift is. Zeroing the block is the *oracle's* reading,
/// not H.2's: a threshold of `2^31` or more is above every `i32` magnitude,
/// so the rule below, taken literally, would keep every coefficient as
/// background. `t1.c` zeroes `datap` wholesale instead, and this follows it —
/// changing the arm to keep the coefficients would diverge from OpenJPEG on
/// every such stream (see `docs/openjpeg-departures.md`).
fn undo_maxshift(mag: i32, roi_shift: u8) -> i32 {
    if roi_shift == 0 {
        return mag;
    }
    if roi_shift >= 31 {
        return 0;
    }
    let threshold = 1i32 << roi_shift;
    if mag >= threshold {
        mag >> roi_shift
    } else {
        mag
    }
}

/// The most significant coded bit-plane of a code-block, one-based: OpenJPEG's
/// `bpno_plus_one = roishift + cblk->numbps`, zero when nothing is coded.
///
/// The zero bit-planes signaled in the packet header count against the
/// *raised* plane count `Mb + roi_shift` (ISO H.2 sets Kmax = Mb + s), so under
/// a maxshift a background-only block can carry more of them than the band has
/// magnitude planes — the shift must be added before the subtraction, or such
/// a block starts too high and decodes its coefficients too large. OpenJPEG's
/// arithmetic is signed; exceeding even the raised count saturates to zero
/// coded planes here, which is also the oracle's outcome — its negative
/// `bpno_plus_one` fails the pass loop's `>= 1` guard, running zero passes,
/// and the block decodes to all zeros without an error.
pub fn top_coded_plane(numbps: u32, zero_bit_planes: u32, roi_shift: u8) -> u32 {
    (numbps + u32::from(roi_shift)).saturating_sub(zero_bit_planes)
}

/// The per-component parameters a code-block decode needs beyond its own
/// subband: the code-block style flags (`SPcod`/`SPcoc`) and the
/// region-of-interest maxshift (`SPrgn`).
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockParams {
    pub style: u8,
    pub roi_shift: u8,
}

/// Which of the three coding passes runs next within a bit-plane (ISO D.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassType {
    SigProp,
    MagRef,
    Cleanup,
}

/// How a codeword segment is decoded: the MQ arithmetic decoder for the
/// context-coded passes, or a raw bit reader for the `bypass` (lazy) passes of
/// the low bit-planes (D.6). One codeword segment uses exactly one of them.
enum PassCoder<'a> {
    Mq(MqDecoder<'a>),
    Raw(RawDecoder<'a>),
}

/// Decode one code-block: run the bit-plane passes over its codeword segments
/// into `state`.
///
/// `numbps` is the subband's magnitude bit-plane count `Mb` (guard bits +
/// quantization exponent − 1); `num_passes` and `zero_bit_planes` come from the
/// Tier-2 packet headers; `orient` is the subband's orientation (it selects the
/// zero-coding table); and `style` is the COD/COC code-block style flags. On
/// On return `state.coeffs` holds the signed quantized coefficients at **twice**
/// their weight — OpenJPEG's `datap` as `opj_t1_clbl_decode_processor` receives
/// it. Halving them is the caller's job because the two arms do it differently:
///
/// - reversible (5/3): an integer `datap[i] /= 2`, truncating toward zero;
/// - irreversible (9/7): folded into the float dequantization, which multiplies
///   by `0.5f * band->stepsize` rather than by `band->stepsize`. The half bit
///   survives into the float, and truncating it first loses up to half a
///   quantization step per coefficient.
///
/// Halving here would force the integer form on both.
///
/// Following OpenJPEG, decoding runs at double scale: it begins at the most
/// significant coded plane `Mb − zero_bit_planes` with a cleanup pass and walks
/// down three passes per plane to plane 1 (never plane 0), carrying each
/// magnitude in mid-point form, then halves toward zero. A stream that stops
/// early (the lossy/rate-truncated case) simply leaves the un-coded low planes
/// zero. Quality layers are handled in Tier-2, which concatenates a block's
/// per-layer contributions into the single MQ codeword decoded here.
///
/// `segments` carries the block's codeword segments in order, each with the
/// coding passes it codes. The MQ decoder is re-initialized at every segment —
/// under `restart` that is once per pass — while the context states, the
/// significance map, and the bit-plane counter carry straight through, exactly
/// as `opj_t1_decode_cblk` does. Without termination there is one segment and
/// the codeword runs unbroken.
///
/// `decode_cod` admits every Part 1 style flag and rejects only the two HTJ2K
/// bits. Most act at the segment split or the context, which Tier-2 and
/// [`BlockState`] have already applied, so the pass cycle below is unchanged.
/// Two reach into the passes themselves: `segmentation symbols` appends four
/// decisions to every cleanup pass, and `bypass` (lazy) switches the
/// significance and refinement passes of the low bit-planes from MQ-coded to raw
/// (D.6) — see the per-segment coder choice below.
pub fn decode_block(
    segments: &[(Vec<u8>, u32)],
    state: &mut BlockState,
    orient: Orientation,
    numbps: u32,
    num_passes: u32,
    zero_bit_planes: u32,
    params: BlockParams,
) -> Result<()> {
    let BlockParams { style, roi_shift } = params;
    use crate::codestream::markers::code_block_style::{
        LAZY, PTERM, RESET, SEGSYM, TERMALL, VCAUSAL,
    };
    debug_assert_eq!(
        style & !(LAZY | TERMALL | PTERM | SEGSYM | VCAUSAL | RESET),
        0,
        "decode_cod rejects every style flag but bypass, restart, pterm, segsym, vcausal and reset",
    );
    // Vertically causal context changes how every significance/sign context is
    // formed; the pass functions read it off the state.
    state.vcausal = style & VCAUSAL != 0;
    if num_passes == 0 {
        return Ok(());
    }

    // Decode at twice the final scale, matching OpenJPEG: the most significant
    // coded plane is `cblk.numbps = Mb − zero_bit_planes` (`numbps = Mb`, the
    // subband's guard + exponent − 1), and decoding walks down to plane 1, never
    // plane 0. Carrying the extra low bit lets the mid-point reconstruction stay
    // integral (the half is ≥ 1 at every coded plane); the final halving below
    // drops it. A truncated (lossy) stream simply stops higher, leaving the
    // un-coded low planes zero. For a fully coded block
    // `Mb − zero_bit_planes == num_passes.div_ceil(3)`.
    // Maxshift starts the block `roi_shift` planes higher, so that every
    // region-of-interest coefficient outranks every background one; see
    // `top_coded_plane` for the interplay with the zero bit-planes.
    let top = top_coded_plane(numbps, zero_bit_planes, roi_shift);
    // The double scale means `1 << top` must stay inside `i32`. `decode_subband`
    // rejects this too, before allocating; checking again here is where OpenJPEG
    // puts it (`opj_t1_decode_cblk`'s `bpno_plus_one >= 31`), so a caller that
    // reaches `decode_block` by another route cannot overflow
    // `Plane::become_significant`. It does *not* bound the maxshift: enough
    // zero bit-planes keep `top` small under any `SPrgn`, which is why
    // `undo_maxshift` carries its own `roi_shift >= 31` arm.
    if top > MAX_BIT_PLANES {
        return Err(Error::Limit(format!(
            "code-block needs {top} bit-planes, over the {MAX_BIT_PLANES}-plane limit"
        )));
    }
    if top < 1 {
        return Ok(()); // no coded magnitude planes: the block stays zero
    }

    let mut cx = init_contexts();
    let mut bpno = top;
    // The most significant plane runs the cleanup pass only (D.4), so the pass
    // cycle starts at cleanup and wraps to the next plane after it.
    let mut pass_type = PassType::Cleanup;

    let bypass = style & LAZY != 0;
    // Under `bypass` a significance or refinement pass turns raw once the four
    // most significant coded bit-planes are behind it (D.6, Table D.9), i.e.
    // once `bpno <= top - 4`. The count starts at the block's first coded
    // plane, `top`, which includes the maxshift (H.2): the shifted planes are
    // coded planes, and tier-2's pass-count split of the segments
    // (`segment_max_passes`) agrees. OpenJPEG's decoder counts from
    // `cblk->numbps`, which leaves the shift out, and so decodes a maxshift +
    // bypass stream wrong, its own encoder's included (docs/openjpeg-departures.md,
    // "Bypass under a maxshift"). Signed, because `top - 4` is negative on a
    // shallow block that never reaches the raw region.
    let raw_from = top as i32 - 4;

    'segments: for (bytes, passes) in segments {
        // A codeword segment is a single terminated stream — MQ for the
        // context-coded passes, raw for the `bypass` ones. Context states and
        // `bpno` carry across segments; only the arithmetic decoder restarts.
        //
        // The coder is fixed by the pass the segment opens on, matching t1.c's
        // per-segment `type`: the lazy split (Tier-2) keeps each segment
        // homogeneous — a two-pass raw run (significance + refinement) or a
        // one-pass MQ cleanup — so this one decision holds for every pass in it.
        let raw = bypass && pass_type != PassType::Cleanup && (bpno as i32) <= raw_from;
        let mut coder = if raw {
            PassCoder::Raw(RawDecoder::new(bytes))
        } else {
            PassCoder::Mq(MqDecoder::new(bytes))
        };
        for _ in 0..*passes {
            if bpno < 1 {
                // The plane counter is exhausted, so this segment stops mid-way
                // and every later one goes unread. None of them is the
                // terminated whole `predictable termination` promises about, and
                // their bytes are never consumed — checking them would reject a
                // sound codestream for leaving a buffer it never had to read.
                break 'segments;
            }
            match (&mut coder, pass_type) {
                (PassCoder::Mq(mq), PassType::SigProp) => {
                    sig_prop_pass(mq, state, &mut cx, orient, bpno)
                }
                (PassCoder::Mq(mq), PassType::MagRef) => mag_ref_pass(mq, state, &mut cx, bpno),
                (PassCoder::Mq(mq), PassType::Cleanup) => {
                    // Whether the encoder terminated this cleanup pass, as
                    // t1.c's `opj_t1_enc_is_term_pass` decides it. The
                    // encoder's plane is `bpno - 1` and its first plane is
                    // `top - 1` (the double scale), so its last cleanup is
                    // `bpno == 1`, and under `bypass` it terminates every
                    // cleanup from the fourth coded plane down, which is
                    // `bpno - 1 <= raw_from`. Under `restart` every pass is
                    // terminated.
                    let terminated = style & TERMALL != 0
                        || bpno == 1
                        || (bypass && bpno as i32 - 1 <= raw_from);
                    cleanup_pass(mq, state, &mut cx, orient, bpno, style, terminated)?
                }
                (PassCoder::Raw(rd), PassType::SigProp) => sig_prop_pass_raw(rd, state, bpno),
                (PassCoder::Raw(rd), PassType::MagRef) => mag_ref_pass_raw(rd, state, bpno),
                // The lazy split routes every cleanup to its own MQ segment, so a
                // raw segment never carries one.
                (PassCoder::Raw(_), PassType::Cleanup) => {
                    unreachable!("a bypass cleanup pass is MQ-coded, never raw")
                }
            }
            // Under `reset context probabilities` the MQ context states return to
            // their initial estimates after every pass (D.3, OpenJPEG's
            // `opj_mqc_resetstates` + the three seeded contexts). The default
            // style carries the states across passes. OpenJPEG guards this with
            // `type == T1_TYPE_MQ`: the raw passes of `bypass` carry no context,
            // so they never reset it.
            if style & RESET != 0 && !raw {
                cx = init_contexts();
            }
            pass_type = match pass_type {
                PassType::SigProp => PassType::MagRef,
                PassType::MagRef => PassType::Cleanup,
                // A cleanup pass closes the plane; the next one starts below it.
                // `bpno >= 1` here, guaranteed by the guard above, so this
                // cannot underflow — and a `bpno` of 0 stops the next pass.
                PassType::Cleanup => {
                    bpno -= 1;
                    PassType::SigProp
                }
            };
        }
        // Every codeword segment is separately terminated, so under `predictable
        // termination` each one must account for its own bytes. OpenJPEG checks
        // only the last segment of a block and warns; checking each one costs
        // nothing and localizes the damage. The check is the MQ flush's, so it
        // applies to MQ segments only — a raw `bypass` segment ends on a byte
        // boundary with no such guarantee.
        if let PassCoder::Mq(mq) = &coder {
            if style & PTERM != 0 && !mq.ends_predictably() {
                return Err(Error::Codestream(format!(
                    "code-block style declares predictable termination, but a codeword segment \
                     leaves {} of its {} byte(s) unconsumed; the coded data is corrupt",
                    mq.unconsumed(),
                    bytes.len(),
                )));
            }
        }
    }

    // Undo the maxshift (Annex H.2), then apply the decoded signs.
    //
    // The passes carried each magnitude in the mid-point reconstruction form at
    // double scale (ISO E.1.1.2, r = ½): becoming significant set
    // `2^bpno + 2^(bpno−1)` and each refinement nudged it by ±2^(bpno−1). The
    // coefficients are left at that scale; the caller halves them, because how
    // the half bit is dropped depends on the arm it lands in (see this
    // function's contract).
    //
    // The maxshift is undone here, before that halving, as
    // `opj_t1_clbl_decode_processor` does it: its threshold is stated in the
    // coefficients' own scale, and halving first would drop a region coefficient
    // below it.
    for y in 0..state.height {
        for x in 0..state.width {
            let i = state.idx(x, y);
            let mag = undo_maxshift(state.coeffs[i], roi_shift);
            state.coeffs[i] = if state.state[state.pad(x, y)] & NEGATIVE != 0 {
                -mag
            } else {
                mag
            };
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::golden_vectors::GOLDEN_BLOCKS;
    use super::*;
    /// Decode one golden block from its committed segment into a fresh state.
    fn decode_golden(g: &super::golden_vectors::GoldenBlock) -> Vec<i32> {
        // The golden vectors carry no termination, so one segment holds them all.
        let segments = [(g.segment.to_vec(), g.num_passes)];
        let mut state = BlockState::new(g.width, g.height);
        // The golden vectors are fully coded (lossless, every plane to plane 0),
        // so Mb = decoded-plane count + skipped MSB planes.
        let numbps = g.num_passes.div_ceil(3) + g.zero_bit_planes;
        decode_block(
            &segments,
            &mut state,
            g.orient,
            numbps,
            g.num_passes,
            g.zero_bit_planes,
            BlockParams::default(),
        )
        .expect("the default style decodes without a style check");
        // `decode_block` leaves the coefficients at double scale; the golden
        // vectors are the halved, integer values the reversible arm produces.
        state.coeffs.iter().map(|&c| c / 2).collect()
    }

    /// Each committed code-block segment decodes bit-exactly to its oracle
    /// coefficient grid (decoded sample − DC shift, from `opj_decompress`). The
    /// three blocks carry different pass counts (19, 16, 10), so a bit-exact
    /// match also proves the bit-plane loop runs *exactly* `num_passes` passes:
    /// stopping early or late would scale or corrupt the magnitudes.
    #[test]
    fn golden_blocks_decode_to_expected_coefficients() {
        for g in GOLDEN_BLOCKS {
            assert_eq!(decode_golden(g), g.coeffs, "block {}", g.name);
        }
    }

    /// Vertically causal context excludes the next stripe from the 3×3
    /// neighborhood of a stripe's bottom row (`y % 4 == 3`), and only there. The
    /// three context types must all see the reduction — significance
    /// (`neighbor_sums`/`zc_context`), sign (`sc_context`), and refinement
    /// (`mr_context`) — because OpenJPEG forms them from the same flags.
    #[test]
    fn vertically_causal_context_excludes_the_next_stripe() {
        // Two stripes of a 3-wide block. Make the whole row below (1, 3) — the
        // top of the second stripe — significant and negative, so the vertical
        // and both diagonal south neighbors of (1, 3) are set.
        let mut state = BlockState::new(3, 8);
        for x in 0..3 {
            state.set_significant(x, 4);
            state.set_negative(x, 4, true);
        }

        // Default: (1, 3) sees one vertical and two diagonal south neighbors.
        let (h, v, d) = state.neighbor_sums(1, 3);
        assert_eq!((h, v, d), (0, 1, 2), "default neighborhood");
        assert_ne!(state.zc_context(1, 3, Orientation::Ll), CTX_ZC);
        // Sign: the south neighbors are negative, so v < 0.
        let (_, xor_default) = state.sc_context(1, 3);
        assert_eq!(xor_default, 1, "negative south neighbors flip the sign");
        // First refinement: a significant neighbor selects context 15, not 14.
        assert_eq!(state.mr_context(1, 3), CTX_MR + 1);

        // Vertically causal: the next stripe vanishes from (1, 3)'s context.
        state.vcausal = true;
        assert_eq!(
            state.neighbor_sums(1, 3),
            (0, 0, 0),
            "south excluded at ci==3"
        );
        assert_eq!(state.zc_context(1, 3, Orientation::Ll), CTX_ZC);
        let (sc, xor_causal) = state.sc_context(1, 3);
        assert_eq!((sc, xor_causal), (CTX_SC, 0), "no neighbor, neutral sign");
        assert_eq!(state.mr_context(1, 3), CTX_MR, "no significant neighbor");

        // An interior row (ci == 2) keeps its south neighbor even under vcausal:
        // (1, 2)'s south row (y = 3) is inside the same stripe.
        for x in 0..3 {
            state.set_significant(x, 3);
        }
        let (_, v_interior, _) = state.neighbor_sums(1, 2);
        assert_eq!(v_interior, 1, "an interior row is unaffected by vcausal");
    }

    /// A code-block with no coding passes contributes nothing.
    #[test]
    fn zero_passes_yields_all_zero_coefficients() {
        let mut state = BlockState::new(4, 4);
        decode_block(
            &[(vec![0x80], 0)],
            &mut state,
            Orientation::Ll,
            8,
            0,
            0,
            BlockParams::default(),
        )
        .expect("zero passes decode to nothing");
        assert!(state.coeffs.iter().all(|&c| c == 0));
    }

    /// `predictable termination` promises each terminated segment accounts for
    /// its own bytes, so a segment padded with bytes the codeword never reaches
    /// is corrupt. Without the flag the same segment decodes without complaint,
    /// which is what makes this a check and not a new parse rule.
    #[test]
    fn predictable_termination_catches_a_segment_the_codeword_does_not_consume() {
        use crate::codestream::markers::code_block_style::PTERM;
        let padded = vec![0x80, 0, 0, 0, 0, 0, 0, 0];

        let mut state = BlockState::new(4, 4);
        let err = decode_block(
            &[(padded.clone(), 1)],
            &mut state,
            Orientation::Ll,
            8,
            1,
            0,
            BlockParams {
                style: PTERM,
                roi_shift: 0,
            },
        )
        .expect_err("eight bytes for one cleanup pass leaves more than two unconsumed");
        assert!(matches!(err, Error::Codestream(_)), "{err:?}");

        let mut state = BlockState::new(4, 4);
        decode_block(
            &[(padded, 1)],
            &mut state,
            Orientation::Ll,
            8,
            1,
            0,
            BlockParams::default(),
        )
        .expect("the same segment is fine without the flag");
    }

    /// A segment cut short because the bit-planes ran out is not a terminated
    /// whole, so the predictable-termination promise does not apply to it.
    #[test]
    fn predictable_termination_ignores_a_segment_stopped_by_the_bit_plane_floor() {
        use crate::codestream::markers::code_block_style::PTERM;
        let mut state = BlockState::new(4, 4);
        // `top` is 1, so the first cleanup pass drops `bpno` to 0 and the second
        // pass never runs; the segment's bytes stay unconsumed.
        decode_block(
            &[(vec![0x80, 0, 0, 0, 0, 0, 0, 0], 4)],
            &mut state,
            Orientation::Ll,
            8,
            4,
            7,
            BlockParams {
                style: PTERM,
                roi_shift: 0,
            },
        )
        .expect("a segment the decoder stops early in is not checked");
    }

    /// The segmentation symbol is verified, not just consumed. Flipping a bit of
    /// a cleanup pass's trailing symbol has to surface as a typed error rather
    /// than as silently wrong coefficients.
    #[test]
    fn a_corrupt_segmentation_symbol_is_a_codestream_error() {
        use crate::codestream::markers::code_block_style::SEGSYM;
        // An all-zero codeword decodes the uniform context to a value that is
        // not `1010`, which is exactly the corruption the symbol exists to find.
        let mut state = BlockState::new(4, 4);
        let err = decode_block(
            &[(vec![0x00; 8], 1)],
            &mut state,
            Orientation::Ll,
            8,
            1,
            0,
            BlockParams {
                style: SEGSYM,
                roi_shift: 0,
            },
        )
        .expect_err("the trailing symbol is not 1010");
        let Error::Codestream(message) = &err else {
            panic!("{err:?}")
        };
        assert!(message.contains("segmentation symbol"), "{message}");
    }

    /// Decode one 4x4 code-block whose only segment is empty, so every
    /// decision comes from the fill past its end, with `passes` coding passes
    /// from the top plane `numbps`.
    fn decode_fill(numbps: u32, passes: u32, style: u8) -> Result<()> {
        let mut state = BlockState::new(4, 4);
        decode_block(
            &[(Vec::new(), passes)],
            &mut state,
            Orientation::Ll,
            numbps,
            passes,
            0,
            BlockParams {
                style,
                roi_shift: 0,
            },
        )
    }

    fn assert_segmentation_symbol_error(result: Result<()>) {
        let err = result.expect_err("a terminated pass reads a correct symbol from fill");
        let Error::Codestream(message) = &err else {
            panic!("{err:?}")
        };
        assert!(message.contains("segmentation symbol"), "{message}");
    }

    /// A wrong symbol read from the fill past the segment end decodes on a
    /// pass the encoder did not terminate, because rate control can cut such a
    /// pass a few bytes short (see [`read_segmentation_symbol`]).
    #[test]
    fn a_wrong_segmentation_symbol_read_from_fill_decodes_on_an_unterminated_pass() {
        use crate::codestream::markers::code_block_style::SEGSYM;
        // The fill really does spell a wrong symbol, so the decode below is
        // not vacuous.
        let mut mq = MqDecoder::new(&[]);
        let mut cx = init_contexts();
        let mut state = BlockState::new(4, 4);
        cleanup_pass(&mut mq, &mut state, &mut cx, Orientation::Ll, 8, 0, false).unwrap();
        let symbol = (0..4).fold(0u8, |s, _| (s << 1) | mq.decode(&mut cx[CTX_UNI as usize]));
        assert_ne!(symbol, 0b1010);
        assert!(mq.ran_past_end());

        // The cleanup of plane 8 of 8, with planes left below it.
        decode_fill(8, 1, SEGSYM).expect("a symbol read from fill decodes");
        // Ten passes end at the cleanup of plane 2 without `bypass`.
        decode_fill(5, 10, SEGSYM).expect("no pass up to plane 2 is terminated");
    }

    /// Under `restart` every pass is terminated, so its length is exact and a
    /// wrong symbol is corruption even past the end.
    #[test]
    fn a_wrong_segmentation_symbol_on_a_restart_pass_is_an_error() {
        use crate::codestream::markers::code_block_style::{SEGSYM, TERMALL};
        assert_segmentation_symbol_error(decode_fill(8, 1, SEGSYM | TERMALL));
    }

    /// The block's last cleanup, at plane 1, is always terminated.
    #[test]
    fn a_wrong_segmentation_symbol_on_the_last_cleanup_is_an_error() {
        use crate::codestream::markers::code_block_style::SEGSYM;
        assert_segmentation_symbol_error(decode_fill(1, 1, SEGSYM));
        // Plane 2 of 2 is not the last plane, so it decodes.
        decode_fill(2, 1, SEGSYM).expect("the cleanup of plane 2 is not terminated");
    }

    /// Under `bypass` the encoder terminates every cleanup from the fourth
    /// coded plane down. With five planes, the first ten passes end at the
    /// cleanup of plane 2, the fourth coded plane, so it is terminated, and the
    /// cleanups of planes 5, 4 and 3 before it are not.
    #[test]
    fn a_wrong_segmentation_symbol_on_a_bypass_terminated_cleanup_is_an_error() {
        use crate::codestream::markers::code_block_style::{LAZY, SEGSYM};
        assert_segmentation_symbol_error(decode_fill(5, 10, SEGSYM | LAZY));
        // Nine passes stop before that cleanup, after the cleanup of plane 3.
        decode_fill(5, 9, SEGSYM | LAZY).expect("the cleanup of plane 3 is not terminated");
    }

    /// The maxshift boundary, against the reference formula. `t1.c` reads
    ///
    /// ```text
    /// thresh = 1 << roishift;
    /// if (mag >= thresh) { mag >>= roishift; }
    /// ```
    ///
    /// so a magnitude of exactly `2^roi_shift` is the *smallest* the encoder can
    /// have lifted, and it must come down. `>` for `>=` leaves it up by
    /// `roi_shift` planes; the whole-component ROI fixture cannot see that,
    /// because it has no background coefficient below the threshold at all.
    #[test]
    fn undo_maxshift_matches_the_reference_formula_across_the_boundary() {
        /// `t1.c`'s roishift block, transliterated.
        fn reference(mag: i32, roishift: u8) -> i32 {
            if roishift == 0 {
                return mag;
            }
            let thresh = 1i32 << roishift;
            if mag >= thresh { mag >> roishift } else { mag }
        }

        for roi_shift in 0..=12u8 {
            let threshold = 1i32 << roi_shift;
            // Straddle the boundary, and cover zero and the far tails.
            let cases = [
                0,
                1,
                threshold - 2,
                threshold - 1,
                threshold,
                threshold + 1,
                threshold * 2,
                threshold * 1234 + 7,
                i32::MAX,
            ];
            for mag in cases.into_iter().filter(|&m| m >= 0) {
                assert_eq!(
                    undo_maxshift(mag, roi_shift),
                    reference(mag, roi_shift),
                    "mag {mag}, roi_shift {roi_shift}"
                );
            }
        }
    }

    /// A magnitude one below the threshold is background and stays where it is;
    /// one at the threshold is the region and comes down. This is the single
    /// discriminating pair, spelled out so a boundary regression names itself.
    #[test]
    fn the_maxshift_threshold_separates_background_from_region() {
        let roi_shift = 6;
        let threshold = 1i32 << roi_shift; // 64
        assert_eq!(undo_maxshift(threshold - 1, roi_shift), 63, "background");
        assert_eq!(undo_maxshift(threshold, roi_shift), 1, "region");
        assert_eq!(undo_maxshift(0, roi_shift), 0, "zero is neither");
        // Without a shift nothing moves, whatever the magnitude.
        assert_eq!(undo_maxshift(threshold, 0), threshold);
    }

    /// The zero bit-planes count against the raised `Mb + roi_shift` (Kmax of
    /// ISO H.2), so under a maxshift they may exceed `Mb` alone — a
    /// background-only block in `p0_06` does exactly that. Subtracting them
    /// from `Mb` first (saturating) and adding the shift after would start
    /// such a block `zero_bit_planes − Mb` planes too high and decode its
    /// coefficients that many planes too large.
    #[test]
    fn zero_bit_planes_count_against_the_maxshift_raised_planes() {
        // OpenJPEG: bpno_plus_one = roishift + (numbps − P), signed.
        assert_eq!(top_coded_plane(10, 3, 0), 7, "no ROI");
        assert_eq!(top_coded_plane(10, 3, 9), 16, "ROI block, few zero planes");
        assert_eq!(top_coded_plane(10, 13, 9), 6, "background block, P > Mb");
        assert_eq!(top_coded_plane(10, 19, 9), 0, "all planes zero");
        assert_eq!(top_coded_plane(10, 25, 9), 0, "past Kmax saturates");
    }

    /// A hostile `SPrgn` whose raised `top` overflows the bit-plane budget is
    /// rejected. Rejection is only this arm's outcome: with enough zero
    /// bit-planes the same shifts are *accepted* and the block zeroed — the
    /// companion test below.
    #[test]
    fn an_out_of_range_maxshift_is_rejected_rather_than_overflowing() {
        for roi_shift in [31u8, 32, 64, 255] {
            let mut state = BlockState::new(4, 4);
            let err = decode_block(
                &[(vec![0x80], 1)],
                &mut state,
                Orientation::Ll,
                8,
                1,
                0,
                BlockParams {
                    style: 0,
                    roi_shift,
                },
            )
            .expect_err(&format!("maxshift {roi_shift} passes the bit-plane limit"));
            assert!(matches!(err, Error::Limit(_)), "{err:?}");
        }
    }

    /// The bit-plane limit alone does not bound `SPrgn`: enough zero bit-planes
    /// pull `top` back under it whatever the shift, so an inexpressible shift
    /// reaches `undo_maxshift`. OpenJPEG's `roishift >= 31` arm zeroes such a
    /// block — this must do the same, not overflow `1i32 << roi_shift`.
    #[test]
    fn an_inexpressible_maxshift_on_a_decodable_block_zeroes_it() {
        for roi_shift in [31u8, 64, 255] {
            let mut state = BlockState::new(4, 4);
            // numbps 8, zero planes 8 + roi_shift − 5: top decodes 5 planes.
            let zero_bit_planes = 8 + u32::from(roi_shift) - 5;
            decode_block(
                &[(vec![0x80, 0x27], 3)],
                &mut state,
                Orientation::Ll,
                8,
                3,
                zero_bit_planes,
                BlockParams {
                    style: 0,
                    roi_shift,
                },
            )
            .expect("a small top decodes whatever the maxshift");
            assert!(
                state.coeffs.iter().all(|&c| c == 0),
                "roi_shift {roi_shift}: every coefficient reads as region and shifts to nothing"
            );
        }
    }

    /// A codeword segment may carry coding passes and no bytes: under `restart`
    /// each pass is its own segment, and its length field may be zero. The MQ
    /// decoder reads `0xFF` past the end of its slice, the way OpenJPEG pads
    /// with `OPJ_COMMON_CBLK_DATA_EXTRA`, so this decodes rather than panicking.
    /// A block whose every segment is empty decodes the same way (B.10.7, D.4.1).
    #[test]
    fn an_empty_codeword_segment_does_not_panic() {
        let mut state = BlockState::new(4, 4);
        decode_block(
            &[(Vec::new(), 1), (vec![0x80], 1)],
            &mut state,
            Orientation::Ll,
            8,
            2,
            0,
            BlockParams {
                style: crate::codestream::markers::code_block_style::TERMALL,
                roi_shift: 0,
            },
        )
        .expect("an empty segment decodes as 0xFF padding");
    }

    /// Signs are recovered, not just magnitudes: the sparse block has both
    /// positive and negative significant coefficients among its zeros.
    #[test]
    fn golden_block_recovers_signs() {
        let sparse = GOLDEN_BLOCKS
            .iter()
            .find(|g| g.name == "sparse_8x8")
            .unwrap();
        let out = decode_golden(sparse);
        assert!(
            out.iter().any(|&c| c > 0),
            "expected a positive coefficient"
        );
        assert!(
            out.iter().any(|&c| c < 0),
            "expected a negative coefficient"
        );
        assert_eq!(out, sparse.coeffs);
    }

    /// Build a block and mark the listed `(x, y)` offsets significant, optionally
    /// negative. Center tests on `(2, 2)` of a 5×5 block so all eight neighbors
    /// are in range unless a test deliberately probes an edge.
    fn block_with(sig: &[(u32, u32)]) -> BlockState {
        let mut b = BlockState::new(5, 5);
        for &(x, y) in sig {
            b.set_significant(x, y);
        }
        b
    }

    #[test]
    fn state_round_trips() {
        let mut b = BlockState::new(4, 3);
        assert!(!b.is_significant(1, 2));
        b.set_significant(1, 2);
        assert!(b.is_significant(1, 2));

        assert!(!b.is_visited(1, 2));
        b.set_visited(1, 2, true);
        assert!(b.is_visited(1, 2));
        b.set_visited(1, 2, false);
        assert!(!b.is_visited(1, 2));

        b.set_negative(1, 2, true);
        assert!(b.is_negative(1, 2));

        assert!(!b.is_refined(1, 2));
        b.set_refined(1, 2);
        assert!(b.is_refined(1, 2));
    }

    // --- Zero coding (ISO Table D-1) -------------------------------------

    /// Each `LL`/`LH` context label, hit by a hand-built neighborhood whose
    /// (ΣH, ΣV, ΣD) lands in that row of the table.
    #[test]
    fn zc_ll_lh_all_labels() {
        let cases: [(&[(u32, u32)], u8); 9] = [
            (&[], 0),               // h0 v0 d0
            (&[(1, 1)], 1),         // h0 v0 d1
            (&[(1, 1), (3, 3)], 2), // h0 v0 d2
            (&[(2, 1)], 3),         // h0 v1
            (&[(2, 1), (2, 3)], 4), // h0 v2
            (&[(1, 2)], 5),         // h1 v0 d0
            (&[(1, 2), (1, 1)], 6), // h1 v0 d1
            (&[(1, 2), (2, 1)], 7), // h1 v1
            (&[(1, 2), (3, 2)], 8), // h2
        ];
        for (sig, want) in cases {
            let b = block_with(sig);
            assert_eq!(
                b.zc_context(2, 2, Orientation::Ll),
                CTX_ZC + want,
                "LL {sig:?}"
            );
            assert_eq!(
                b.zc_context(2, 2, Orientation::Lh),
                CTX_ZC + want,
                "LH {sig:?}"
            );
        }
    }

    /// `HL` is the `LL`/`LH` table with horizontal and vertical roles swapped:
    /// a purely horizontal neighborhood (ΣH=2) that scores 8 for LL must follow
    /// the ΣV=2 row for HL, and vice versa.
    #[test]
    fn zc_hl_swaps_h_and_v() {
        // Two horizontal neighbors: LL → 8 (h2), HL reads it as v2 → 4.
        let horiz = block_with(&[(1, 2), (3, 2)]);
        assert_eq!(horiz.zc_context(2, 2, Orientation::Ll), CTX_ZC + 8);
        assert_eq!(horiz.zc_context(2, 2, Orientation::Hl), CTX_ZC + 4);

        // Two vertical neighbors: LL → 4 (v2), HL reads it as h2 → 8.
        let vert = block_with(&[(2, 1), (2, 3)]);
        assert_eq!(vert.zc_context(2, 2, Orientation::Ll), CTX_ZC + 4);
        assert_eq!(vert.zc_context(2, 2, Orientation::Hl), CTX_ZC + 8);

        // One horizontal + one vertical is symmetric (label 7) either way.
        let mix = block_with(&[(1, 2), (2, 1)]);
        assert_eq!(mix.zc_context(2, 2, Orientation::Hl), CTX_ZC + 7);
    }

    /// Each `HH` context label, keyed by (ΣD, ΣH+ΣV).
    #[test]
    fn zc_hh_all_labels() {
        let cases: [(&[(u32, u32)], u8); 9] = [
            (&[], 0),                       // d0 hv0
            (&[(2, 1)], 1),                 // d0 hv1
            (&[(2, 1), (1, 2)], 2),         // d0 hv2
            (&[(1, 1)], 3),                 // d1 hv0
            (&[(1, 1), (2, 1)], 4),         // d1 hv1
            (&[(1, 1), (2, 1), (1, 2)], 5), // d1 hv2
            (&[(1, 1), (3, 3)], 6),         // d2 hv0
            (&[(1, 1), (3, 3), (2, 1)], 7), // d2 hv1
            (&[(1, 1), (3, 3), (3, 1)], 8), // d3
        ];
        for (sig, want) in cases {
            let b = block_with(sig);
            assert_eq!(
                b.zc_context(2, 2, Orientation::Hh),
                CTX_ZC + want,
                "HH {sig:?}"
            );
        }
    }

    // --- Sign coding (ISO Tables D-2, D-3) ------------------------------

    /// Build a block whose horizontal pair sums to `h` and vertical pair to `v`
    /// (each in -1..=1), by signing one neighbor per axis.
    fn block_for_sign(h: i32, v: i32) -> BlockState {
        let mut b = BlockState::new(5, 5);
        let mut sign = |x: u32, y: u32, neg: bool| {
            b.set_significant(x, y);
            b.set_negative(x, y, neg);
        };
        match h {
            1 => sign(1, 2, false),
            -1 => sign(1, 2, true),
            _ => {}
        }
        match v {
            1 => sign(2, 1, false),
            -1 => sign(2, 1, true),
            _ => {}
        }
        b
    }

    #[test]
    fn sc_all_nine_contexts_and_xorbit() {
        let cases: [(i32, i32, u8, u8); 9] = [
            (1, 1, 4, 0),
            (1, 0, 3, 0),
            (1, -1, 2, 0),
            (0, 1, 1, 0),
            (0, 0, 0, 0),
            (0, -1, 1, 1),
            (-1, 1, 2, 1),
            (-1, 0, 3, 1),
            (-1, -1, 4, 1),
        ];
        for (h, v, label, xor) in cases {
            let b = block_for_sign(h, v);
            assert_eq!(b.sc_context(2, 2), (CTX_SC + label, xor), "H={h} V={v}");
        }
    }

    /// Two like-signed neighbors on one axis clamp to a single contribution
    /// (ISO Table D-2: H, V are bounded to -1..=1).
    #[test]
    fn sc_contributions_clamp() {
        let mut b = BlockState::new(5, 5);
        for x in [1, 3] {
            b.set_significant(x, 2); // both horizontal neighbors positive
        }
        // H would be +2 unclamped; clamped to +1 → label 3, xorbit 0.
        assert_eq!(b.sc_context(2, 2), (CTX_SC + 3, 0));
    }

    // --- Magnitude refinement (ISO Table D-4) ---------------------------

    #[test]
    fn mr_first_refinement_with_no_significant_neighbors() {
        let b = BlockState::new(5, 5);
        assert_eq!(b.mr_context(2, 2), CTX_MR);
    }

    #[test]
    fn mr_first_refinement_with_a_significant_neighbor() {
        let b = block_with(&[(2, 1)]);
        assert_eq!(b.mr_context(2, 2), CTX_MR + 1);
    }

    #[test]
    fn mr_after_first_refinement_ignores_neighbors() {
        let mut b = block_with(&[(2, 1)]);
        b.set_refined(2, 2);
        assert_eq!(b.mr_context(2, 2), CTX_MR + 2);
    }

    // --- Edge clamping --------------------------------------------------

    /// At a corner the off-block neighbors read as insignificant, so a lone
    /// significant in-block neighbor is counted and nothing indexes out of
    /// bounds.
    #[test]
    fn neighbors_clamp_at_corners() {
        let mut b = BlockState::new(3, 3);
        // Corner (0,0): only (1,0), (0,1), (1,1) exist as neighbors.
        b.set_significant(1, 0); // horizontal neighbor of (0,0)
        assert_eq!(b.zc_context(0, 0, Orientation::Ll), CTX_ZC + 5); // h1 v0 d0

        // The opposite corner with a diagonal-only neighbor → label 1.
        let mut c = BlockState::new(3, 3);
        c.set_significant(1, 1); // diagonal neighbor of (2,2)
        assert_eq!(c.zc_context(2, 2, Orientation::Ll), CTX_ZC + 1);

        // Sign and MR at a corner must not panic and read clamped neighbors.
        let _ = b.sc_context(0, 0);
        let _ = b.mr_context(0, 0);
    }

    /// Code-block edge columns: the padding sits between one row's last sample
    /// and the next row's first, so neither sees the other. A whole significant
    /// edge column is invisible to the opposite edge column, and an edge
    /// sample counts only the neighbors inside the block.
    #[test]
    fn edge_columns_see_no_neighbor_across_the_block_edge() {
        let (w, h) = (4, 6);
        let mut east = BlockState::new(w, h);
        for y in 0..h {
            east.set_significant(w - 1, y);
            east.set_negative(w - 1, y, true);
        }
        for y in 0..h {
            assert_eq!(east.neighbor_sums(0, y), (0, 0, 0), "(0, {y})");
            assert_eq!(east.sc_context(0, y), (CTX_SC, 0), "(0, {y})");
            assert_eq!(east.mr_context(0, y), CTX_MR, "(0, {y})");
            // The column beside it does see the edge column: one east
            // neighbor, plus a diagonal above and below where those rows exist.
            let d = u8::from(y > 0) + u8::from(y + 1 < h);
            assert_eq!(east.neighbor_sums(w - 2, y), (1, 0, d), "({}, {y})", w - 2);
        }

        let mut west = BlockState::new(w, h);
        for y in 0..h {
            west.set_significant(0, y);
        }
        for y in 0..h {
            assert_eq!(west.neighbor_sums(w - 1, y), (0, 0, 0), "({}, {y})", w - 1);
            // The edge column itself sees its vertical neighbors only.
            let v = u8::from(y > 0) + u8::from(y + 1 < h);
            assert_eq!(west.neighbor_sums(0, y), (0, v, 0), "(0, {y})");
        }
    }

    /// Code-block edge rows: the top and bottom rows, and blocks one sample
    /// wide or tall, count only in-block neighbors.
    #[test]
    fn edge_rows_count_only_in_block_neighbors() {
        // Every sample of a 3×3 block significant: a corner sees three
        // neighbors, an edge middle five, the center eight.
        let mut full = BlockState::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                full.set_significant(x, y);
            }
        }
        for (x, y) in [(0, 0), (2, 0), (0, 2), (2, 2)] {
            assert_eq!(full.neighbor_sums(x, y), (1, 1, 1), "corner ({x}, {y})");
        }
        assert_eq!(full.neighbor_sums(1, 0), (2, 1, 2), "top edge");
        assert_eq!(full.neighbor_sums(1, 2), (2, 1, 2), "bottom edge");
        assert_eq!(full.neighbor_sums(0, 1), (1, 2, 2), "left edge");
        assert_eq!(full.neighbor_sums(2, 1), (1, 2, 2), "right edge");
        assert_eq!(full.neighbor_sums(1, 1), (2, 2, 4), "center");

        // A one-sample-wide column and a one-sample-tall row.
        let mut column = BlockState::new(1, 8);
        let mut row = BlockState::new(8, 1);
        for i in 0..8 {
            column.set_significant(0, i);
            row.set_significant(i, 0);
        }
        for i in 0..8 {
            let n = u8::from(i > 0) + u8::from(i < 7);
            assert_eq!(column.neighbor_sums(0, i), (0, n, 0), "column {i}");
            assert_eq!(row.neighbor_sums(i, 0), (n, 0, 0), "row {i}");
        }

        // A whole significant bottom row is invisible to the top row.
        let mut bottom = BlockState::new(5, 3);
        for x in 0..5 {
            bottom.set_significant(x, 2);
        }
        for x in 0..5 {
            assert_eq!(bottom.neighbor_sums(x, 0), (0, 0, 0), "({x}, 0)");
        }
    }

    /// Vertically causal coding drops each of the three south neighbors from
    /// a stripe's bottom row, on every stripe, and nothing else: an interior
    /// row keeps its south neighbors and the next stripe's top row keeps its
    /// north ones.
    #[test]
    fn vertically_causal_masks_each_south_neighbor_on_a_stripe_bottom_row() {
        for bottom in [3u32, 7, 11] {
            for dx in [0u32, 1, 2] {
                // (1, bottom)'s south neighbor at (dx, bottom + 1): SW, S or SE.
                let mut b = BlockState::new(3, 13);
                b.set_significant(dx, bottom + 1);
                b.set_negative(dx, bottom + 1, true);
                let seen = if dx == 1 { (0, 1, 0) } else { (0, 0, 1) };
                assert_eq!(b.neighbor_sums(1, bottom), seen, "default, dx {dx}");
                b.vcausal = true;
                let at = format!("vcausal, row {bottom}, dx {dx}");
                assert_eq!(b.neighbor_sums(1, bottom), (0, 0, 0), "{at}");
                for orient in [Orientation::Ll, Orientation::Hl, Orientation::Hh] {
                    assert_eq!(b.zc_context(1, bottom, orient), CTX_ZC, "{at}");
                }
                assert_eq!(b.sc_context(1, bottom), (CTX_SC, 0), "{at}");
                assert_eq!(b.mr_context(1, bottom), CTX_MR, "{at}");
                // The next stripe's top row still sees the bottom row above it.
                b.set_significant(1, bottom);
                assert_eq!(b.neighbor_sums(dx, bottom + 1), seen, "{at}, north kept");
            }
            // An interior row keeps its south neighbor under vcausal.
            let mut b = BlockState::new(3, 13);
            b.vcausal = true;
            b.set_significant(1, bottom);
            assert_eq!(
                b.neighbor_sums(1, bottom - 1),
                (0, 1, 0),
                "row {}",
                bottom - 1
            );
        }
    }

    /// The cleanup pass's run-length test sees the same vertically causal
    /// neighborhood as the contexts. A 4-row column whose only significant
    /// neighbor sits below its bottom row, in the next stripe, still aggregates
    /// under VCAUSAL, so it decodes exactly as if that neighbor were not
    /// significant. Without VCAUSAL the neighbor breaks the run and the decode
    /// differs, which shows the comparison can tell the two apart.
    #[test]
    fn vertically_causal_cleanup_aggregates_past_a_next_stripe_neighbor() {
        use crate::codestream::markers::code_block_style::VCAUSAL;
        let bytes = [0x5A, 0xC3, 0x1F, 0x88, 0x24, 0xE7, 0x6B, 0x90, 0x3D, 0x71];
        // Everything the decode leaves behind: the first stripe's
        // coefficients and signs, every context state, and the decoder's
        // position, read as the next 32 uniform decisions.
        let run = |south_significant: bool, vcausal: bool| {
            // A 1×5 block: the first stripe is rows 0..=3, and row 4 is the
            // next stripe's only row. Row 4 is significant, or only visited so
            // the cleanup pass skips it the same way without its neighbors
            // seeing it.
            let mut state = BlockState::new(1, 5);
            state.vcausal = vcausal;
            if south_significant {
                state.set_significant(0, 4);
            } else {
                state.set_visited(0, 4, true);
            }
            let mut mq = MqDecoder::new(&bytes);
            let mut cx = init_contexts();
            let style = if vcausal { VCAUSAL } else { 0 };
            cleanup_pass(
                &mut mq,
                &mut state,
                &mut cx,
                Orientation::Ll,
                4,
                style,
                false,
            )
            .unwrap();
            let stripe: Vec<(i32, bool)> = (0..4)
                .map(|y| (state.coeffs[y as usize], state.is_negative(0, y)))
                .collect();
            let contexts: Vec<(u8, u8)> = cx.iter().map(|c| (c.index(), c.mps())).collect();
            let after: Vec<u8> = (0..32)
                .map(|_| mq.decode(&mut cx[CTX_UNI as usize]))
                .collect();
            (stripe, contexts, after)
        };
        assert_eq!(
            run(true, true),
            run(false, true),
            "under VCAUSAL the next stripe's neighbor must not break the run"
        );
        assert_ne!(
            run(true, false),
            run(false, false),
            "without VCAUSAL the neighbor breaks the run"
        );
    }

    /// Every table entry is the label function's value for the mask's sums,
    /// and label 0 means exactly that no neighbor is significant, which is
    /// what lets the passes skip on a zero mask.
    #[test]
    fn zc_tables_match_the_label_functions() {
        for mask in 0..=255u8 {
            let (h, v, d) = neighbor_sums(mask);
            assert_eq!(u32::from(h + v + d), mask.count_ones());
            let want = [
                (Orientation::Ll, zc_label_lh(h, v, d)),
                (Orientation::Lh, zc_label_lh(h, v, d)),
                (Orientation::Hl, zc_label_lh(v, h, d)),
                (Orientation::Hh, zc_label_hh(h + v, d)),
            ];
            for (orient, label) in want {
                let got = zc_table(orient)[usize::from(mask)];
                assert_eq!(got, label, "{orient:?} mask {mask:#010b}");
                assert_eq!(got == 0, mask == 0, "{orient:?} mask {mask:#010b}");
            }
        }
    }

    /// The contexts as they were formed before the padded planes: eight
    /// bounds-checked reads of a plain significance map. The table-driven
    /// contexts must agree with it at every sample, for many block shapes and
    /// significance patterns, with and without vertically causal coding.
    #[test]
    fn padded_contexts_match_bounds_checked_neighbor_reads() {
        struct Reference {
            w: u32,
            h: u32,
            sig: Vec<bool>,
            neg: Vec<bool>,
            vcausal: bool,
        }
        impl Reference {
            fn at(&self, x: i64, y: i64) -> Option<usize> {
                let inside = x >= 0 && y >= 0 && x < self.w.into() && y < self.h.into();
                inside.then(|| (y * i64::from(self.w) + x) as usize)
            }
            fn sig(&self, x: i64, y: i64) -> u8 {
                u8::from(self.at(x, y).is_some_and(|i| self.sig[i]))
            }
            fn contrib(&self, x: i64, y: i64) -> i32 {
                match self.at(x, y) {
                    Some(i) if self.sig[i] && self.neg[i] => -1,
                    Some(i) if self.sig[i] => 1,
                    _ => 0,
                }
            }
            fn south(&self, y: i64) -> bool {
                !(self.vcausal && y % 4 == 3)
            }
            fn sums(&self, x: i64, y: i64) -> (u8, u8, u8) {
                let s = |dx: i64| {
                    if self.south(y) {
                        self.sig(x + dx, y + 1)
                    } else {
                        0
                    }
                };
                let h = self.sig(x - 1, y) + self.sig(x + 1, y);
                let v = self.sig(x, y - 1) + s(0);
                let d = self.sig(x - 1, y - 1) + self.sig(x + 1, y - 1) + s(-1) + s(1);
                (h, v, d)
            }
            /// ISO Table D-3, from the clamped (H, V) contributions.
            fn sign(&self, x: i64, y: i64) -> (u8, u8) {
                let south = if self.south(y) {
                    self.contrib(x, y + 1)
                } else {
                    0
                };
                let h = (self.contrib(x - 1, y) + self.contrib(x + 1, y)).clamp(-1, 1);
                let v = (self.contrib(x, y - 1) + south).clamp(-1, 1);
                let (label, xor) = match (h, v) {
                    (1, 1) => (4, 0),
                    (1, 0) => (3, 0),
                    (1, -1) => (2, 0),
                    (0, 1) => (1, 0),
                    (0, 0) => (0, 0),
                    (0, -1) => (1, 1),
                    (-1, 1) => (2, 1),
                    (-1, 0) => (3, 1),
                    _ => (4, 1),
                };
                (CTX_SC + label, xor)
            }
        }

        // xorshift64, so the patterns are fixed and need no dependency.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        let mut shapes: Vec<(u32, u32)> = Vec::new();
        for w in 1..=6 {
            for h in 1..=9 {
                shapes.push((w, h));
            }
        }
        shapes.extend([(13, 10), (16, 16), (64, 4), (4, 64)]);

        for (w, h) in shapes {
            let n = (w * h) as usize;
            // One sample significant at each position in turn, then random
            // maps of rising density.
            let mut maps: Vec<Vec<bool>> =
                (0..n).map(|k| (0..n).map(|i| i == k).collect()).collect();
            for density in [8u64, 32, 64, 128, 224] {
                maps.push((0..n).map(|_| next() % 256 < density).collect());
            }
            for vcausal in [false, true] {
                for sig in &maps {
                    let neg: Vec<bool> = (0..n).map(|_| next() % 2 == 1).collect();
                    let mut state = BlockState::new(w, h);
                    state.vcausal = vcausal;
                    for y in 0..h {
                        for x in 0..w {
                            let i = (y * w + x) as usize;
                            if sig[i] {
                                state.set_significant(x, y);
                                state.set_negative(x, y, neg[i]);
                            }
                        }
                    }
                    let reference = Reference {
                        w,
                        h,
                        sig: sig.clone(),
                        neg,
                        vcausal,
                    };
                    for y in 0..h {
                        for x in 0..w {
                            let at = format!("{w}x{h} vcausal {vcausal} at ({x}, {y})");
                            let (sh, sv, sd) = reference.sums(x.into(), y.into());
                            assert_eq!(state.neighbor_sums(x, y), (sh, sv, sd), "{at}");
                            let zc = [
                                (Orientation::Ll, zc_label_lh(sh, sv, sd)),
                                (Orientation::Hl, zc_label_lh(sv, sh, sd)),
                                (Orientation::Hh, zc_label_hh(sh + sv, sd)),
                            ];
                            for (orient, label) in zc {
                                let got = state.zc_context(x, y, orient);
                                assert_eq!(got, CTX_ZC + label, "{at} {orient:?}");
                            }
                            let sign = reference.sign(x.into(), y.into());
                            assert_eq!(state.sc_context(x, y), sign, "{at}");
                            let mr = CTX_MR + u8::from(sh + sv + sd > 0);
                            assert_eq!(state.mr_context(x, y), mr, "{at}");
                        }
                    }
                }
            }
        }
    }

    /// Marking a sample significant sets exactly one bit in each neighbor's
    /// state word, the direction that neighbor sees it in, and for a negative
    /// sample the matching sign bit in each direct neighbor. The sign context
    /// reads `NB_N`, `NB_S`, `NB_W`, `NB_E` and the `NEG_*` bits one at a
    /// time, and vertically causal coding reads the south ones, so a swap of
    /// any two directions is a bug even where the zero-coding tables would not
    /// notice it. The centers sit on stripe rows 0, 1 and 3, so the north and
    /// south neighbors are found both within a stripe and across a stripe
    /// boundary, in both directions.
    #[test]
    fn each_neighbor_direction_sets_its_own_bit() {
        // (dx, dy) of the significant sample as seen from the center, and the
        // bits the center's word must get for it.
        let directions: [(i32, i32, u16, u16); 8] = [
            (0, -1, NB_N, NEG_N),
            (0, 1, NB_S, NEG_S),
            (-1, 0, NB_W, NEG_W),
            (1, 0, NB_E, NEG_E),
            (-1, -1, NB_NW, 0),
            (1, -1, NB_NE, 0),
            (-1, 1, NB_SW, 0),
            (1, 1, NB_SE, 0),
        ];
        for cy in [4u32, 5, 7] {
            for (dx, dy, bit, neg_bit) in directions {
                for negative in [false, true] {
                    let (x, y) = ((1 + dx) as u32, (cy as i32 + dy) as u32);
                    let want = bit | if negative { neg_bit } else { 0 };
                    let at = format!("center (1, {cy}), neighbor ({x}, {y}), negative {negative}");
                    // Through the test helpers, which set the sign after.
                    let mut b = BlockState::new(3, 12);
                    b.set_significant(x, y);
                    b.set_negative(x, y, negative);
                    assert_eq!(b.state[b.pad(1, cy)], want, "{at}");
                    // Through the marking the passes use, which takes the
                    // sign up front.
                    let mut m = BlockState::new(3, 12);
                    let p = m.pad(x, y);
                    mark_significant(&mut m.state, p, y as usize % 4, m.stripe_len, negative);
                    assert_eq!(m.state[m.pad(1, cy)], want, "{at}, mark_significant");
                    // Nothing in the block changed but the sample and its
                    // eight neighbors.
                    for oy in 0..12u32 {
                        for ox in 0..3u32 {
                            if ox.abs_diff(x) > 1 || oy.abs_diff(y) > 1 {
                                assert_eq!(m.state[m.pad(ox, oy)], 0, "{at}, ({ox}, {oy})");
                            }
                        }
                    }
                }
            }
        }
    }

    /// The three MQ pass loops written the plain way: every sample in scan
    /// order, its contexts from the coordinate accessors, no column skips and
    /// no run test on a column word.
    mod reference {
        use super::*;

        fn sign(
            mq: &mut MqDecoder<'_>,
            cx: &mut [Context],
            b: &BlockState,
            x: u32,
            y: u32,
        ) -> bool {
            let (ctx, xor) = b.sc_context(x, y);
            mq.decode(&mut cx[usize::from(ctx)]) ^ xor == 1
        }

        fn significant(b: &mut BlockState, x: u32, y: u32, bpno: u32, negative: bool) {
            let one = 1i32 << bpno;
            let i = b.idx(x, y);
            b.coeffs[i] = one | (one >> 1);
            b.set_significant(x, y);
            b.set_negative(x, y, negative);
        }

        fn scan(b: &BlockState) -> Vec<(u32, u32)> {
            let mut order = Vec::new();
            for y0 in (0..b.height).step_by(4) {
                for x in 0..b.width {
                    for y in y0..(y0 + 4).min(b.height) {
                        order.push((x, y));
                    }
                }
            }
            order
        }

        fn zero_code(
            mq: &mut MqDecoder<'_>,
            cx: &mut [Context],
            b: &mut BlockState,
            orient: Orientation,
            (x, y): (u32, u32),
            bpno: u32,
        ) {
            let ctx = b.zc_context(x, y, orient);
            if mq.decode(&mut cx[usize::from(ctx)]) == 1 {
                let negative = sign(mq, cx, b, x, y);
                significant(b, x, y, bpno, negative);
            }
        }

        pub fn sig_prop(
            mq: &mut MqDecoder<'_>,
            cx: &mut [Context],
            b: &mut BlockState,
            orient: Orientation,
            bpno: u32,
        ) {
            for (x, y) in scan(b) {
                if b.is_significant(x, y) || b.neighbor_sums(x, y) == (0, 0, 0) {
                    continue;
                }
                zero_code(mq, cx, b, orient, (x, y), bpno);
                b.set_visited(x, y, true);
            }
        }

        pub fn mag_ref(mq: &mut MqDecoder<'_>, cx: &mut [Context], b: &mut BlockState, bpno: u32) {
            for (x, y) in scan(b) {
                if !b.is_significant(x, y) || b.is_visited(x, y) {
                    continue;
                }
                let bit = mq.decode(&mut cx[usize::from(b.mr_context(x, y))]);
                let half = (1i32 << bpno) >> 1;
                let i = b.idx(x, y);
                b.coeffs[i] += if bit == 1 { half } else { -half };
                b.set_refined(x, y);
            }
        }

        pub fn cleanup(
            mq: &mut MqDecoder<'_>,
            cx: &mut [Context],
            b: &mut BlockState,
            orient: Orientation,
            bpno: u32,
        ) {
            let quiet = |b: &BlockState, x: u32, y: u32| {
                !b.is_significant(x, y) && !b.is_visited(x, y) && b.neighbor_sums(x, y) == (0, 0, 0)
            };
            for y0 in (0..b.height).step_by(4) {
                for x in 0..b.width {
                    let rows = (y0 + 4).min(b.height) - y0;
                    let mut first = 0;
                    if rows == 4 && (y0..y0 + 4).all(|y| quiet(b, x, y)) {
                        if mq.decode(&mut cx[CTX_RUN as usize]) == 0 {
                            continue;
                        }
                        let hi = mq.decode(&mut cx[CTX_UNI as usize]);
                        let lo = mq.decode(&mut cx[CTX_UNI as usize]);
                        let run = u32::from((hi << 1) | lo);
                        let negative = sign(mq, cx, b, x, y0 + run);
                        significant(b, x, y0 + run, bpno, negative);
                        first = run + 1;
                    }
                    for y in y0 + first..y0 + rows {
                        if !b.is_significant(x, y) && !b.is_visited(x, y) {
                            zero_code(mq, cx, b, orient, (x, y), bpno);
                        }
                    }
                }
            }
            for y in 0..b.height {
                for x in 0..b.width {
                    b.set_visited(x, y, false);
                }
            }
        }
    }

    /// Everything a pass leaves behind that a later pass or the caller can
    /// see: each sample's coefficient and state bits, every context state, and
    /// the decoder's position, read as the next 48 uniform decisions.
    type PassOutcome = (Vec<(i32, bool, bool, bool, bool)>, Vec<Context>, Vec<u8>);

    fn pass_outcome(
        b: &BlockState,
        cx: &[Context; NUM_CONTEXTS],
        mq: &mut MqDecoder<'_>,
    ) -> PassOutcome {
        let mut samples = Vec::new();
        for y in 0..b.height {
            for x in 0..b.width {
                samples.push((
                    b.coeffs[b.idx(x, y)],
                    b.is_significant(x, y),
                    b.is_visited(x, y),
                    b.is_negative(x, y),
                    b.is_refined(x, y),
                ));
            }
        }
        let mut uniform = Context::new(46, 0);
        let after = (0..48).map(|_| mq.decode(&mut uniform)).collect();
        (samples, cx.to_vec(), after)
    }

    /// The pass loops, with their column skips, unrolled full stripes, short
    /// last stripes, run test on a column word and per-column `VISITED`
    /// clearing, decode exactly as the plain per-sample loops in
    /// [`reference`] do. Each pass starts from a random state (significance
    /// and sign, `VISITED` and `REFINED` marks, at several densities) and
    /// random context states, for block shapes whose last stripe has one to
    /// four rows, every orientation, and vertically causal coding on and off.
    #[test]
    fn pass_loops_match_a_per_sample_reference() {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let shapes = [
            (1, 1),
            (1, 4),
            (1, 5),
            (2, 7),
            (3, 8),
            (4, 9),
            (5, 10),
            (6, 11),
            (7, 12),
            (8, 3),
            (9, 6),
            (13, 13),
            (16, 16),
            (5, 2),
        ];
        let orients = [
            Orientation::Ll,
            Orientation::Lh,
            Orientation::Hl,
            Orientation::Hh,
        ];
        let densities = [0u64, 4, 16, 64, 160, 240];
        let trials = 6;
        let mut checked = 0;
        for (w, h) in shapes {
            for vcausal in [false, true] {
                for density in densities {
                    for _ in 0..trials {
                        let mut start = BlockState::new(w, h);
                        start.vcausal = vcausal;
                        for y in 0..h {
                            for x in 0..w {
                                if next() % 256 < density {
                                    start.set_significant(x, y);
                                    start.set_negative(x, y, next() % 2 == 1);
                                    let i = start.idx(x, y);
                                    start.coeffs[i] = (next() % 1000) as i32 + 64;
                                    if next() % 3 == 0 {
                                        start.set_refined(x, y);
                                    }
                                }
                                if next() % 256 < density / 2 {
                                    start.set_visited(x, y, true);
                                }
                            }
                        }
                        let bytes: Vec<u8> = (0..24).map(|_| next() as u8).collect();
                        let mut cx = init_contexts();
                        for c in cx.iter_mut() {
                            *c = Context::new((next() % 47) as u8, (next() % 2) as u8);
                        }
                        let orient = orients[(next() % 4) as usize];
                        let bpno = (next() % 8) as u32 + 2;
                        let copy = |from: &BlockState| {
                            let mut b = BlockState::new(w, h);
                            b.state.clone_from(&from.state);
                            b.coeffs.clone_from(&from.coeffs);
                            b.vcausal = from.vcausal;
                            b
                        };
                        for pass in 0..3 {
                            let (mut real, mut want) = (copy(&start), copy(&start));
                            let mut mq = MqDecoder::new(&bytes);
                            let mut ref_mq = MqDecoder::new(&bytes);
                            let (mut cx_real, mut cx_want) = (cx, cx);
                            match pass {
                                0 => {
                                    sig_prop_pass(&mut mq, &mut real, &mut cx_real, orient, bpno);
                                    reference::sig_prop(
                                        &mut ref_mq,
                                        &mut cx_want,
                                        &mut want,
                                        orient,
                                        bpno,
                                    );
                                }
                                1 => {
                                    mag_ref_pass(&mut mq, &mut real, &mut cx_real, bpno);
                                    reference::mag_ref(&mut ref_mq, &mut cx_want, &mut want, bpno);
                                }
                                _ => {
                                    cleanup_pass(
                                        &mut mq,
                                        &mut real,
                                        &mut cx_real,
                                        orient,
                                        bpno,
                                        0,
                                        false,
                                    )
                                    .unwrap();
                                    reference::cleanup(
                                        &mut ref_mq,
                                        &mut cx_want,
                                        &mut want,
                                        orient,
                                        bpno,
                                    );
                                }
                            }
                            assert_eq!(
                                pass_outcome(&real, &cx_real, &mut mq),
                                pass_outcome(&want, &cx_want, &mut ref_mq),
                                "{w}x{h} vcausal {vcausal} density {density} {orient:?} pass {pass}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, shapes.len() * 2 * densities.len() * trials * 3);
    }
}
