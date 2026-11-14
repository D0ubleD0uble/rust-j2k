//! Stage 4 — dequantization (ISO/IEC 15444-1 Annex E.1).
//!
//! Maps the quantized integers from Tier-1 back to wavelet-coefficient
//! magnitudes before the inverse transform. For the **reversible** path (5/3,
//! `QuantStyle::None`) there is no scaling — only the implicit bit-shift from
//! the decoded bit-planes — and the inverse must stay exact. For the
//! **irreversible** path (9/7) each subband has a scalar step size, derived or
//! expounded, reconstructed from the QCD/QCC (exponent, mantissa) and the
//! number of guard bits.
//!
//! For subband `b`, the ISO step size (E-3) is
//!
//! ```text
//! Δ_b = (1 + μ_b / 2^11) · 2^(R_I + gain_b − ε_b)
//! ```
//!
//! where `ε_b`/`μ_b` are the QCD exponent/mantissa, `R_I` the component bit
//! depth (SIZ), and `gain_b` the log2 nominal subband gain (LL 0, HL/LH 1,
//! HH 2). Guard bits do not enter the step; they size the magnitude bit-planes,
//! which Tier-1/Tier-2 already consumed. The `gain_b` term is **not** applied
//! here — the inverse 9/7 carries it, one factor of two per high-pass filtering
//! (see [`half_step_of`] and [`crate::dwt`]).
//!
//! **The mid-point reconstruction (E.1.1.2, r = ½) is not applied here.**
//! Tier-1 already carries it: a coefficient that becomes significant at plane
//! `b` is set to `2^b + 2^(b−1)`, which *is* the interval mid-point, and every
//! refinement keeps the running half. Those magnitudes arrive at twice their
//! weight, so the coefficient is
//!
//! ```text
//! v = q_double · (Δ_b / 2)
//! ```
//!
//! which is `opj_t1_clbl_decode_processor`'s `stepsize = 0.5f * band->stepsize;
//! tmp = (float)datap * stepsize`. Adding a further `± ½ · Δ_b` here would bias
//! every coefficient by a half step, and halving `q_double` with an integer
//! divide first would silently cancel that bias for odd `q` while leaving it for
//! even `q` — an easy pair of mistakes to make together, because they look
//! correct on any coefficient that was coded to its last bit-plane.
//!
//! The step here therefore matches OpenJPEG's gain-free 9/7 decode step on
//! every band; the gain moves to the inverse transform (see [`half_step_of`]).

use crate::Result;
use crate::codestream::MainHeader;
use crate::codestream::markers::{Qcd, QuantStyle};
use crate::error::Error;

/// 2^11 — the implicit denominator of the 11-bit QCD mantissa.
const MANTISSA_DENOM: f64 = 2048.0;

/// The factor that turns one decoded 9/7 index of subband `band` of component
/// `comp` into a coefficient: half the subband step, because Tier-1 hands over
/// magnitudes at twice their weight and already at the interval mid-point.
/// Tier-1 multiplies each index by it as it places the block, one `f32`
/// multiply per coefficient. (The 5/3 path has no scaling.)
///
/// `band` is the subband index in QCD order: LL is 0, then each resolution
/// level coarsest-first as `HL, LH, HH`.
///
/// The step depends on the component's declared bit depth, so a
/// multi-component image with mixed depths dequantizes each one on its own
/// scale. The quantization parameters read here are per component: the
/// codestream stage resolves QCD's main-header default and any QCC override
/// before this runs. A QCD without a step for `band` is an error, so a
/// pyramid deeper than its steps is refused, as before.
pub fn half_step(header: &MainHeader, comp: usize, band: usize) -> Result<f32> {
    let prec = i32::from(
        header
            .siz
            .components
            .get(comp)
            .ok_or_else(|| Error::Inconsistent(format!("`SIZ` declares no component {comp}")))?
            .bit_depth,
    );
    let qcd = &header
        .components
        .get(comp)
        .ok_or_else(|| Error::Inconsistent(format!("no coding parameters for component {comp}")))?
        .quant;
    Ok(half_step_of(step_params(qcd, band)?, prec))
}

