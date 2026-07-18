//! Stage 5 — inverse discrete wavelet transform (ISO/IEC 15444-1 Annex F).
//!
//! Reconstructs the image from its subbands, one resolution level at a time:
//! each level combines LL with HL/LH/HH into the next-larger LL until the full
//! image remains. Two filter banks, both via the lifting scheme:
//!
//! - **5/3 reversible** (F.3.8.2) — integer lifting, must be bit-exact (it is
//!   the lossless path).
//! - **9/7 irreversible** (F.3.8.1) — floating-point lifting with the four
//!   lifting coefficients and the two scaling constants.
//!
//! Both use whole-sample symmetric (mirror) extension at boundaries (F.3.6).
//!
//! ## The 1-D kernel contract
//!
//! The two kernels here operate on one *interleaved* subband row/column in
//! place (the array the standard calls `a(i)` after the implicit deinterleave).
//! On return the slice holds the reconstructed samples.
//!
//! Which index holds a low-pass coefficient depends on where the row *starts*.
//! A sample is low-pass when its coordinate on the tile-component grid is even,
//! so a row beginning at an odd coordinate begins on a high-pass sample and
//! every parity flips. Both kernels take that starting parity as `cas` — the
//! name OpenJPEG gives it. A single tile at the canvas origin is always even,
//! which is why the decoder could assume it until tiling landed; a tile further
//! along the grid, or a sub-sampled component, produces odd origins routinely.
//!
//! ## The 2-D driver
//!
//! [`inverse`] drives those kernels over the subband layout in [`SubbandCoeffs`].
//! Per resolution level (coarsest first) it scatters the four subbands back into
//! one interleaved grid by the ISO `(xob, yob)` parity — LL to the low-pass row
//! and column, HL to the low-pass row and high-pass column, LH the reverse, HH
//! to both high-pass — then runs the 1-D synthesis across every row and down
//! every column. The merged grid is the next finer level's LL; after the last
//! level it is the full-resolution raster of that tile-component (pre
//! level-shift), which [`crate::image::Canvas`] places into the image.

use crate::Result;
use crate::codestream::MainHeader;
use crate::codestream::markers::Transform;
use crate::tier1::{Band, Bands, DetailBands, SubbandCoeffs};

/// 9/7 lifting coefficients (ISO/IEC 15444-1 Table F.4): the two predict
/// (`ALPHA`, `GAMMA`) and two update (`BETA`, `DELTA`) factors.
/// Values are the ISO constants rounded to `f32` (the lifting runs in `f32`,
/// matching OpenJPEG's reconstruction precision).
const ALPHA: f32 = -1.586_134_3;
const BETA: f32 = -0.052_980_12;
const GAMMA: f32 = 0.882_911_1;
const DELTA: f32 = 0.443_506_85;
/// 9/7 scaling constant `K` (Table F.4). On the inverse the low-pass samples
/// are scaled by `K` and the high-pass samples by its reciprocal.
const K: f32 = 1.230_174_1;

/// Inverse-transform all resolution levels into the final raster of samples
/// (pre level-shift), driven by the COD transform choice and decomposition
/// level count. Output is row-major, `width * height` of the full resolution.
///
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

/// The [`SubbandCoeffs`] arm fixes the arithmetic: reversible 5/3 reconstructs
/// in exact integers, irreversible 9/7 in `f32`. Both must agree with the COD
/// transform (checked in debug builds).
pub fn inverse(header: &MainHeader, comp: usize, coeffs: SubbandCoeffs) -> Result<Samples> {
    match coeffs {
        SubbandCoeffs::Reversible(bands) => {
            let coding = &header.components[comp].coding;
            debug_assert_eq!(coding.transform, Transform::Reversible53);
            // `<=`: under a resolution reduction the pyramid arrives with its
            // finest levels already dropped; the synthesis just stops early.
            debug_assert!(bands.levels.len() <= coding.decomposition_levels as usize);
            Ok(Samples::Reversible(reconstruct(bands, inverse_5_3).data))
        }
        SubbandCoeffs::Irreversible(bands) => {
            let coding = &header.components[comp].coding;
            debug_assert_eq!(coding.transform, Transform::Irreversible97);
            debug_assert!(bands.levels.len() <= coding.decomposition_levels as usize);
            Ok(Samples::Irreversible(reconstruct(bands, inverse_9_7).data))
        }
    }
}

/// Merge the subband pyramid into the full-resolution band, coarsest level
/// first. Each level combines the running LL with its three detail bands via
/// `kernel` (the 1-D synthesis for the chosen filter) and becomes the next
/// level's LL.
fn reconstruct<T, F>(bands: Bands<T>, kernel: F) -> Band<T>
where
    T: Copy + Default,
    F: Fn(&mut [T], usize),
{
    let mut ll = bands.ll;
    for detail in &bands.levels {
        ll = merge_level(&ll, detail, &kernel);
    }
    ll
}

