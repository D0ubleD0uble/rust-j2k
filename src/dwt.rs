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
//!
//! ## The 2-D driver
//!
//! [`inverse`] drives those kernels over the subband layout in [`SubbandCoeffs`].
//! Per resolution level (coarsest first) it scatters the four subbands back into
//! one interleaved grid by the ISO `(xob, yob)` parity — LL to even row / even
//! column, HL to even row / odd column, LH to odd row / even column, HH to odd
//! row / odd column — then runs the 1-D synthesis down every column and across
//! every row. The merged grid is the next finer level's LL; after the last level
//! it is the full-resolution raster (pre level-shift).

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
/// The [`SubbandCoeffs`] arm fixes the arithmetic: reversible 5/3 reconstructs
/// in exact integers, irreversible 9/7 in `f32` then rounds to the nearest
/// integer. Both must agree with the COD transform (checked in debug builds).
pub fn inverse(header: &MainHeader, coeffs: SubbandCoeffs) -> Result<Vec<i32>> {
    match coeffs {
        SubbandCoeffs::Reversible(bands) => {
            debug_assert_eq!(header.cod.transform, Transform::Reversible53);
            debug_assert_eq!(bands.levels.len(), header.cod.decomposition_levels as usize);
            Ok(reconstruct(bands, inverse_5_3).data)
        }
        SubbandCoeffs::Irreversible(bands) => {
            debug_assert_eq!(header.cod.transform, Transform::Irreversible97);
            debug_assert_eq!(bands.levels.len(), header.cod.decomposition_levels as usize);
            let raster = reconstruct(bands, inverse_9_7);
            Ok(raster.data.into_iter().map(|v| v.round() as i32).collect())
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
    F: Fn(&mut [T]),
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
fn merge_level<T, F>(ll: &Band<T>, detail: &DetailBands<T>, kernel: &F) -> Band<T>
where
    T: Copy + Default,
    F: Fn(&mut [T]),
{
    let (hl, lh, hh) = (&detail.hl, &detail.lh, &detail.hh);
    // Phase 1 decodes a single tile at the canvas origin: even parity, so the
    // low-pass bands land on the even rows/columns of the interleaved grid.
    debug_assert!(ll.origin.0.is_multiple_of(2) && ll.origin.1.is_multiple_of(2));
    // The four bands tile the resolution: LL/LH share the low-pass column count,
    // HL/HH the high-pass count; LL/HL share the low-pass row count, LH/HH the
    // high-pass count.
    debug_assert_eq!(hl.height, ll.height);
    debug_assert_eq!(lh.width, ll.width);
    debug_assert_eq!(hh.width, hl.width);
    debug_assert_eq!(hh.height, lh.height);

    let width = ll.width + hl.width;
    let height = ll.height + lh.height;
    let mut grid = vec![T::default(); width * height];
    scatter(&mut grid, width, ll, 0, 0); // LL: even row, even column
    scatter(&mut grid, width, hl, 1, 0); // HL: even row, odd column
    scatter(&mut grid, width, lh, 0, 1); // LH: odd row, even column
    scatter(&mut grid, width, hh, 1, 1); // HH: odd row, odd column

    // Synthesis is separable, but the 5/3 integer lifting rounds, so the two
    // passes do not commute: match OpenJPEG's order — horizontal (rows) first,
    // then vertical (columns) — so the reversible path is bit-exact.
    for row in grid.chunks_exact_mut(width) {
        kernel(row);
    }
    let mut column = vec![T::default(); height];
    for x in 0..width {
        for (y, slot) in column.iter_mut().enumerate() {
            *slot = grid[y * width + x];
        }
        kernel(&mut column);
        for (y, &value) in column.iter().enumerate() {
            grid[y * width + x] = value;
        }
    }

    Band {
        origin: ll.origin,
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
                .map(|i| i as f32 * 1.5 - if i.is_multiple_of(2) { 3.0 } else { -2.0 })
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

    // ---- 2-D multi-level driver ----------------------------------------------
    //
    // Same philosophy as the 1-D round-trips above: an independent forward 2-D
    // transform (rows then columns, then deinterleave into the four subbands)
    // builds the coefficient pyramid that `inverse` must take back to the image.

    use super::inverse;
    use crate::codestream::MainHeader;
    use crate::codestream::markers::{
        Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform,
    };
    use crate::tier1::{Band, Bands, DetailBands, SubbandCoeffs};

    /// A minimal single-component main header. The inverse reads only the
    /// transform choice and the decomposition-level count from it (both checked
    /// against the pyramid in debug builds); the rest is filler.
    fn header(transform: Transform, levels: u8, w: u32, h: u32) -> MainHeader {
        MainHeader {
            siz: Siz {
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
            cod: Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: levels,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                transform,
                precinct_sizes: Vec::new(),
            },
            qcd: Qcd {
                style: QuantStyle::None,
                guard_bits: 2,
                steps: Vec::new(),
            },
        }
    }

    /// Forward 1-D kernel down every column then across every row, in place —
    /// the analysis counterpart of [`super::merge_level`]'s rows-then-columns
    /// synthesis (the passes must run in the reverse order to round-trip).
    fn forward_2d<T: Copy + Default, F: Fn(&mut [T])>(grid: &mut [T], w: usize, h: usize, fwd: &F) {
        let mut col = vec![T::default(); h];
        for x in 0..w {
            for (y, slot) in col.iter_mut().enumerate() {
                *slot = grid[y * w + x];
            }
            fwd(&mut col);
            for (y, &v) in col.iter().enumerate() {
                grid[y * w + x] = v;
            }
        }
        for row in grid.chunks_exact_mut(w) {
            fwd(row);
        }
    }

    /// Deinterleave one `(col_off, row_off)` parity quadrant of a transformed
    /// grid into a subband.
    fn gather<T: Copy + Default>(
        grid: &[T],
        w: usize,
        h: usize,
        col_off: usize,
        row_off: usize,
    ) -> Band<T> {
        let bw = (w - col_off).div_ceil(2);
        let bh = (h - row_off).div_ceil(2);
        let mut data = vec![T::default(); bw * bh];
        for by in 0..bh {
            for bx in 0..bw {
                data[by * bw + bx] = grid[(2 * by + row_off) * w + (2 * bx + col_off)];
            }
        }
        Band {
            origin: (0, 0),
            width: bw,
            height: bh,
            data,
        }
    }

    /// Build the coefficient pyramid for `levels` decompositions: forward-
    /// transform, split off the three detail bands, recurse on the LL. Stores
    /// the detail levels coarsest-first, the order [`inverse`] consumes.
    fn forward_bands<T: Copy + Default, F: Fn(&mut [T])>(
        image: &[T],
        w: usize,
        h: usize,
        levels: usize,
        fwd: &F,
    ) -> Bands<T> {
        let mut data = image.to_vec();
        let (mut cw, mut ch) = (w, h);
        let mut details = Vec::new();
        for _ in 0..levels {
            forward_2d(&mut data, cw, ch, fwd);
            let ll = gather(&data, cw, ch, 0, 0);
            let hl = gather(&data, cw, ch, 1, 0);
            let lh = gather(&data, cw, ch, 0, 1);
            let hh = gather(&data, cw, ch, 1, 1);
            details.push(DetailBands { hl, lh, hh });
            cw = ll.width;
            ch = ll.height;
            data = ll.data;
        }
        details.reverse();
        Bands {
            ll: Band {
                origin: (0, 0),
                width: cw,
                height: ch,
                data,
            },
            levels: details,
        }
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
            let out = inverse(&hdr, SubbandCoeffs::Reversible(bands)).unwrap();
            assert_eq!(out, image, "5/3 mismatch for {w}x{h}, {levels} levels");
        }
    }

    /// 9/7 is float, but rounding an integer-valued image through the round-trip
    /// recovers it exactly (the per-sample error stays far below 0.5), which also
    /// exercises the final round-to-`i32`.
    #[test]
    fn reconstruct_9_7_within_tolerance() {
        for (w, h, levels) in CASES {
            let image = ramp(w, h);
            let as_f32: Vec<f32> = image.iter().map(|&v| v as f32).collect();
            let bands = forward_bands(&as_f32, w, h, levels, &forward_9_7);
            let hdr = header(Transform::Irreversible97, levels as u8, w as u32, h as u32);
            let out = inverse(&hdr, SubbandCoeffs::Irreversible(bands)).unwrap();
            assert_eq!(out, image, "9/7 mismatch for {w}x{h}, {levels} levels");
        }
    }
}
