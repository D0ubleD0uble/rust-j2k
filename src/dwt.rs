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
//! place: the low-pass coefficients sit at the even indices and the high-pass
//! coefficients at the odd indices (the array the standard calls `a(i)` after
//! the implicit deinterleave). On return the slice holds the reconstructed
//! samples. They assume even parity — index 0 is a low-pass sample — which is
//! the only case Phase 1 needs: a single tile at the origin with no precincts.
//! Driving these over the 2-D subband layout for the declared decomposition
//! depth, including the per-subband coordinate parity, is the next milestone
//! (P1.6 wiring in [`inverse`]).

use crate::Result;
use crate::codestream::MainHeader;
use crate::tier1::SubbandCoeffs;

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
/// level count.
pub fn inverse(header: &MainHeader, coeffs: SubbandCoeffs) -> Result<Vec<i32>> {
    todo!("per level: 2-D inverse via 1-D lifting on rows then columns, LL up to full res")
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
fn inverse_5_3(signal: &mut [i32]) {
    let n = signal.len();
    if n <= 1 {
        return;
    }
    // Undo the update step on the even (low-pass) samples first, then undo the
    // predict step on the odd (high-pass) samples — the forward order reversed.
    for i in (0..n).step_by(2) {
        let left = signal[reflect(i as isize - 1, n)];
        let right = signal[reflect(i as isize + 1, n)];
        signal[i] -= (left + right + 2) >> 2;
    }
    for i in (1..n).step_by(2) {
        let left = signal[reflect(i as isize - 1, n)];
        let right = signal[reflect(i as isize + 1, n)];
        signal[i] += (left + right) >> 1;
    }
}

/// One-dimensional inverse 9/7 irreversible lifting over `signal` in place
/// (F.3.8.1): undo the scaling, then the two update/predict lifting pairs in
/// reverse order (`δ` update, `γ` predict, `β` update, `α` predict).
fn inverse_9_7(signal: &mut [f32]) {
    let n = signal.len();
    if n <= 1 {
        return;
    }
    let inv_k = 1.0 / K;
    // Undo scaling: low-pass (even) by K, high-pass (odd) by 1/K.
    for i in (0..n).step_by(2) {
        signal[i] *= K;
    }
    for i in (1..n).step_by(2) {
        signal[i] *= inv_k;
    }
    // Each lifting step adjusts one parity using its two immediate neighbours of
    // the other parity, swept across the whole row before the next step runs.
    lift_step(signal, n, false, -DELTA); // s'_n = s_n - δ(d_{n-1} + d_n)
    lift_step(signal, n, true, -GAMMA); // d'_n = d_n - γ(s'_n + s'_{n+1})
    lift_step(signal, n, false, -BETA); // x_2n  = s'_n - β(d'_{n-1} + d'_n)
    lift_step(signal, n, true, -ALPHA); // x_2n+1 = d'_n - α(x_2n + x_2n+2)
}

/// One 9/7 lifting sweep: add `coeff * (neighbour_left + neighbour_right)` to
/// every sample of the chosen parity (`odd` selects the high-pass positions).
fn lift_step(signal: &mut [f32], n: usize, odd: bool, coeff: f32) {
    let start = usize::from(odd);
    for i in (start..n).step_by(2) {
        let left = signal[reflect(i as isize - 1, n)];
        let right = signal[reflect(i as isize + 1, n)];
        signal[i] += coeff * (left + right);
    }
}

#[cfg(test)]
mod tests {
    use super::{ALPHA, BETA, DELTA, GAMMA, K, inverse_5_3, inverse_9_7, lift_step, reflect};

    /// Forward 5/3 lifting, the exact inverse of [`inverse_5_3`], transcribed
    /// straight from the standard's predict-then-update order. Kept in the test
    /// module so the round-trip checks `inverse_5_3` against an independent
    /// implementation of the forward transform rather than against itself.
    fn forward_5_3(signal: &mut [i32]) {
        let n = signal.len();
        if n <= 1 {
            return;
        }
        for i in (1..n).step_by(2) {
            let left = signal[reflect(i as isize - 1, n)];
            let right = signal[reflect(i as isize + 1, n)];
            signal[i] -= (left + right) >> 1;
        }
        for i in (0..n).step_by(2) {
            let left = signal[reflect(i as isize - 1, n)];
            let right = signal[reflect(i as isize + 1, n)];
            signal[i] += (left + right + 2) >> 2;
        }
    }

    /// Forward 9/7 lifting, the exact inverse of [`inverse_9_7`]: the lifting
    /// pairs in forward order then the reciprocal scaling.
    fn forward_9_7(signal: &mut [f32]) {
        let n = signal.len();
        if n <= 1 {
            return;
        }
        lift_step(signal, n, true, ALPHA);
        lift_step(signal, n, false, BETA);
        lift_step(signal, n, true, GAMMA);
        lift_step(signal, n, false, DELTA);
        let inv_k = 1.0 / K;
        for i in (0..n).step_by(2) {
            signal[i] *= inv_k;
        }
        for i in (1..n).step_by(2) {
            signal[i] *= K;
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

    /// A worked 5/3 vector derived by hand from F.3.8.2, independent of the
    /// forward reference: the interleaved subband array `[10, 0, 33, 10]` (the
    /// forward 5/3 of the ramp `[10, 20, 30, 40]`) inverts back to the ramp.
    #[test]
    fn inverse_5_3_known_vector() {
        let mut a = [10, 0, 33, 10];
        inverse_5_3(&mut a);
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
            let mut a = s.to_vec();
            forward_5_3(&mut a);
            inverse_5_3(&mut a);
            assert_eq!(a, s, "5/3 round-trip mismatch for len {}", s.len());
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
                .map(|i| i as f32 * 1.5 - if i % 2 == 0 { 3.0 } else { -2.0 })
                .collect();
            let mut a = original.clone();
            forward_9_7(&mut a);
            inverse_9_7(&mut a);
            for (got, want) in a.iter().zip(&original) {
                assert!(
                    (got - want).abs() < 1e-3,
                    "9/7 round-trip mismatch at len {len}: got {got}, want {want}",
                );
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
        forward_9_7(&mut a);
        // High-pass (odd) coefficients of a constant signal vanish.
        for i in (1..len).step_by(2) {
            assert!(a[i].abs() < 1e-3, "high-pass coeff {i} = {}", a[i]);
        }
        inverse_9_7(&mut a);
        for &v in &a {
            assert!((v - 5.0).abs() < 1e-3, "DC not preserved: {v}");
        }
    }

    /// Both kernels must handle empty and single-sample slices as identity
    /// without panicking (no high-pass partner to undo).
    #[test]
    fn degenerate_lengths_are_identity() {
        let mut empty_i: [i32; 0] = [];
        inverse_5_3(&mut empty_i);
        let mut one_i = [42];
        inverse_5_3(&mut one_i);
        assert_eq!(one_i, [42]);

        let mut empty_f: [f32; 0] = [];
        inverse_9_7(&mut empty_f);
        let mut one_f = [42.0f32];
        inverse_9_7(&mut one_f);
        assert_eq!(one_f, [42.0]);
    }
}