/// One resolution level: scatter `ll` and the three detail bands into an
/// interleaved grid by their `(xob, yob)` parity, then run the 1-D inverse down
/// each column and across each row. Returns the reconstructed (next finer) LL.
///
/// The level's own coordinate frame is recovered from the band origins. For a
/// resolution spanning `[u0, u1)` horizontally, ISO Eq. B-15 puts the low-pass
/// band at `ceil(u0 / 2)` and the high-pass band at `floor(u0 / 2)`, so
///
/// ```text
/// ll.origin.x + hl.origin.x == ceil(u0 / 2) + floor(u0 / 2) == u0
/// ```
///
/// and likewise `ll.origin.y + lh.origin.y == v0`. That is what makes each level
/// self-describing: nothing has to thread the tile's geometry down here.
fn merge_level<T, F>(ll: &Band<T>, detail: &DetailBands<T>, kernel: &F) -> Band<T>
where
    T: Copy + Default,
    F: Fn(&mut [T], usize),
{
    let (hl, lh, hh) = (&detail.hl, &detail.lh, &detail.hh);
    // The four bands tile the resolution: LL/LH share the low-pass column count,
    // HL/HH the high-pass count; LL/HL share the low-pass row count, LH/HH the
    // high-pass count.
    debug_assert_eq!(hl.height, ll.height);
    debug_assert_eq!(lh.width, ll.width);
    debug_assert_eq!(hh.width, hl.width);
    debug_assert_eq!(hh.height, lh.height);

    let (u0, v0) = (ll.origin.0 + hl.origin.0, ll.origin.1 + lh.origin.1);
    // The interleave parity, OpenJPEG's `cas`: with an even origin the row
    // starts on a low-pass sample and the low-pass band lands on the even
    // columns, and with an odd origin every one of those roles flips. A single
    // tile at the canvas origin is always even — which is why this could be
    // assumed until tiles landed — but a tile further along the grid, or a
    // sub-sampled component, puts subbands at odd origins routinely.
    let (cas_x, cas_y) = ((u0 & 1) as usize, (v0 & 1) as usize);

    let width = ll.width + hl.width;
    let height = ll.height + lh.height;
    let mut grid = vec![T::default(); width * height];
    scatter(&mut grid, width, ll, cas_x, cas_y); // LL: low-pass row, low-pass column
    scatter(&mut grid, width, hl, 1 - cas_x, cas_y); // HL: low-pass row, high-pass column
    scatter(&mut grid, width, lh, cas_x, 1 - cas_y); // LH: high-pass row, low-pass column
    scatter(&mut grid, width, hh, 1 - cas_x, 1 - cas_y); // HH: high-pass both

    // Synthesis is separable, but the 5/3 integer lifting rounds, so the two
    // passes do not commute: match OpenJPEG's order — horizontal (rows) first,
    // then vertical (columns) — so the reversible path is bit-exact.
    //
    // A resolution can be *empty* on an axis, which is why the width is checked
    // rather than assumed: a one-sample-wide tile-component at an odd origin has
    // `ceil(u0/2) == ceil(u1/2)`, so its low-pass band holds nothing and a
    // coarse enough resolution collapses to zero columns. Legal input — a narrow
    // edge tile with enough decomposition levels reaches it — and unreachable
    // for a single tile at the canvas origin, which is why it only surfaces now.
    if width > 0 {
        for row in grid.chunks_exact_mut(width) {
            kernel(row, cas_x);
        }
    }
    let mut column = vec![T::default(); height];
    for x in 0..width {
        for (y, slot) in column.iter_mut().enumerate() {
            *slot = grid[y * width + x];
        }
        kernel(&mut column, cas_y);
        for (y, &value) in column.iter().enumerate() {
            grid[y * width + x] = value;
        }
    }

    Band {
        origin: (u0, v0),
        width,
        height,
        data: grid,
    }
}

/// Place every sample of `band` into `grid` (row-major, `grid_width` wide) at
/// the interleaved position `(2*by + row_off, 2*bx + col_off)`.
fn scatter<T: Copy>(
    grid: &mut [T],
    grid_width: usize,
    band: &Band<T>,
    col_off: usize,
    row_off: usize,
) {
    for by in 0..band.height {
        for bx in 0..band.width {
            let x = 2 * bx + col_off;
            let y = 2 * by + row_off;
            grid[y * grid_width + x] = band.data[by * band.width + bx];
        }
    }
}

