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
//! which Tier-1/Tier-2 already consumed. A decoded index `q` reconstructs to the
//! interval mid-point (E.1.1.2, parameter r = ½): `sign(q) · (|q| + ½) · Δ_b`,
//! with zero mapping to zero. Single-layer Phase 1 decodes every bit-plane, so
//! the bias is exactly one half-step.

use crate::Result;
use crate::codestream::MainHeader;
use crate::codestream::markers::{Qcd, QuantStyle};
use crate::error::Error;
use crate::tier1::{Band, Bands, SubbandCoeffs};

/// 2^11 — the implicit denominator of the 11-bit QCD mantissa.
const MANTISSA_DENOM: f64 = 2048.0;

/// Mid-point reconstruction offset r (E.1.1.2). With every bit-plane decoded
/// (single layer), the reconstruction bias is exactly one half-step.
///
/// The standard leaves r a decoder choice; the integration gate (#17) must
/// confirm the OpenJPEG/eccodes oracle also reconstructs at the interval
/// mid-point rather than its lower edge (r = 0), which would be a one-line change.
const RECON_BIAS: f64 = 0.5;

/// Apply per-subband dequantization in place. Reversible: identity. Irreversible:
/// multiply by the subband step (with the standard mid-point reconstruction
/// bias). Returns coefficients ready for the inverse DWT.
pub fn dequantize(header: &MainHeader, coeffs: SubbandCoeffs) -> Result<SubbandCoeffs> {
    match coeffs {
        // Reversible (5/3): the integer coefficients are already exact.
        SubbandCoeffs::Reversible(bands) => Ok(SubbandCoeffs::Reversible(bands)),
        SubbandCoeffs::Irreversible(mut bands) => {
            scale_irreversible(header, &mut bands)?;
            Ok(SubbandCoeffs::Irreversible(bands))
        }
    }
}

/// Scale every subband of the 9/7 pyramid by its reconstructed step size. Bands
/// run in QCD subband order: LL first, then each resolution level coarsest-first
/// as `HL, LH, HH` ([`Bands`] stores `levels` coarsest-first to match).
fn scale_irreversible(header: &MainHeader, bands: &mut Bands<f32>) -> Result<()> {
    let prec = i32::from(
        header
            .siz
            .components
            .first()
            .ok_or_else(|| Error::Inconsistent("SIZ declares no components".into()))?
            .bit_depth,
    );
    let qcd = &header.qcd;

    // Expounded QCDs carry exactly one step per subband (1 LL + 3 per level); a
    // mismatch means the QCD and the COD decomposition depth disagree.
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
    match qcd.style {
        QuantStyle::ScalarExpounded => qcd.steps.get(band).copied().ok_or_else(|| {
            Error::Inconsistent(format!(
                "QCD carries {} step sizes but subband {band} needs one",
                qcd.steps.len()
            ))
        }),
        QuantStyle::ScalarDerived => {
            let (exp0, mant0) = *qcd
                .steps
                .first()
                .ok_or_else(|| Error::Inconsistent("derived QCD carries no step size".into()))?;
            // Subband 0 (LL) keeps ε₀; band b≥1 sits at level ⌊(b−1)/3⌋. The
            // saturating subtraction also makes the b=0 case fall out to level 0.
            let level = band.saturating_sub(1) / 3;
            let drop = u8::try_from(level).unwrap_or(u8::MAX);
            Ok((exp0.saturating_sub(drop), mant0))
        }
        QuantStyle::None => Err(Error::Inconsistent(
            "irreversible transform needs scalar quantization, found QuantStyle::None".into(),
        )),
    }
}

/// Reconstruct one subband's coefficients in place: `sign(q) · (|q| + ½) · Δ`,
/// leaving zeros at zero. `gain` is the log2 nominal subband gain.
///
/// The step and the per-sample product are formed in `f64` and narrowed to
/// `f32` only at the end: `R_I` can reach 38 bits (the SIZ depth), so the
/// `2^(R_I + gain − ε)` factor exceeds the `f32` range, and the wider mantissa
/// keeps `(|q| + ½)·Δ` accurate before the lossy-tolerance comparison. A `-0.0`
/// index is filtered by the `!= 0.0` guard, so `signum` never sees it.
fn apply_band(band: &mut Band<f32>, (exp, mant): (u8, u16), gain: i32, prec: i32) {
    let step = (1.0 + f64::from(mant) / MANTISSA_DENOM) * 2f64.powi(prec + gain - i32::from(exp));
    for v in &mut band.data {
        if *v != 0.0 {
            let q = f64::from(*v);
            *v = (q.signum() * (q.abs() + RECON_BIAS) * step) as f32;
        }
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
        MainHeader {
            siz: Siz {
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
            cod: Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: 1,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                transform: Transform::Irreversible97,
                precinct_sizes: vec![],
            },
            qcd,
        }
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

    /// Mid-point reconstruction of one index, mirroring [`apply_band`].
    fn recon(q: f32, step: f32) -> f32 {
        if q == 0.0 {
            0.0
        } else {
            q.signum() * (q.abs() + 0.5) * step
        }
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
        let out = dequantize(&header(8, qcd), SubbandCoeffs::Reversible(bands.clone())).unwrap();
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
        let out =
            irreversible(dequantize(&header(prec, qcd), one_level(5.0, -3.0, 0.0, 2.0)).unwrap());

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
        let out = irreversible(dequantize(&header(prec, qcd), coeffs).unwrap());

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
        let out = irreversible(dequantize(&header(8, qcd), coeffs).unwrap());
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
        let err = dequantize(&header(8, qcd), one_level(1.0, 1.0, 1.0, 1.0)).unwrap_err();
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
        let err = dequantize(&header(8, qcd), one_level(1.0, 1.0, 1.0, 1.0)).unwrap_err();
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
        let out = irreversible(dequantize(&header(prec, qcd), coeffs).unwrap());

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
            dequantize(&header(10, qcd.clone()), one_level(6.0, 0.0, 0.0, 0.0)).unwrap(),
        );
        let neg =
            irreversible(dequantize(&header(10, qcd), one_level(-6.0, 0.0, 0.0, 0.0)).unwrap());
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
        let err = dequantize(&header(8, qcd), one_level(1.0, 1.0, 1.0, 1.0)).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)));
    }
}
