//! The three EBCOT coding passes and their context formation
//! (ISO/IEC 15444-1 Annex D).
//!
//! A code-block is decoded bit-plane by bit-plane, MSB→LSB. Each plane runs, in
//! order, the passes that apply: significance propagation (D.3.1), magnitude
//! refinement (D.3.2), and cleanup (D.3.3) with its run-length mode. Contexts
//! come from the significance state of the 3×3 neighbourhood (D.3.* tables),
//! and sign coding uses the neighbour-sign context (D.3.4). The cleanup pass
//! of the top plane runs first; the optional code-block style flags
//! (bypass/lazy, reset, restart, vertically-causal, segmentation) modulate
//! termination and context reset.

use crate::tier1::mq::{Context, MqDecoder};

#[cfg(test)]
#[path = "golden_vectors.rs"]
mod golden_vectors;

/// MQ context indices, laid out as ISO/IEC 15444-1 Table D-7 allocates them and
/// OpenJPEG's `T1_CTXNO_*` constants order them. A code-block's decoder owns one
/// [`Context`](crate::tier1::mq::Context) per index (`NUM_CONTEXTS` total); the
/// context-formation routines below return an index into that array.
///
/// - `0..=8`   zero coding (the nine significance contexts, [`zc_context`](BlockState::zc_context))
/// - `9..=13`  sign coding ([`sc_context`](BlockState::sc_context))
/// - `14..=16` magnitude refinement ([`mr_context`](BlockState::mr_context))
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
/// vertical neighbour roles, and `HH` keys off the diagonal count instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Ll,
    Lh,
    Hl,
    Hh,
}

/// A code-block's decode state: the coefficient magnitudes plus the per-sample
/// flag planes the passes consult and update. Each flag vector is parallel to
/// `coeffs` (row-major, `width × height`).
#[derive(Debug)]
pub struct BlockState {
    pub width: u32,
    pub height: u32,
    pub coeffs: Vec<i32>,
    /// σ: the coefficient has become significant (a 1 bit has been coded).
    significant: Vec<bool>,
    /// The coefficient was coded in the current bit-plane's significance
    /// propagation pass, so the cleanup pass skips it.
    visited: Vec<bool>,
    /// Sign bit: `true` is negative. Meaningful only once significant.
    negative: Vec<bool>,
    /// The coefficient has been through at least one magnitude-refinement pass,
    /// which selects magnitude-refinement context 16 over 14/15 (D.3.2).
    refined: Vec<bool>,
}

