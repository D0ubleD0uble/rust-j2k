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
//! For subband `b`, the step size (E-3) is
//!
//! ```text
//! Δ_b = (1 + μ_b / 2^11) · 2^(R_I + gain_b − ε_b)
//! ```
//!
//! where `ε_b`/`μ_b` are the QCD exponent/mantissa, `R_I` the component bit
//! depth (SIZ), and `gain_b` the log2 nominal subband gain (LL 0, HL/LH 1,
//! HH 2). Guard bits do not enter the step; they size the magnitude bit-planes,
//! which Tier-1/Tier-2 already consumed.
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
//! The two decoders split `gain_b` differently — see [`apply_band`] — so the
//! step matches OpenJPEG's `band->stepsize` exactly on `LL` and only closely on
//! the detail bands.

use crate::Result;
use crate::codestream::MainHeader;
use crate::codestream::markers::{Qcd, QuantStyle};
use crate::error::Error;
use crate::tier1::{Band, Bands, SubbandCoeffs};

/// 2^11 — the implicit denominator of the 11-bit QCD mantissa.
const MANTISSA_DENOM: f64 = 2048.0;

/// Apply per-subband dequantization in place for component `comp`. Reversible:
/// identity. Irreversible: multiply by half the subband step, because Tier-1
/// hands over magnitudes at twice their weight and already at the interval
/// mid-point. Returns coefficients ready for the inverse DWT.
///
/// The step size depends on the component's declared bit depth, so a
/// multi-component image with mixed depths dequantizes each one on its own
/// scale. The quantization parameters read here are per component: the
/// codestream stage resolves QCD's main-header default and any QCC override
/// before this runs.
pub fn dequantize(
    header: &MainHeader,
    comp: usize,
    coeffs: SubbandCoeffs,
) -> Result<SubbandCoeffs> {
    match coeffs {
        // Reversible (5/3): the integer coefficients are already exact.
        SubbandCoeffs::Reversible(bands) => Ok(SubbandCoeffs::Reversible(bands)),
        SubbandCoeffs::Irreversible(mut bands) => {
            scale_irreversible(header, comp, &mut bands)?;
            Ok(SubbandCoeffs::Irreversible(bands))
        }
    }
}

/// Scale every subband of the 9/7 pyramid by its reconstructed step size. Bands
/// run in QCD subband order: LL first, then each resolution level coarsest-first
/// as `HL, LH, HH` ([`Bands`] stores `levels` coarsest-first to match).
fn scale_irreversible(header: &MainHeader, comp: usize, bands: &mut Bands<f32>) -> Result<()> {
    let prec = i32::from(
        header
            .siz
            .components
            .get(comp)
            .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {comp}")))?
            .bit_depth,
    );
    let qcd = &header
        .components
        .get(comp)
        .ok_or_else(|| Error::Inconsistent(format!("no coding parameters for component {comp}")))?
        .quant;

    // Expounded quantization carries exactly one step per subband (1 LL + 3 per
    // level); a mismatch means the component's quantization and its decomposition
    // depth disagree.
    if qcd.style == QuantStyle::ScalarExpounded {
        let expected = 1 + 3 * bands.levels.len();
        if qcd.steps.len() != expected {
            return Err(Error::Inconsistent(format!(
                "expounded QCD carries {} step sizes, expected {expected} for {} levels",
                qcd.steps.len(),
                bands.levels.len()
            )));
        }
    }

    apply_band(&mut bands.ll, step_params(qcd, 0)?, 0, prec);
    let mut b = 1;
    for level in &mut bands.levels {
        apply_band(&mut level.hl, step_params(qcd, b)?, 1, prec);
        apply_band(&mut level.lh, step_params(qcd, b + 1)?, 1, prec);
        apply_band(&mut level.hh, step_params(qcd, b + 2)?, 2, prec);
        b += 3;
    }
    Ok(())
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
            "QCD carries {} step sizes but subband {band} needs one",
            qcd.steps.len()
        ))
    })
}