/// Whole-sample symmetric (mirror) extension (ISO/IEC 15444-1 F.3.6): map any
/// index — including the negative and past-the-end ones the lifting steps reach
/// for — onto a valid position in `0..n`, reflecting about the edge samples
/// without repeating them. For `n > 1` the pattern has period `2 * (n - 1)`, so
/// e.g. `-1 -> 1` and `n -> n - 2`.
fn reflect(i: isize, n: usize) -> usize {
    debug_assert!(n > 0);
    if n == 1 {
        return 0;
    }
    let period = 2 * (n as isize - 1);
    let mut k = i % period;
    if k < 0 {
        k += period;
    }
    if k >= n as isize {
        k = period - k;
    }
    k as usize
}

/// One-dimensional inverse 5/3 reversible lifting over `signal` in place
/// (F.3.8.2). Exact integer arithmetic: the arithmetic right shifts floor
/// toward negative infinity, matching the standard's `floor` even for negative
/// operands.
///
/// The lifting sums run in `i64`: Tier-1 admits double-scale magnitudes near
/// `2^30`, so on a hostile codestream `left + right` (of already-lifted
/// samples) and the `±` on the target can leave `i32` — a debug build would
/// panic on the overflow. Saturating back preserves bit-exactness for every
/// in-range (legal) input, exactly as [`crate::mct`] does for RCT.
fn inverse_5_3(signal: &mut [i32], cas: usize) {
    let n = signal.len();
    // A lone low-pass sample passes through; a lone high-pass sample is halved
    // (F.3.8.2.1's single-sample case, `tiledp[0] /= 2` in `opj_idwt53_h`). The
    // truncating division is the reference's, not a floor: `/ 2`, not `>> 1`.
    if n <= 1 {
        if n == 1 && cas == 1 {
            signal[0] /= 2;
        }
        return;
    }
    let saturate = crate::mct::saturate;
    // Undo the update step on the low-pass samples first, then undo the predict
    // step on the high-pass samples — the forward order reversed. A sample is
    // low-pass when its *absolute* coordinate is even, which is local index
    // `i ≡ cas (mod 2)`.
    for i in (cas..n).step_by(2) {
        let left = signal[reflect(i as isize - 1, n)] as i64;
        let right = signal[reflect(i as isize + 1, n)] as i64;
        signal[i] = saturate(signal[i] as i64 - ((left + right + 2) >> 2));
    }
    for i in (1 - cas..n).step_by(2) {
        let left = signal[reflect(i as isize - 1, n)] as i64;
        let right = signal[reflect(i as isize + 1, n)] as i64;
        signal[i] = saturate(signal[i] as i64 + ((left + right) >> 1));
    }
}

/// One-dimensional inverse 9/7 irreversible lifting over `signal` in place
/// (F.3.8.1): undo the scaling, then the two update/predict lifting pairs in
/// reverse order (`δ` update, `γ` predict, `β` update, `α` predict).
///
/// `cas` is the parity of the signal's first absolute coordinate: low-pass
/// samples sit at local index `i ≡ cas (mod 2)`, high-pass at the other parity.
fn inverse_9_7(signal: &mut [f32], cas: usize) {
    let n = signal.len();
    // A single sample passes through unchanged whichever band it belongs to —
    // `opj_v8dwt_decode`'s two early returns, which cover `sn <= 1, dn == 0`
    // (a lone low-pass sample) and `sn == 0, dn <= 1` (a lone high-pass one).
    // The 5/3 kernel halves the lone high-pass sample and this one does not:
    // that asymmetry is the reference's, and it survives here because deviating
    // from OpenJPEG needs proof, which no reading of F.3.8.1 supplies.
    if n <= 1 {
        return;
    }
    let (low, high) = (cas, 1 - cas);
    // Undo scaling: low-pass by K, high-pass by 2/K. The extra factor of two
    // over the ISO `1/K` carries the subband gain, which dequantization leaves
    // out of the step (see [`crate::quant`]): every high-pass filtering
    // contributes one gain doubling here, so a coefficient is scaled by exactly
    // `2^(number of times it is high-pass)` rather than by a fixed `2^gain`.
    // For an ordinary tile-component the two agree, but they diverge once a
    // coarse resolution is empty, where the high-pass count no longer equals the
    // nominal gain (#126). Written `2.0 * (1/K)` so the doubling is exact and
    // the ordinary case stays bit-for-bit identical to the old form.
    let two_inv_k = 2.0 * (1.0 / K);
    for i in (low..n).step_by(2) {
        signal[i] *= K;
    }
    for i in (high..n).step_by(2) {
        signal[i] *= two_inv_k;
    }
    // Each lifting step adjusts one parity using its two immediate neighbours of
    // the other parity, swept across the whole row before the next step runs.
    lift_step(signal, n, low, -DELTA); // s'_n = s_n - δ(d_{n-1} + d_n)
    lift_step(signal, n, high, -GAMMA); // d'_n = d_n - γ(s'_n + s'_{n+1})
    lift_step(signal, n, low, -BETA); // x_2n  = s'_n - β(d'_{n-1} + d'_n)
    lift_step(signal, n, high, -ALPHA); // x_2n+1 = d'_n - α(x_2n + x_2n+2)
}