impl BlockState {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        BlockState {
            width,
            height,
            coeffs: vec![0; n],
            significant: vec![false; n],
            visited: vec![false; n],
            negative: vec![false; n],
            refined: vec![false; n],
        }
    }

    /// Flat row-major index of an in-bounds coefficient.
    fn idx(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }

    /// Whether `(x, y)` lies inside the block.
    fn in_bounds(&self, x: i64, y: i64) -> bool {
        x >= 0 && y >= 0 && x < self.width as i64 && y < self.height as i64
    }

    /// Significance at a possibly out-of-range position; positions outside the
    /// block read as insignificant, which clamps the 3×3 neighbourhood at the
    /// block edges (D.3: out-of-block samples contribute 0).
    fn sig_at(&self, x: i64, y: i64) -> bool {
        self.in_bounds(x, y) && self.significant[self.idx(x as u32, y as u32)]
    }

    /// A single neighbour's signed contribution to the sign context (Table D-2):
    /// `0` if insignificant, `+1` if significant and positive, `-1` if negative.
    fn contrib_at(&self, x: i64, y: i64) -> i32 {
        if !self.sig_at(x, y) {
            0
        } else if self.negative[self.idx(x as u32, y as u32)] {
            -1
        } else {
            1
        }
    }

    pub fn is_significant(&self, x: u32, y: u32) -> bool {
        self.significant[self.idx(x, y)]
    }

    pub fn set_significant(&mut self, x: u32, y: u32) {
        let i = self.idx(x, y);
        self.significant[i] = true;
    }

    pub fn is_visited(&self, x: u32, y: u32) -> bool {
        self.visited[self.idx(x, y)]
    }

    pub fn set_visited(&mut self, x: u32, y: u32, value: bool) {
        let i = self.idx(x, y);
        self.visited[i] = value;
    }

    /// Set a coefficient's magnitude when it first becomes significant at plane
    /// `bpno`, to the mid-point reconstruction value `2^bpno + 2^(bpno−1)`
    /// (ISO E.1.1.2 / OpenJPEG `oneplushalf`): the significant bit plus half the
    /// next plane's weight, placing it at the centre of its interval.
    fn set_significant_magnitude(&mut self, x: u32, y: u32, bpno: u32) {
        let one = 1i32 << bpno;
        let i = self.idx(x, y);
        self.coeffs[i] = one | (one >> 1);
    }

    /// Refine a significant coefficient's magnitude at plane `bpno`: a `1` bit
    /// pushes it up by half the plane weight, a `0` bit down (OpenJPEG `poshalf`,
    /// the running mid-point update). Decoding runs at double scale and stops at
    /// `bpno == 1`, so `poshalf` is always ≥ 1; the carried half is dropped by
    /// the final halving in [`decode_block`].
    fn refine_magnitude(&mut self, x: u32, y: u32, bpno: u32, bit: u8) {
        let poshalf = (1i32 << bpno) >> 1;
        let i = self.idx(x, y);
        self.coeffs[i] += if bit == 1 { poshalf } else { -poshalf };
    }

    /// Clear every per-sample `visited` flag. The flag marks samples coded in
    /// the current bit-plane's significance-propagation pass; it is local to one
    /// plane, so the cleanup pass clears it before the next plane begins.
    fn clear_visited(&mut self) {
        self.visited.iter_mut().for_each(|v| *v = false);
    }

    pub fn is_negative(&self, x: u32, y: u32) -> bool {
        self.negative[self.idx(x, y)]
    }

    pub fn set_negative(&mut self, x: u32, y: u32, value: bool) {
        let i = self.idx(x, y);
        self.negative[i] = value;
    }

    pub fn is_refined(&self, x: u32, y: u32) -> bool {
        self.refined[self.idx(x, y)]
    }

    pub fn set_refined(&mut self, x: u32, y: u32) {
        let i = self.idx(x, y);
        self.refined[i] = true;
    }

    /// (ΣH, ΣV, ΣD): the significance sums of the two horizontal, two vertical,
    /// and four diagonal neighbours of `(x, y)` — the inputs to every D.3 table.
    fn neighbour_sums(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let (x, y) = (x as i64, y as i64);
        let h = self.sig_at(x - 1, y) as u8 + self.sig_at(x + 1, y) as u8;
        let v = self.sig_at(x, y - 1) as u8 + self.sig_at(x, y + 1) as u8;
        let d = self.sig_at(x - 1, y - 1) as u8
            + self.sig_at(x + 1, y - 1) as u8
            + self.sig_at(x - 1, y + 1) as u8
            + self.sig_at(x + 1, y + 1) as u8;
        (h, v, d)
    }

    /// Zero-coding context label (`CTX_ZC..CTX_SC`) for `(x, y)` in a subband of
    /// the given orientation (ISO Table D-1). `LL`/`LH` use the base assignment,
    /// `HL` swaps the horizontal and vertical sums, and `HH` keys off the
    /// diagonal count.
    pub fn zc_context(&self, x: u32, y: u32, orient: Orientation) -> u8 {
        let (h, v, d) = self.neighbour_sums(x, y);
        let label = match orient {
            Orientation::Ll | Orientation::Lh => zc_label_lh(h, v, d),
            Orientation::Hl => zc_label_lh(v, h, d),
            Orientation::Hh => zc_label_hh(h + v, d),
        };
        CTX_ZC + label
    }

    /// Sign-coding context (`CTX_SC..CTX_MR`) and the XOR bit (ISO Tables D-2,
    /// D-3). The MQ decision is XORed with the returned bit to recover the sign
    /// (`0` positive, `1` negative).
    pub fn sc_context(&self, x: u32, y: u32) -> (u8, u8) {
        let (x, y) = (x as i64, y as i64);
        let h = (self.contrib_at(x - 1, y) + self.contrib_at(x + 1, y)).clamp(-1, 1);
        let v = (self.contrib_at(x, y - 1) + self.contrib_at(x, y + 1)).clamp(-1, 1);
        let (label, xor) = match (h, v) {
            (1, 1) => (4, 0),
            (1, 0) => (3, 0),
            (1, -1) => (2, 0),
            (0, 1) => (1, 0),
            (0, 0) => (0, 0),
            (0, -1) => (1, 1),
            (-1, 1) => (2, 1),
            (-1, 0) => (3, 1),
            (-1, -1) => (4, 1),
            _ => unreachable!("clamped contributions are in -1..=1"),
        };
        (CTX_SC + label, xor)
    }

    /// Magnitude-refinement context (`CTX_MR..CTX_RUN`) for `(x, y)` (ISO
    /// Table D-4 / D.3.2). After the first refinement the context is always 16;
    /// on the first refinement it is 15 if any neighbour is significant, else 14.
    pub fn mr_context(&self, x: u32, y: u32) -> u8 {
        if self.is_refined(x, y) {
            CTX_MR + 2
        } else {
            let (h, v, d) = self.neighbour_sums(x, y);
            let label = if h + v + d > 0 { 1 } else { 0 };
            CTX_MR + label
        }
    }
}