/// Scale one subband by half its step size. `gain` is the log2 nominal subband
/// gain (Table E-1: LL 0, HL/LH 1, HH 2).
///
/// The step is computed in `f64` because `R_I` can reach 38 bits (the SIZ
/// depth), so the `2^(R_I + gain − ε)` factor can leave the `f32` range. It is
/// then narrowed and halved the way OpenJPEG does — `(OPJ_FLOAT32)step`, then
/// `0.5f * band->stepsize`. Narrowing first and halving after is exact (halving
/// only decrements the exponent), and multiplying each coefficient by
/// `0.5 · step` in one `f32` operation is a single rounding, as the oracle has.
/// Keeping an `f64` intermediate would be *more* accurate and would round
/// differently near a tie.
///
/// The gain lives here, in the step. OpenJPEG's decoder instead zeroes it
/// (`tcd.c`'s `log2_gain = (!isEncoder && qmfbid == 0) ? 0 : …`) and folds the
/// missing factor of two into the inverse 9/7 as `two_invK`. Its own comment
/// calls that `BUG_WEIRD_TWO_INVK`, and the constant it uses, `1.625732422`,
/// is 3.3e-5 off the true `2/K`. So on the detail bands the two decoders reach
/// the same answer by different routes, and agreement there is close but not
/// bit-for-bit. This crate keeps the exact `1/K`; `p0_09` decodes bit-exact
/// against the ISO reference with it.
fn apply_band(band: &mut Band<f32>, (exp, mant): (u8, u16), gain: i32, prec: i32) {
    let step = (1.0 + f64::from(mant) / MANTISSA_DENOM) * 2f64.powi(prec + gain - i32::from(exp));
    let half_step = 0.5f32 * step as f32;
    for v in &mut band.data {
        *v *= half_step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream::markers::{Cod, Progression, Siz, SizComponent, Transform};
    use crate::tier1::DetailBands;

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
                decomposition_levels: 1,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                use_sop: false,
                use_eph: false,
                multiple_component_transform: false,
                transform: Transform::Irreversible97,
                precinct_sizes: vec![],
            },
            qcd,
        )
    }

    /// A 1×1 float band holding a single coefficient.
    fn band(value: f32) -> Band<f32> {
        Band {
            origin: (0, 0),
            width: 1,
            height: 1,
            data: vec![value],
        }
    }

    /// A one-decomposition-level pyramid: `ll` plus a single detail level.
    fn one_level(ll: f32, hl: f32, lh: f32, hh: f32) -> SubbandCoeffs {
        SubbandCoeffs::Irreversible(Bands {
            ll: band(ll),
            levels: vec![DetailBands {
                hl: band(hl),
                lh: band(lh),
                hh: band(hh),
            }],
        })
    }

    /// The Annex E.1 step size, recomputed independently for the assertions.
    fn step(prec: i32, gain: i32, exp: i32, mant: i32) -> f32 {
        (1.0 + mant as f32 / 2048.0) * 2f32.powi(prec + gain - exp)
    }

    /// One coefficient, as `opj_t1_clbl_decode_processor` computes it:
    /// `(float)datap * (0.5f * band->stepsize)`. `q` is the double-scale
    /// magnitude Tier-1 produces, which already sits at the interval mid-point.
    fn recon(q: f32, step: f32) -> f32 {
        q * (0.5f32 * step)
    }

    fn assert_close(got: f32, want: f32) {
        assert!(
            (got - want).abs() <= 1e-3 * want.abs().max(1.0),
            "got {got}, want {want}"
        );
    }

    /// Pull the irreversible bands out, failing loudly on the wrong arm.
    fn irreversible(coeffs: SubbandCoeffs) -> Bands<f32> {
        match coeffs {
            SubbandCoeffs::Irreversible(b) => b,
            SubbandCoeffs::Reversible(_) => panic!("expected irreversible coefficients"),
        }
    }

    #[test]
    fn reversible_is_identity() {
        let bands = Bands {
            ll: Band {
                origin: (0, 0),
                width: 2,
                height: 1,
                data: vec![3, -7],
            },
            levels: vec![],
        };
        let qcd = Qcd {
            style: QuantStyle::None,
            guard_bits: 2,
            steps: vec![(8, 0)],
        };
        let out = dequantize(&header(8, qcd), 0, SubbandCoeffs::Reversible(bands.clone())).unwrap();
        assert_eq!(out, SubbandCoeffs::Reversible(bands));
    }

    #[test]
    fn expounded_matches_hand_computed() {
        let prec = 8;
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            // (exp, mant) for LL, HL, LH, HH of the single level.
            steps: vec![(8, 0), (7, 512), (7, 512), (6, 1024)],
        };
        let out = irreversible(
            dequantize(&header(prec, qcd), 0, one_level(5.0, -3.0, 0.0, 2.0)).unwrap(),
        );

        assert_close(out.ll.data[0], recon(5.0, step(8, 0, 8, 0)));
        assert_close(out.levels[0].hl.data[0], recon(-3.0, step(8, 1, 7, 512)));
        // A zero index stays exactly zero, no bias applied.
        assert_eq!(out.levels[0].lh.data[0], 0.0);
        assert_close(out.levels[0].hh.data[0], recon(2.0, step(8, 2, 6, 1024)));
    }

    #[test]
    fn derived_drops_exponent_one_per_level() {
        let prec = 8;
        let qcd = Qcd {
            style: QuantStyle::ScalarDerived,
            guard_bits: 1,
            steps: vec![(10, 100)],
        };
        let coeffs = SubbandCoeffs::Irreversible(Bands {
            ll: band(1.0),
            levels: vec![
                // Coarsest level: exponent ε₀ = 10.
                DetailBands {
                    hl: band(1.0),
                    lh: band(1.0),
                    hh: band(1.0),
                },
                // Finer level: exponent drops to 9.
                DetailBands {
                    hl: band(1.0),
                    lh: band(1.0),
                    hh: band(1.0),
                },
            ],
        });
        let out = irreversible(dequantize(&header(prec, qcd), 0, coeffs).unwrap());

        assert_close(out.ll.data[0], recon(1.0, step(8, 0, 10, 100)));
        assert_close(out.levels[0].hl.data[0], recon(1.0, step(8, 1, 10, 100)));
        assert_close(out.levels[1].hl.data[0], recon(1.0, step(8, 1, 9, 100)));
    }

    #[test]
    fn derived_exponent_saturates_at_zero() {
        let qcd = Qcd {
            style: QuantStyle::ScalarDerived,
            guard_bits: 1,
            steps: vec![(1, 0)],
        };
        // Three levels: the finest sits at level index 2, so ε = max(1 − 2, 0) = 0.
        let levels = (0..3)
            .map(|_| DetailBands {
                hl: band(1.0),
                lh: band(1.0),
                hh: band(1.0),
            })
            .collect();
        let coeffs = SubbandCoeffs::Irreversible(Bands {
            ll: band(1.0),
            levels,
        });
        let out = irreversible(dequantize(&header(8, qcd), 0, coeffs).unwrap());
        assert_close(out.levels[2].hl.data[0], recon(1.0, step(8, 1, 0, 0)));
    }

    #[test]
    fn expounded_with_too_few_steps_is_inconsistent() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            // Only the LL step, but a one-level pyramid needs four.
            steps: vec![(8, 0)],
        };
        let err = dequantize(&header(8, qcd), 0, one_level(1.0, 1.0, 1.0, 1.0)).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }

    #[test]
    fn expounded_with_too_many_steps_is_inconsistent() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            // Seven steps (two levels' worth) against a one-level pyramid: the
            // QCD and COD decomposition depth disagree.
            steps: vec![(8, 0); 7],
        };
        let err = dequantize(&header(8, qcd), 0, one_level(1.0, 1.0, 1.0, 1.0)).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }

    #[test]
    fn expounded_walks_two_levels_in_order() {
        let prec = 8;
        // LL, then coarsest HL/LH/HH, then finer HL/LH/HH — each a distinct step
        // so a mis-indexed walk would surface.
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: vec![
                (9, 0),
                (8, 16),
                (8, 32),
                (7, 64),
                (6, 128),
                (6, 256),
                (5, 512),
            ],
        };
        let coeffs = SubbandCoeffs::Irreversible(Bands {
            ll: band(2.0),
            levels: vec![
                DetailBands {
                    hl: band(2.0),
                    lh: band(2.0),
                    hh: band(2.0),
                },
                DetailBands {
                    hl: band(2.0),
                    lh: band(2.0),
                    hh: band(2.0),
                },
            ],
        });
        let out = irreversible(dequantize(&header(prec, qcd), 0, coeffs).unwrap());

        assert_close(out.ll.data[0], recon(2.0, step(8, 0, 9, 0)));
        assert_close(out.levels[0].lh.data[0], recon(2.0, step(8, 1, 8, 32)));
        assert_close(out.levels[0].hh.data[0], recon(2.0, step(8, 2, 7, 64)));
        assert_close(out.levels[1].hl.data[0], recon(2.0, step(8, 1, 6, 128)));
        assert_close(out.levels[1].hh.data[0], recon(2.0, step(8, 2, 5, 512)));
    }

    #[test]
    fn opposite_signs_reconstruct_to_negatives() {
        let qcd = Qcd {
            style: QuantStyle::ScalarExpounded,
            guard_bits: 2,
            steps: vec![(7, 300), (7, 300), (7, 300), (7, 300)],
        };
        let pos = irreversible(
            dequantize(&header(10, qcd.clone()), 0, one_level(6.0, 0.0, 0.0, 0.0)).unwrap(),
        );
        let neg =
            irreversible(dequantize(&header(10, qcd), 0, one_level(-6.0, 0.0, 0.0, 0.0)).unwrap());
        assert_eq!(pos.ll.data[0], -neg.ll.data[0]);
        assert!(pos.ll.data[0] > 0.0);
    }

    #[test]
    fn irreversible_without_scalar_quant_is_inconsistent() {
        let qcd = Qcd {
            style: QuantStyle::None,
            guard_bits: 2,
            steps: vec![(8, 0)],
        };
        let err = dequantize(&header(8, qcd), 0, one_level(1.0, 1.0, 1.0, 1.0)).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }
}