/// One 9/7 lifting sweep: add `coeff * (neighbour_left + neighbour_right)` to
/// every sample of the parity `start` selects.
fn lift_step(signal: &mut [f32], n: usize, start: usize, coeff: f32) {
    for i in (start..n).step_by(2) {
        let left = signal[reflect(i as isize - 1, n)];
        let right = signal[reflect(i as isize + 1, n)];
        signal[i] += coeff * (left + right);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALPHA, BETA, Band, Bands, DELTA, DetailBands, GAMMA, K, inverse_5_3, inverse_9_7,
        lift_step, reconstruct, reflect,
    };

    /// Forward 5/3 lifting, the exact inverse of [`inverse_5_3`], transcribed
    /// straight from the standard's predict-then-update order. Kept in the test
    /// module so the round-trip checks `inverse_5_3` against an independent
    /// implementation of the forward transform rather than against itself.
    fn forward_5_3(signal: &mut [i32], cas: usize) {
        let n = signal.len();
        if n <= 1 {
            if n == 1 && cas == 1 {
                signal[0] *= 2;
            }
            return;
        }
        for i in (1 - cas..n).step_by(2) {
            let left = signal[reflect(i as isize - 1, n)];
            let right = signal[reflect(i as isize + 1, n)];
            signal[i] -= (left + right) >> 1;
        }
        for i in (cas..n).step_by(2) {
            let left = signal[reflect(i as isize - 1, n)];
            let right = signal[reflect(i as isize + 1, n)];
            signal[i] += (left + right + 2) >> 2;
        }
    }

    /// Forward 9/7 lifting, the exact inverse of [`inverse_9_7`]: the lifting
    /// pairs in forward order then the reciprocal scaling.
    fn forward_9_7(signal: &mut [f32], cas: usize) {
        let n = signal.len();
        if n <= 1 {
            return;
        }
        let (low, high) = (cas, 1 - cas);
        lift_step(signal, n, high, ALPHA);
        lift_step(signal, n, low, BETA);
        lift_step(signal, n, high, GAMMA);
        lift_step(signal, n, low, DELTA);
        // Reciprocals of the inverse scaling: low-pass by 1/K, high-pass by K/2
        // (the inverse multiplies high-pass by 2/K, carrying the subband gain).
        for i in (low..n).step_by(2) {
            signal[i] *= 1.0 / K;
        }
        for i in (high..n).step_by(2) {
            signal[i] *= K / 2.0;
        }
    }

    /// Whole-sample symmetric extension reflects about the edges without
    /// repeating them: `-1 -> 1`, `n -> n - 2`, period `2(n-1)`.
    #[test]
    fn reflect_mirrors_about_edges() {
        // n = 4, period 6.
        assert_eq!(reflect(-1, 4), 1);
        assert_eq!(reflect(-2, 4), 2);
        assert_eq!(reflect(4, 4), 2);
        assert_eq!(reflect(5, 4), 1);
        assert_eq!(reflect(6, 4), 0);
        // In-range indices are returned unchanged.
        for i in 0..4 {
            assert_eq!(reflect(i as isize, 4), i);
        }
        // Degenerate single sample: everything folds to 0.
        for i in -3..3 {
            assert_eq!(reflect(i, 1), 0);
        }
    }

    /// A non-first tile-component under a high `NL` over a tiny tile can have its
    /// coarse resolutions empty at an odd origin, so a coefficient is high-pass a
    /// different number of times than its nominal subband gain. With the gain
    /// carried per high-pass filtering (2/K) rather than baked into the step
    /// (2^gain), the 9/7 reconstruction is correct here; the old form was not
    /// (#126). These are `p1_06`'s tile (1,0), component 0 — `x ∈ [3, 6)`,
    /// `NL = 4` — with its dequantized coefficients (LL empty; the DC is carried
    /// by an HL coefficient at the first non-empty level), and the expected
    /// column-constant output matches the ISO/IEC 15444-4 reference within class.
    #[test]
    fn empty_coarse_resolution_carries_the_subband_gain_per_filter() {
        fn fb(ox: u32, oy: u32, w: usize, h: usize, data: &[f32]) -> Band<f32> {
            Band {
                origin: (ox, oy),
                width: w,
                height: h,
                data: data.to_vec(),
            }
        }
        let bands = Bands {
            ll: fb(1, 0, 0, 1, &[]),
            levels: vec![
                DetailBands {
                    hl: fb(0, 0, 0, 1, &[]),
                    lh: fb(1, 0, 0, 0, &[]),
                    hh: fb(0, 0, 0, 0, &[]),
                },
                // First non-empty level: a lone HL coefficient carries the DC.
                DetailBands {
                    hl: fb(0, 0, 1, 1, &[4.910_476]),
                    lh: fb(1, 0, 0, 0, &[]),
                    hh: fb(0, 0, 1, 0, &[]),
                },
                DetailBands {
                    hl: fb(1, 0, 0, 1, &[]),
                    lh: fb(1, 0, 1, 1, &[0.0]),
                    hh: fb(1, 0, 0, 1, &[]),
                },
                DetailBands {
                    hl: fb(
                        1,
                        0,
                        2,
                        2,
                        &[-21.382_141, 2.101_135, -21.382_141, 2.101_135],
                    ),
                    lh: fb(2, 0, 1, 1, &[0.0]),
                    hh: fb(1, 0, 2, 1, &[0.0, 0.0]),
                },
            ],
        };
        let out = reconstruct(bands, inverse_9_7);
        assert_eq!((out.width, out.height), (3, 3));
        // Column-constant (all vertical detail is zero), the reference structure.
        let expected = [-23.819, 14.551, 14.359];
        for (i, v) in out.data.iter().enumerate() {
            let want = expected[i % 3];
            assert!(
                (v - want).abs() < 0.02,
                "sample {i}: got {v}, want ~{want} (gain must be per-filter, not 2^gain)"
            );
        }
    }

    /// A worked 5/3 vector derived by hand from F.3.8.2, independent of the
    /// forward reference: the interleaved subband array `[10, 0, 33, 10]` (the
    /// forward 5/3 of the ramp `[10, 20, 30, 40]`) inverts back to the ramp.
    #[test]
    fn inverse_5_3_known_vector() {
        let mut a = [10, 0, 33, 10];
        inverse_5_3(&mut a, 0);
        assert_eq!(a, [10, 20, 30, 40]);
    }

    /// 5/3 is the lossless path, so forward-then-inverse must be bit-exact for
    /// every length, including the odd lengths and tiny boundary cases.
    #[test]
    fn inverse_5_3_round_trips_bit_exact() {
        let signals: &[&[i32]] = &[
            &[],
            &[42],
            &[7, -7],
            &[1, 2, 3],
            &[10, 20, 30, 40],
            &[5, -3, 0, 9, -100, 64, 7],
            &[0, 0, 0, 0, 0, 0, 0, 0],
            &[-5, -10, -15, -20, -25, -30],
            &[1000, -1000, 500, -500, 250, -250, 125, -125, 1],
        ];
        for &s in signals {
            for cas in [0, 1] {
                let mut a = s.to_vec();
                forward_5_3(&mut a, cas);
                inverse_5_3(&mut a, cas);
                assert_eq!(
                    a,
                    s,
                    "5/3 round-trip mismatch for len {} cas {cas}",
                    s.len()
                );
            }
        }
    }

    /// Hostile coefficients at the Tier-1 magnitude ceiling (~2^30 after
    /// double-scale halving) must saturate, not overflow: the raw lifting sums
    /// leave `i32`, which would panic any overflow-checked build.
    #[test]
    fn inverse_5_3_saturates_on_extreme_coefficients() {
        for cas in [0, 1] {
            let mut a = [i32::MAX / 2, i32::MIN / 2, i32::MAX / 2, i32::MIN / 2];
            inverse_5_3(&mut a, cas); // must not panic
            let mut b = [i32::MAX, i32::MIN, i32::MAX, i32::MIN, i32::MAX];
            inverse_5_3(&mut b, cas); // must not panic
        }
    }

    /// 9/7 is float, so forward-then-inverse must recover the input within a
    /// tight tolerance across odd, even, and boundary lengths.
    #[test]
    fn inverse_9_7_round_trips_within_tolerance() {
        let lengths = [2usize, 3, 4, 5, 6, 7, 8, 9, 16, 31];
        for &len in &lengths {
            // A deterministic mix of a ramp and an alternating component.
            let original: Vec<f32> = (0..len)
                .map(|i| i as f32 * 1.5 - if i.is_multiple_of(2) { 3.0 } else { -2.0 })
                .collect();
            for cas in [0, 1] {
                let mut a = original.clone();
                forward_9_7(&mut a, cas);
                inverse_9_7(&mut a, cas);
                for (got, want) in a.iter().zip(&original) {
                    assert!(
                        (got - want).abs() < 1e-3,
                        "9/7 round-trip mismatch at len {len} cas {cas}: got {got}, want {want}",
                    );
                }
            }
        }
    }

    /// A constant (DC) signal is the canonical low-pass case: the forward
    /// transform leaves all energy in the low band (the high-pass coefficients
    /// vanish), and the inverse reconstructs the constant. A structural check on
    /// the lifting, separate from the round-trip. (The *absolute* 9/7 scaling
    /// convention is sealed against the OpenJPEG oracle at integration, P1.7;
    /// a self-contained synthetic test cannot pin it.)
    #[test]
    fn inverse_9_7_preserves_dc() {
        let len = 8;
        let mut a = vec![5.0f32; len];
        forward_9_7(&mut a, 0);
        // High-pass (odd) coefficients of a constant signal vanish.
        for i in (1..len).step_by(2) {
            assert!(a[i].abs() < 1e-3, "high-pass coeff {i} = {}", a[i]);
        }
        inverse_9_7(&mut a, 0);
        for &v in &a {
            assert!((v - 5.0).abs() < 1e-3, "DC not preserved: {v}");
        }
    }

    /// Both kernels must handle empty and single-sample slices as identity
    /// without panicking (no high-pass partner to undo).
    #[test]
    fn degenerate_lengths_are_identity() {
        for cas in [0, 1] {
            let mut empty_i: [i32; 0] = [];
            inverse_5_3(&mut empty_i, cas);
            let mut empty_f: [f32; 0] = [];
            inverse_9_7(&mut empty_f, cas);
            // A single sample passes through 9/7 whichever band it is in.
            let mut one_f = [42.0f32];
            inverse_9_7(&mut one_f, cas);
            assert_eq!(one_f, [42.0]);
        }
        // 5/3 passes a lone low-pass sample through and halves a lone high-pass
        // one (`tiledp[0] /= 2` in `opj_idwt53_h`), truncating toward zero.
        let mut one_low = [42];
        inverse_5_3(&mut one_low, 0);
        assert_eq!(one_low, [42]);
        let mut one_high = [42];
        inverse_5_3(&mut one_high, 1);
        assert_eq!(one_high, [21]);
        let mut one_high_negative = [-43];
        inverse_5_3(&mut one_high_negative, 1);
        assert_eq!(one_high_negative, [-21], "truncating division, not a floor");
    }

    // ---- 2-D multi-level driver ----------------------------------------------
    //
    // Same philosophy as the 1-D round-trips above: an independent forward 2-D
    // transform (rows then columns, then deinterleave into the four subbands)
    // builds the coefficient pyramid that `inverse` must take back to the image.

    use super::{Samples, inverse};
    use crate::codestream::MainHeader;
    use crate::codestream::markers::{
        Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform,
    };
    use crate::tier1::SubbandCoeffs;

    /// A minimal single-component main header. The inverse reads only the
    /// transform choice and the decomposition-level count from it (both checked
    /// against the pyramid in debug builds); the rest is filler.
    fn header(transform: Transform, levels: u8, w: u32, h: u32) -> MainHeader {
        MainHeader::new(
            Siz {
                x_size: w,
                y_size: h,
                x_offset: 0,
                y_offset: 0,
                tile_width: w,
                tile_height: h,
                tile_x_offset: 0,
                tile_y_offset: 0,
                components: vec![SizComponent {
                    bit_depth: 16,
                    signed: false,
                    x_sampling: 1,
                    y_sampling: 1,
                }],
            },
            Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: levels,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                use_sop: false,
                use_eph: false,
                multiple_component_transform: false,
                transform,
                precinct_sizes: Vec::new(),
            },
            Qcd {
                style: QuantStyle::None,
                guard_bits: 2,
                steps: Vec::new(),
            },
        )
    }

    /// Forward 1-D kernel down every column then across every row, in place —
    /// the analysis counterpart of [`super::merge_level`]'s rows-then-columns
    /// synthesis (the passes must run in the reverse order to round-trip).
    /// `cas_x`/`cas_y` are the parities of the grid's own first coordinates.
    fn forward_2d<T: Copy + Default, F: Fn(&mut [T], usize)>(
        grid: &mut [T],
        w: usize,
        h: usize,
        cas_x: usize,
        cas_y: usize,
        fwd: &F,
    ) {
        let mut col = vec![T::default(); h];
        for x in 0..w {
            for (y, slot) in col.iter_mut().enumerate() {
                *slot = grid[y * w + x];
            }
            fwd(&mut col, cas_y);
            for (y, &v) in col.iter().enumerate() {
                grid[y * w + x] = v;
            }
        }
        if w > 0 {
            for row in grid.chunks_exact_mut(w) {
                fwd(row, cas_x);
            }
        }
    }

    /// Deinterleave one `(col_off, row_off)` parity quadrant of a transformed
    /// grid into a subband sitting at `origin` on the tile-component grid.
    fn gather<T: Copy + Default>(
        grid: &[T],
        w: usize,
        h: usize,
        col_off: usize,
        row_off: usize,
        origin: (u32, u32),
    ) -> Band<T> {
        let bw = (w - col_off.min(w)).div_ceil(2);
        let bh = (h - row_off.min(h)).div_ceil(2);
        let mut data = vec![T::default(); bw * bh];
        for by in 0..bh {
            for bx in 0..bw {
                data[by * bw + bx] = grid[(2 * by + row_off) * w + (2 * bx + col_off)];
            }
        }
        Band {
            origin,
            width: bw,
            height: bh,
            data,
        }
    }

    /// Build the coefficient pyramid for `levels` decompositions of a
    /// tile-component whose top-left sample sits at `(u0, v0)`: forward-transform
    /// at that level's parity, split off the three detail bands at the origins
    /// ISO Eq. B-15 gives them, recurse on the LL. Stores the detail levels
    /// coarsest-first, the order [`inverse`] consumes.
    ///
    /// This is the analysis side of the whole 2-D driver, origin and all: a tile
    /// away from the canvas origin decomposes on odd parities, and a pyramid
    /// built here at an odd origin is exactly what a real encoder would emit for
    /// that tile.
    fn forward_bands_at<T: Copy + Default, F: Fn(&mut [T], usize)>(
        image: &[T],
        w: usize,
        h: usize,
        levels: usize,
        fwd: &F,
        u0: u32,
        v0: u32,
    ) -> Bands<T> {
        let mut data = image.to_vec();
        let (mut cw, mut ch) = (w, h);
        let (mut u, mut v) = (u0, v0);
        let mut details = Vec::new();
        for _ in 0..levels {
            let (cas_x, cas_y) = ((u & 1) as usize, (v & 1) as usize);
            forward_2d(&mut data, cw, ch, cas_x, cas_y, fwd);
            // Eq. B-15: the low-pass band starts at ceil(u/2), the high-pass at
            // floor(u/2) — which is why the two sum back to u in `merge_level`.
            let (lo_x, hi_x) = (u.div_ceil(2), u / 2);
            let (lo_y, hi_y) = (v.div_ceil(2), v / 2);
            let ll = gather(&data, cw, ch, cas_x, cas_y, (lo_x, lo_y));
            let hl = gather(&data, cw, ch, 1 - cas_x, cas_y, (hi_x, lo_y));
            let lh = gather(&data, cw, ch, cas_x, 1 - cas_y, (lo_x, hi_y));
            let hh = gather(&data, cw, ch, 1 - cas_x, 1 - cas_y, (hi_x, hi_y));
            details.push(DetailBands { hl, lh, hh });
            cw = ll.width;
            ch = ll.height;
            (u, v) = (lo_x, lo_y);
            data = ll.data;
        }
        details.reverse();
        Bands {
            ll: Band {
                origin: (u, v),
                width: cw,
                height: ch,
                data,
            },
            levels: details,
        }
    }

    /// [`forward_bands_at`] for a tile-component at the canvas origin.
    fn forward_bands<T: Copy + Default, F: Fn(&mut [T], usize)>(
        image: &[T],
        w: usize,
        h: usize,
        levels: usize,
        fwd: &F,
    ) -> Bands<T> {
        forward_bands_at(image, w, h, levels, fwd, 0, 0)
    }

    /// A small deterministic, non-separable image of the given dimensions.
    fn ramp(w: usize, h: usize) -> Vec<i32> {
        (0..w * h).map(|i| (i as i32 * 7 % 23) - 11).collect()
    }

    /// (width, height, levels): odd and even extents, 0..=3 levels, and the
    /// degenerate single-row / single-column shapes.
    const CASES: [(usize, usize, usize); 10] = [
        (1, 1, 0),
        (6, 4, 0),
        (4, 4, 1),
        (5, 3, 1),
        (9, 1, 1),
        (4, 4, 2),
        (7, 5, 2),
        (3, 9, 2),
        (8, 8, 3),
        (1, 8, 3),
    ];

    /// 5/3 is the lossless path: the full pyramid must reconstruct bit-exactly,
    /// including the zero-level (identity) and single-axis cases.
    #[test]
    fn reconstruct_5_3_bit_exact() {
        for (w, h, levels) in CASES {
            let image = ramp(w, h);
            let bands = forward_bands(&image, w, h, levels, &forward_5_3);
            let hdr = header(Transform::Reversible53, levels as u8, w as u32, h as u32);
            let out = inverse(&hdr, 0, SubbandCoeffs::Reversible(bands)).unwrap();
            assert_eq!(
                out,
                Samples::Reversible(image),
                "5/3 mismatch for {w}x{h}, {levels} levels"
            );
        }
    }

    /// The same pyramid built for a tile-component that does *not* start at the
    /// canvas origin. An odd origin flips the interleave parity of that level —
    /// the low-pass band lands on the odd columns, and index 0 of an interleaved
    /// row is a high-pass sample — which is the case a single tile at the origin
    /// never produces. 5/3 is lossless, so it must still reconstruct bit-exactly.
    ///
    /// The origins keep flipping parity as the pyramid coarsens: 1 and 3 are odd
    /// at the finest level, 2 is even there but odd one level up
    /// (`ceil(2/2) == 1`), and 6 stays even for two levels before turning odd.
    #[test]
    fn reconstruct_5_3_bit_exact_at_odd_origins() {
        for (w, h, levels) in CASES {
            for (u0, v0) in ODD_ORIGINS {
                let image = ramp(w, h);
                let bands = forward_bands_at(&image, w, h, levels, &forward_5_3, u0, v0);
                let hdr = header(Transform::Reversible53, levels as u8, w as u32, h as u32);
                let out = inverse(&hdr, 0, SubbandCoeffs::Reversible(bands)).unwrap();
                assert_eq!(
                    out,
                    Samples::Reversible(image),
                    "5/3 mismatch for {w}x{h}, {levels} levels at origin ({u0}, {v0})"
                );
            }
        }
    }

    /// [`reconstruct_5_3_bit_exact_at_odd_origins`] for the irreversible path:
    /// the 9/7 lifting swaps which parity each of its four steps sweeps, and an
    /// off-by-one there surfaces as reconstruction error, not as a panic.
    #[test]
    fn reconstruct_9_7_within_tolerance_at_odd_origins() {
        for (w, h, levels) in CASES {
            for (u0, v0) in ODD_ORIGINS {
                let image = ramp(w, h);
                let as_f32: Vec<f32> = image.iter().map(|&v| v as f32).collect();
                let bands = forward_bands_at(&as_f32, w, h, levels, &forward_9_7, u0, v0);
                let hdr = header(Transform::Irreversible97, levels as u8, w as u32, h as u32);
                let out = inverse(&hdr, 0, SubbandCoeffs::Irreversible(bands)).unwrap();

                let Samples::Irreversible(values) = out else {
                    panic!("the 9/7 path must carry f32");
                };
                let rounded: Vec<i32> = values.iter().map(|v| v.round_ties_even() as i32).collect();
                assert_eq!(
                    rounded, image,
                    "9/7 mismatch for {w}x{h}, {levels} levels at origin ({u0}, {v0})"
                );
            }
        }
    }

    /// Tile-component origins that flip the interleave parity somewhere in the
    /// pyramid, on one axis or both.
    const ODD_ORIGINS: [(u32, u32); 6] = [(1, 0), (0, 1), (1, 1), (3, 2), (2, 6), (5, 7)];

    /// The band origins a level is built from must add back to that resolution's
    /// own origin — `ceil(u0/2) + floor(u0/2) == u0` — the identity
    /// [`super::merge_level`] recovers the interleave parity from. Pinned here so
    /// a change to the subband geometry cannot quietly cut the DWT's only link to
    /// the tile's coordinate frame.
    #[test]
    fn band_origins_sum_back_to_the_resolution_origin() {
        for (u0, v0) in [(0u32, 0u32), (1, 1), (2, 3), (7, 6), (255, 256)] {
            let image = ramp(8, 8);
            let bands = forward_bands_at(&image, 8, 8, 1, &forward_5_3, u0, v0);
            let level = &bands.levels[0];
            assert_eq!(bands.ll.origin.0 + level.hl.origin.0, u0);
            assert_eq!(bands.ll.origin.1 + level.lh.origin.1, v0);
        }
    }

    /// 9/7 is float and stays float: `inverse` no longer rounds, so the samples
    /// reach `image::assemble` unrounded. Rounding an integer-valued image
    /// through the round-trip still recovers it exactly (the per-sample error
    /// stays far below 0.5).
    #[test]
    fn reconstruct_9_7_within_tolerance() {
        for (w, h, levels) in CASES {
            let image = ramp(w, h);
            let as_f32: Vec<f32> = image.iter().map(|&v| v as f32).collect();
            let bands = forward_bands(&as_f32, w, h, levels, &forward_9_7);
            let hdr = header(Transform::Irreversible97, levels as u8, w as u32, h as u32);
            let out = inverse(&hdr, 0, SubbandCoeffs::Irreversible(bands)).unwrap();

            let Samples::Irreversible(values) = out else {
                panic!("the 9/7 path must carry f32");
            };
            let rounded: Vec<i32> = values.iter().map(|v| v.round_ties_even() as i32).collect();
            assert_eq!(rounded, image, "9/7 mismatch for {w}x{h}, {levels} levels");
        }
    }

    /// The irreversible path hands `assemble` unrounded samples. A decoder that
    /// rounded here would destroy the precision the inverse colour transform
    /// needs (issue #76).
    #[test]
    fn the_irreversible_path_is_not_rounded_by_the_dwt() {
        // A single 1x1 LL band with no decomposition: `inverse` returns it as is.
        let hdr = header(Transform::Irreversible97, 0, 1, 1);
        let bands = Bands {
            ll: Band {
                origin: (0, 0),
                width: 1,
                height: 1,
                data: vec![2.5f32],
            },
            levels: Vec::new(),
        };
        let out = inverse(&hdr, 0, SubbandCoeffs::Irreversible(bands)).unwrap();
        assert_eq!(out, Samples::Irreversible(vec![2.5]));
    }
}