/// Zero-coding label for the `LL`/`LH` table (ISO Table D-1). `HL` reuses this
/// with `h` and `v` exchanged by the caller.
fn zc_label_lh(h: u8, v: u8, d: u8) -> u8 {
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
fn zc_label_hh(hv: u8, d: u8) -> u8 {
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

/// Initial MQ states for a fresh code-block (ISO Table D-7 / Annex D.3): the
/// uniform context starts at state 46, the run-length context at 3, and the
/// all-insignificant zero-coding context at 4; every other context at 0. Using
/// the default zero state for these three would desynchronize the decoder.
fn init_contexts() -> [Context; NUM_CONTEXTS] {
    let mut cx = [Context::default(); NUM_CONTEXTS];
    cx[CTX_ZC as usize].index = 4;
    cx[CTX_RUN as usize].index = 3;
    cx[CTX_UNI as usize].index = 46;
    cx
}

/// Decode the sign of a coefficient that has just become significant (D.3.4):
/// the MQ decision under the neighbour-sign context is XORed with the context's
/// predicted sign. Records the sign in `state` (`true` = negative).
fn decode_sign(mq: &mut MqDecoder<'_>, state: &mut BlockState, cx: &mut [Context], x: u32, y: u32) {
    let (ctx, xor) = state.sc_context(x, y);
    let bit = mq.decode(&mut cx[ctx as usize]);
    state.set_negative(x, y, bit ^ xor == 1);
}

/// Iterate a code-block in EBCOT scan order: stripes of four rows top to bottom,
/// each stripe scanned column by column. The bottom stripe may be shorter.
fn stripes(width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
    (0..height)
        .step_by(4)
        .flat_map(move |y0| (0..width).map(move |x| (x, y0)))
}

/// Significance-propagation pass (D.3.1): visit each still-insignificant sample
/// that has at least one significant neighbour, zero-code it, and on a 1 decode
/// its sign. `bpno` is the current bit-plane; `1 << bpno` is its weight.
fn sig_prop_pass(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context],
    orient: Orientation,
    bpno: u32,
) {
    let (w, h) = (state.width, state.height);
    for (x, y0) in stripes(w, h) {
        for y in y0..(y0 + 4).min(h) {
            if state.is_significant(x, y) {
                continue;
            }
            let ctx = state.zc_context(x, y, orient);
            if ctx == CTX_ZC {
                continue; // no significant neighbour: deferred to cleanup
            }
            if mq.decode(&mut cx[ctx as usize]) == 1 {
                state.set_significant_magnitude(x, y, bpno);
                state.set_significant(x, y);
                decode_sign(mq, state, cx, x, y);
            }
            state.set_visited(x, y, true);
        }
    }
}

/// Magnitude-refinement pass (D.3.2): refine every sample that was already
/// significant before this plane (so not coded in this plane's significance
/// pass) by decoding one more magnitude bit under the refinement context.
fn mag_ref_pass(mq: &mut MqDecoder<'_>, state: &mut BlockState, cx: &mut [Context], bpno: u32) {
    let (w, h) = (state.width, state.height);
    for (x, y0) in stripes(w, h) {
        for y in y0..(y0 + 4).min(h) {
            if !state.is_significant(x, y) || state.is_visited(x, y) {
                continue;
            }
            let ctx = state.mr_context(x, y);
            let bit = mq.decode(&mut cx[ctx as usize]);
            state.refine_magnitude(x, y, bpno, bit);
            state.set_refined(x, y);
        }
    }
}

/// Cleanup pass (D.3.3): code every sample not yet handled this plane. A full
/// column of four insignificant samples with no significant neighbours is
/// aggregated through the run-length context — a 0 leaves all four
/// insignificant, a 1 reads the first significant sample's position as two
/// uniform bits — after which the column finishes with ordinary zero coding.
fn cleanup_pass(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    cx: &mut [Context],
    orient: Orientation,
    bpno: u32,
) {
    let (w, h) = (state.width, state.height);
    for (x, y0) in stripes(w, h) {
        let rows = (y0 + 4).min(h) - y0;
        let mut first = 0;
        if rows == 4 {
            let aggregate = (0..4).all(|dy| {
                let y = y0 + dy;
                !state.is_significant(x, y)
                    && !state.is_visited(x, y)
                    && state.zc_context(x, y, orient) == CTX_ZC
            });
            if aggregate {
                if mq.decode(&mut cx[CTX_RUN as usize]) == 0 {
                    continue; // run of four insignificant samples
                }
                let hi = mq.decode(&mut cx[CTX_UNI as usize]);
                let lo = mq.decode(&mut cx[CTX_UNI as usize]);
                let run = (hi << 1) | lo; // position of the first significant
                let y = y0 + run as u32;
                state.set_significant_magnitude(x, y, bpno);
                state.set_significant(x, y);
                decode_sign(mq, state, cx, x, y);
                first = run as u32 + 1;
            }
        }
        for dy in first..rows {
            let y = y0 + dy;
            if state.is_significant(x, y) || state.is_visited(x, y) {
                continue;
            }
            let ctx = state.zc_context(x, y, orient);
            if mq.decode(&mut cx[ctx as usize]) == 1 {
                state.set_significant_magnitude(x, y, bpno);
                state.set_significant(x, y);
                decode_sign(mq, state, cx, x, y);
            }
        }
    }
    state.clear_visited();
}

/// Decode one code-block: run the bit-plane passes over `mq` into `state`.
///
/// `numbps` is the subband's magnitude bit-plane count `Mb` (guard bits +
/// quantization exponent − 1); `num_passes` and `zero_bit_planes` come from the
/// Tier-2 packet headers; `orient` is the subband's orientation (it selects the
/// zero-coding table); and `style` is the COD/COC code-block style flags. On
/// return `state.coeffs` holds the signed quantized coefficients at their true
/// bit weights.
///
/// Following OpenJPEG, decoding runs at double scale: it begins at the most
/// significant coded plane `Mb − zero_bit_planes` with a cleanup pass and walks
/// down three passes per plane to plane 1 (never plane 0), carrying each
/// magnitude in mid-point form, then halves toward zero. A stream that stops
/// early (the lossy/rate-truncated case) simply leaves the un-coded low planes
/// zero. Phase 1 still handles only the default code-block style and a single
/// quality layer; the non-default styles
/// (bypass/reset/restart/vertically-causal/segmentation) and multi-layer
/// progression are Phase 2.
pub fn decode_block(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    orient: Orientation,
    numbps: u32,
    num_passes: u32,
    zero_bit_planes: u32,
    style: u8,
) {
    let _ = style;
    if num_passes == 0 {
        return;
    }

    // Decode at twice the final scale, matching OpenJPEG: the most significant
    // coded plane is `cblk.numbps = Mb − zero_bit_planes` (`numbps = Mb`, the
    // subband's guard + exponent − 1), and decoding walks down to plane 1, never
    // plane 0. Carrying the extra low bit lets the mid-point reconstruction stay
    // integral (the half is ≥ 1 at every coded plane); the final halving below
    // drops it. A truncated (lossy) stream simply stops higher, leaving the
    // un-coded low planes zero. For a fully coded block
    // `Mb − zero_bit_planes == num_passes.div_ceil(3)`.
    let top = numbps.saturating_sub(zero_bit_planes);
    if top < 1 {
        return; // no coded magnitude planes: the block stays zero
    }
    // Callers reject anything past this (see `decode_code_blocks`); the double
    // scale means `1 << top` must stay inside `i32`.
    debug_assert!(top <= MAX_BIT_PLANES, "bit-plane count {top} overflows i32");

    let mut cx = init_contexts();
    let mut bpno = top;
    let mut left = num_passes;

    // The most significant plane runs the cleanup pass only (D.4).
    cleanup_pass(mq, state, &mut cx, orient, bpno);
    left -= 1;

    while left > 0 && bpno > 1 {
        bpno -= 1;
        sig_prop_pass(mq, state, &mut cx, orient, bpno);
        left -= 1;
        if left == 0 {
            break;
        }
        mag_ref_pass(mq, state, &mut cx, bpno);
        left -= 1;
        if left == 0 {
            break;
        }
        cleanup_pass(mq, state, &mut cx, orient, bpno);
        left -= 1;
    }

    // The passes carried each magnitude in the mid-point reconstruction form at
    // double scale (ISO E.1.1.2, r = ½): becoming significant set
    // `2^bpno + 2^(bpno−1)` and each refinement nudged it by ±2^(bpno−1). Halve
    // toward zero to drop the carried low bit, then apply the decoded signs
    // (OpenJPEG's reversible `tmp / 2`).
    for y in 0..state.height {
        for x in 0..state.width {
            let i = state.idx(x, y);
            let mag = state.coeffs[i] / 2;
            state.coeffs[i] = if state.is_negative(x, y) { -mag } else { mag };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::golden_vectors::GOLDEN_BLOCKS;
    use super::*;
    use crate::tier1::mq::MqDecoder;

    /// Decode one golden block from its committed segment into a fresh state.
    fn decode_golden(g: &super::golden_vectors::GoldenBlock) -> Vec<i32> {
        let mut mq = MqDecoder::new(g.segment);
        let mut state = BlockState::new(g.width, g.height);
        // The golden vectors are fully coded (lossless, every plane to plane 0),
        // so Mb = decoded-plane count + skipped MSB planes.
        let numbps = g.num_passes.div_ceil(3) + g.zero_bit_planes;
        decode_block(
            &mut mq,
            &mut state,
            Orientation::Ll,
            numbps,
            g.num_passes,
            g.zero_bit_planes,
            0,
        );
        state.coeffs
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

    /// A code-block with no coding passes contributes nothing.
    #[test]
    fn zero_passes_yields_all_zero_coefficients() {
        let mut mq = MqDecoder::new(GOLDEN_BLOCKS[0].segment);
        let mut state = BlockState::new(4, 4);
        decode_block(&mut mq, &mut state, Orientation::Ll, 8, 0, 0, 0);
        assert!(state.coeffs.iter().all(|&c| c == 0));
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
    /// negative. Centre tests on `(2, 2)` of a 5×5 block so all eight neighbours
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

    /// Each `LL`/`LH` context label, hit by a hand-built neighbourhood whose
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
    /// a purely horizontal neighbourhood (ΣH=2) that scores 8 for LL must follow
    /// the ΣV=2 row for HL, and vice versa.
    #[test]
    fn zc_hl_swaps_h_and_v() {
        // Two horizontal neighbours: LL → 8 (h2), HL reads it as v2 → 4.
        let horiz = block_with(&[(1, 2), (3, 2)]);
        assert_eq!(horiz.zc_context(2, 2, Orientation::Ll), CTX_ZC + 8);
        assert_eq!(horiz.zc_context(2, 2, Orientation::Hl), CTX_ZC + 4);

        // Two vertical neighbours: LL → 4 (v2), HL reads it as h2 → 8.
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
    /// (each in -1..=1), by signing one neighbour per axis.
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

    /// Two like-signed neighbours on one axis clamp to a single contribution
    /// (ISO Table D-2: H, V are bounded to -1..=1).
    #[test]
    fn sc_contributions_clamp() {
        let mut b = BlockState::new(5, 5);
        for x in [1, 3] {
            b.set_significant(x, 2); // both horizontal neighbours positive
        }
        // H would be +2 unclamped; clamped to +1 → label 3, xorbit 0.
        assert_eq!(b.sc_context(2, 2), (CTX_SC + 3, 0));
    }

    // --- Magnitude refinement (ISO Table D-4) ---------------------------

    #[test]
    fn mr_first_refinement_with_no_significant_neighbours() {
        let b = BlockState::new(5, 5);
        assert_eq!(b.mr_context(2, 2), CTX_MR);
    }

    #[test]
    fn mr_first_refinement_with_a_significant_neighbour() {
        let b = block_with(&[(2, 1)]);
        assert_eq!(b.mr_context(2, 2), CTX_MR + 1);
    }

    #[test]
    fn mr_after_first_refinement_ignores_neighbours() {
        let mut b = block_with(&[(2, 1)]);
        b.set_refined(2, 2);
        assert_eq!(b.mr_context(2, 2), CTX_MR + 2);
    }

    // --- Edge clamping --------------------------------------------------

    /// At a corner the off-block neighbours read as insignificant, so a lone
    /// significant in-block neighbour is counted and nothing indexes out of
    /// bounds.
    #[test]
    fn neighbours_clamp_at_corners() {
        let mut b = BlockState::new(3, 3);
        // Corner (0,0): only (1,0), (0,1), (1,1) exist as neighbours.
        b.set_significant(1, 0); // horizontal neighbour of (0,0)
        assert_eq!(b.zc_context(0, 0, Orientation::Ll), CTX_ZC + 5); // h1 v0 d0

        // The opposite corner with a diagonal-only neighbour → label 1.
        let mut c = BlockState::new(3, 3);
        c.set_significant(1, 1); // diagonal neighbour of (2,2)
        assert_eq!(c.zc_context(2, 2, Orientation::Ll), CTX_ZC + 1);

        // Sign and MR at a corner must not panic and read clamped neighbours.
        let _ = b.sc_context(0, 0);
        let _ = b.mr_context(0, 0);
    }
}