/// The `(exponent, mantissa)` pair for subband index `band`. Expounded styles
/// read it straight from the QCD; the derived style stores only subband 0 and
/// drops the exponent by one per resolution level finer (E-5, OpenJPEG's
/// `ε_b = max(ε_0 − ⌊(b−1)/3⌋, 0)`), keeping the single mantissa.
fn step_params(qcd: &Qcd, band: usize) -> Result<(u8, u16)> {
    if qcd.style == QuantStyle::None {
        return Err(Error::Inconsistent(
            "irreversible transform needs scalar quantization, found QuantStyle::None".into(),
        ));
    }
    // The per-subband exponent/mantissa mapping (expounded lookup or derived
    // per-level drop) is shared with Tier-1's bit-plane count so the two stay
    // numerically identical.
    qcd.subband_step(band).ok_or_else(|| {
        Error::Inconsistent(format!(
            "`QCD` carries {} step sizes but subband {band} needs one",
            qcd.steps.len()
        ))
    })
}

/// Half the step size of one subband.
///
/// The step is computed in `f64` because `R_I` can reach 38 bits (the SIZ
/// depth), so the `2^(R_I − ε)` factor can leave the `f32` range. It is then
/// narrowed and halved the way OpenJPEG does — `(OPJ_FLOAT32)step`, then
/// `0.5f * band->stepsize`. Narrowing first and halving after is exact (halving
/// only decrements the exponent), and multiplying each coefficient by
/// `0.5 · step` in one `f32` operation is a single rounding, as the oracle has.
/// Keeping an `f64` intermediate would be *more* accurate and would round
/// differently near a tie.
///
/// The step deliberately omits the log2 nominal subband gain (Table E-1: LL 0,
/// HL/LH 1, HH 2). OpenJPEG omits it too for the 9/7 decode (`tcd.c`'s
/// `log2_gain = (!isEncoder && qmfbid == 0) ? 0 : …`) and folds the gain into
/// the inverse 9/7 instead, one factor of two per high-pass filtering. This
/// crate does the same in [`crate::dwt`] (high-pass reconstruction scaled by
/// `2/K` rather than `1/K`). Placing the gain per-filter, not per-subband, is
/// what makes it correct on a tile-component whose coarse resolutions are empty:
/// there a coefficient can be high-pass a different number of times than its
/// nominal subband gain, and a fixed `2^gain` in the step would mis-scale it
/// (see #126). `2/K` is applied as an exact `2 · (1/K)`, so for the ordinary
/// case — where the high-pass count equals the gain — the result is bit-for-bit
/// what the old `2^gain`-in-step form produced, and `p0_09` stays exact.
fn half_step_of((exp, mant): (u8, u16), prec: i32) -> f32 {
    let step = (1.0 + f64::from(mant) / MANTISSA_DENOM) * 2f64.powi(prec - i32::from(exp));
    0.5f32 * step as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream::markers::{Cod, Coding, Progression, Siz, SizComponent, Transform};

    /// A `prec`-bit single-component header carrying `qcd`. Only the component
    /// depth and the QCD feed dequantization; the rest is filler.
    fn header(prec: u8, qcd: Qcd) -> MainHeader {
        MainHeader::new(
            Siz {
                x_size: 4,
                y_size: 4,
                x_offset: 0,
                y_offset: 0,
                tile_width: 4,
                tile_height: 4,
                tile_x_offset: 0,
                tile_y_offset: 0,
                components: vec![SizComponent {
                    bit_depth: prec,
                    signed: false,
                    x_sampling: 1,
                    y_sampling: 1,
                }],
            },
            Cod {
                progression: Progression::Lrcp,
                layers: 1,
                use_sop: false,
                use_eph: false,
                multiple_component_transform: false,
                coding: Coding {
                    decomposition_levels: 1,
                    code_block_width: 4,
                    code_block_height: 4,
                    code_block_style: 0,
                    transform: Transform::Irreversible97,
                    precinct_sizes: vec![],
                },
            },
            qcd,
        )
    }

    /// The Annex E.1 step size, recomputed independently for the assertions.
    /// The gain is not part of the step (dequantization omits it; the inverse
    /// 9/7 carries it, see [`half_step_of`]), so it does not appear here.
    fn step(prec: i32, exp: i32, mant: i32) -> f32 {
        (1.0 + mant as f32 / 2048.0) * 2f32.powi(prec - exp)
    }

    /// The factor `opj_t1_clbl_decode_processor` multiplies by:
    /// `0.5f * band->stepsize`.
    fn want(step: f32) -> f32 {
        0.5f32 * step
    }

    fn assert_close(got: f32, want: f32) {
        assert!(
            (got - want).abs() <= 1e-3 * want.abs().max(1.0),
            "got {got}, want {want}"
        );
    }

    #[test]
    fn expounded_matches_hand_computed() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            // (exp, mant) for LL, HL, LH, HH of the single level.
            steps: vec![(8, 0), (7, 512), (7, 512), (6, 1024)],
        };
        let h = header(8, qcd);
        assert_close(half_step(&h, 0, 0).unwrap(), want(step(8, 8, 0)));
        assert_close(half_step(&h, 0, 1).unwrap(), want(step(8, 7, 512)));
        assert_close(half_step(&h, 0, 2).unwrap(), want(step(8, 7, 512)));
        assert_close(half_step(&h, 0, 3).unwrap(), want(step(8, 6, 1024)));
    }

    /// The factor is the exact operand the oracle multiplies by, not just a
    /// close value: narrow the `f64` step, then halve.
    #[test]
    fn half_step_is_the_narrowed_step_halved() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: vec![(9, 1234)],
        };
        let exact = (1.0 + 1234.0 / 2048.0) * 2f64.powi(12 - 9);
        assert_eq!(
            half_step(&header(12, qcd), 0, 0).unwrap().to_bits(),
            (0.5f32 * exact as f32).to_bits()
        );
    }

    #[test]
    fn derived_drops_exponent_one_per_level() {
        let qcd = Qcd {
            style: QuantStyle::ScalarDerived,
            guard_bits: 1,
            steps: vec![(10, 100)],
        };
        let h = header(8, qcd);
        assert_close(half_step(&h, 0, 0).unwrap(), want(step(8, 10, 100)));
        // Coarsest level: exponent 10. Finer level: 9.
        assert_close(half_step(&h, 0, 1).unwrap(), want(step(8, 10, 100)));
        assert_close(half_step(&h, 0, 4).unwrap(), want(step(8, 9, 100)));
    }

    #[test]
    fn derived_exponent_saturates_at_zero() {
        let qcd = Qcd {
            style: QuantStyle::ScalarDerived,
            guard_bits: 1,
            steps: vec![(1, 0)],
        };
        // The finest of three levels is band 7: exponent max(1 - 2, 0) = 0.
        assert_close(
            half_step(&header(8, qcd), 0, 7).unwrap(),
            want(step(8, 0, 0)),
        );
    }

    #[test]
    fn expounded_walks_two_levels_in_order() {
        // LL, then coarsest HL/LH/HH, then finer HL/LH/HH, each a distinct
        // step so a mis-indexed walk would surface.
        let steps = [
            (9, 0),
            (8, 16),
            (8, 32),
            (7, 64),
            (6, 128),
            (6, 256),
            (5, 512),
        ];
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: steps.to_vec(),
        };
        let h = header(8, qcd);
        for (band, (exp, mant)) in steps.into_iter().enumerate() {
            assert_close(
                half_step(&h, 0, band).unwrap(),
                want(step(8, i32::from(exp), i32::from(mant))),
            );
        }
    }

    #[test]
    fn a_band_without_a_step_is_inconsistent() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: vec![(8, 0)],
        };
        let err = half_step(&header(8, qcd), 0, 1).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }

    #[test]
    fn excess_steps_are_ignored() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: vec![(8, 0); 7],
        };
        half_step(&header(8, qcd), 0, 3)
            .expect("excess entries are padding, as OpenJPEG treats them");
    }

    #[test]
    fn irreversible_without_scalar_quant_is_inconsistent() {
        let qcd = Qcd {
            style: QuantStyle::None,
            guard_bits: 2,
            steps: vec![(8, 0)],
        };
        let err = half_step(&header(8, qcd), 0, 0).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }

    #[test]
    fn an_unknown_component_is_inconsistent() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: vec![(8, 0)],
        };
        let err = half_step(&header(8, qcd), 1, 0).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }
}
