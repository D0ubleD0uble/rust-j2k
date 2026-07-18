//! Stage 6 — inverse multiple component transform (ISO/IEC 15444-1 Annex G.2).
//!
//! When `COD`'s `SGcod` multiple-component-transform byte is set, the first
//! three components of a tile are not independent: the encoder decorrelated them
//! before the wavelet. Inverting that is the last step before the DC level shift
//! (§G.1), so it runs on the reconstructed sample grids, not on coefficients.
//!
//! Which transform to invert is **not** a separate flag. The wavelet chooses it:
//! the reversible 5/3 path pairs with the reversible colour transform (RCT,
//! §G.2), the irreversible 9/7 path with the irreversible one (ICT, §G.3).
//! OpenJPEG makes the same choice on `qmfbid == 1` in `opj_tcd_mct_decode`,
//! reading the wavelet off component 0. [`inverse_rct`] inverts the 5/3 arm in
//! exact integers; [`inverse_ict`] inverts the 9/7 arm in floats.
//!
//! Components past the third pass through untouched.

use crate::pipeline::Samples;
use crate::{Error, Result};

/// Invert the reversible colour transform over the first three components
/// (§G.2, equations G-6 to G-8):
///
/// ```text
/// G = Y0 - floor((Y1 + Y2) / 4)
/// R = Y2 + G
/// B = Y1 + G
/// ```
///
/// The division floors rather than truncates toward zero — it is an arithmetic
/// right shift by 2 — so a negative `Y1 + Y2` rounds down. Truncating instead
/// would be off by one for roughly half the negative sums, and RCT is meant to
/// be exactly invertible.
///
/// The arithmetic is done in `i64`. The samples arriving here are wavelet
/// reconstructions, not yet clamped to the component depth, so `Y1 + Y2` and
/// `Y2 + G` can leave `i32` on a hostile codestream; a debug build would panic
/// on the overflow. Values are saturated back into `i32`, and `image::assemble`
/// clamps them to the declared depth immediately afterwards.
///
/// `components` must hold at least three equally sized sample vectors on the
/// reversible arm; [`check_geometry`] and `decode_cod` are what guarantee it —
/// the colour transform is only RCT when the wavelet is 5/3, and the 5/3 path
/// reconstructs in integers.
pub fn inverse_rct(components: &mut [Samples]) -> Result<()> {
    let count = components.len();
    let [c0, c1, c2, ..] = components else {
        return Err(Error::Inconsistent(format!(
            "the colour transform needs three components, found {count}"
        )));
    };
    let [
        Samples::Reversible(c0),
        Samples::Reversible(c1),
        Samples::Reversible(c2),
    ] = [c0, c1, c2]
    else {
        return Err(Error::Inconsistent(
            "the reversible colour transform needs integer samples; the 9/7 path is ICT".into(),
        ));
    };
    if c0.len() != c1.len() || c0.len() != c2.len() {
        return Err(Error::Inconsistent(
            "the colour transform needs three components of equal size".into(),
        ));
    }

    for ((y0, y1), y2) in c0.iter_mut().zip(c1.iter_mut()).zip(c2.iter_mut()) {
        let (y, u, v) = (i64::from(*y0), i64::from(*y1), i64::from(*y2));
        let g = y - ((u + v) >> 2);
        *y0 = saturate(v + g); // R
        *y1 = saturate(g); // G
        *y2 = saturate(u + g); // B
    }
    Ok(())
}

/// Invert the irreversible colour transform over the first three components
/// (§G.3, equations G-9 to G-11):
///
/// ```text
/// R = Y + 1.402·Cr
/// G = Y − 0.34413·Cb − 0.71414·Cr
/// B = Y + 1.772·Cb
/// ```
///
/// The samples are floats — the 9/7 arm reconstructs in floating point — so the
/// transform is not exactly invertible, and a decode is graded against the
/// reference within a compliance-class bound rather than bit-exactly. To land
/// inside that bound, the constants and the left-to-right evaluation order match
/// OpenJPEG's `opj_mct_decode_real` exactly (`0.34413f`, `0.71414f`, not the
/// spec's fuller `0.344136`/`0.714136`), so the rounding follows the oracle's.
///
/// `components` must hold at least three equally sized float sample vectors;
/// [`check_geometry`] and the wavelet check in `parse_main_header` guarantee it —
/// the colour transform is ICT only when the wavelet is 9/7, and the 9/7 path
/// reconstructs in floats.
pub fn inverse_ict(components: &mut [Samples]) -> Result<()> {
    let count = components.len();
    let [c0, c1, c2, ..] = components else {
        return Err(Error::Inconsistent(format!(
            "the colour transform needs three components, found {count}"
        )));
    };
    let [
        Samples::Irreversible(c0),
        Samples::Irreversible(c1),
        Samples::Irreversible(c2),
    ] = [c0, c1, c2]
    else {
        return Err(Error::Inconsistent(
            "the irreversible colour transform needs float samples; the 5/3 path is RCT".into(),
        ));
    };
    if c0.len() != c1.len() || c0.len() != c2.len() {
        return Err(Error::Inconsistent(
            "the colour transform needs three components of equal size".into(),
        ));
    }

    for ((y0, y1), y2) in c0.iter_mut().zip(c1.iter_mut()).zip(c2.iter_mut()) {
        let (y, cb, cr) = (*y0, *y1, *y2);
        *y0 = y + 1.402 * cr; // R
        *y1 = y - 0.34413 * cb - 0.71414 * cr; // G
        *y2 = y + 1.772 * cb; // B
    }
    Ok(())
}

/// Narrow to `i32` without wrapping. The sample is clamped again to its declared
/// depth in `image::assemble`, so saturating here only bounds the intermediate.
/// Shared with the 5/3 lifting in [`crate::dwt`], which faces the same
/// hostile-coefficient headroom problem.
pub(crate) fn saturate(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Whether the three transformed components can be inverted together.
///
/// §G.2 applies the transform to components 0, 1 and 2, which must therefore
/// share a sample grid. OpenJPEG *skips* the transform when a tile has fewer
/// than three components and errors when their dimensions disagree; skipping
/// would silently decode an image the encoder never wrote, so both cases are
/// rejected here.
///
/// The bit depths must agree as well: the transform mixes the three components
/// arithmetically, so a mismatch means the codestream is describing something
/// the transform cannot express.
pub fn check_geometry(components: &[crate::codestream::markers::SizComponent]) -> Result<()> {
    let [c0, c1, c2, ..] = components else {
        return Err(Error::Marker(format!(
            "COD signals the colour transform but SIZ declares {} component(s); \
             it is defined over the first three",
            components.len()
        )));
    };
    for (index, comp) in [c1, c2].into_iter().enumerate() {
        if (comp.bit_depth, comp.signed) != (c0.bit_depth, c0.signed) {
            return Err(Error::Marker(format!(
                "COD signals the colour transform but component {} is {}-bit (signed {}) \
                 against component 0's {}-bit (signed {})",
                index + 1,
                comp.bit_depth,
                comp.signed,
                c0.bit_depth,
                c0.signed,
            )));
        }
        if (comp.x_sampling, comp.y_sampling) != (c0.x_sampling, c0.y_sampling) {
            return Err(Error::Marker(format!(
                "COD signals the colour transform but component {} sub-samples ({}, {}) \
                 against component 0's ({}, {})",
                index + 1,
                comp.x_sampling,
                comp.y_sampling,
                c0.x_sampling,
                c0.y_sampling,
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream::markers::SizComponent;

    /// The forward RCT of §G.2 (equations G-1 to G-3), for round-tripping the
    /// inverse against the transform the standard actually defines.
    ///
    /// ```text
    /// Y0 = floor((R + 2G + B) / 4)
    /// Y1 = B - G
    /// Y2 = R - G
    /// ```
    fn forward_rct(r: i32, g: i32, b: i32) -> (i32, i32, i32) {
        let (r, g, b) = (i64::from(r), i64::from(g), i64::from(b));
        (
            ((r + 2 * g + b) >> 2) as i32,
            (b - g) as i32,
            (r - g) as i32,
        )
    }

    /// Wrap integer samples on the reversible arm, which is the only arm RCT
    /// ever sees.
    fn reversible(values: &[&[i32]]) -> Vec<Samples> {
        values
            .iter()
            .map(|v| Samples::Reversible(v.to_vec()))
            .collect()
    }

    fn plain(components: &[Samples]) -> Vec<Vec<i32>> {
        components
            .iter()
            .map(|c| match c {
                Samples::Reversible(v) => v.clone(),
                Samples::Irreversible(_) => panic!("RCT never sees the 9/7 arm"),
            })
            .collect()
    }

    fn invert(y0: i32, y1: i32, y2: i32) -> (i32, i32, i32) {
        let mut components = reversible(&[&[y0], &[y1], &[y2]]);
        inverse_rct(&mut components).expect("three equal components");
        let out = plain(&components);
        (out[0][0], out[1][0], out[2][0])
    }

    /// The standard's own worked shape: the inverse must undo the forward
    /// transform exactly, which is the whole point of the *reversible* one.
    #[test]
    fn inverse_undoes_the_forward_transform_exactly() {
        for r in [0, 1, 17, 128, 255] {
            for g in [0, 3, 64, 200, 255] {
                for b in [0, 7, 99, 254, 255] {
                    let (y0, y1, y2) = forward_rct(r, g, b);
                    assert_eq!(invert(y0, y1, y2), (r, g, b), "RGB ({r}, {g}, {b})");
                }
            }
        }
    }

    /// Signed components round-trip too: the level shift has not been applied
    /// yet, so negative samples reach the transform.
    #[test]
    fn inverse_round_trips_negative_samples() {
        for r in [-128, -1, 0, 127] {
            for g in [-128, -5, 0, 127] {
                for b in [-128, 0, 3, 127] {
                    let (y0, y1, y2) = forward_rct(r, g, b);
                    assert_eq!(invert(y0, y1, y2), (r, g, b), "RGB ({r}, {g}, {b})");
                }
            }
        }
    }

    /// `(Y1 + Y2) / 4` must floor. Truncating toward zero gives `G` one too
    /// large whenever the sum is negative and not a multiple of four, which
    /// breaks reversibility on exactly the samples a colour image is full of.
    #[test]
    fn the_divide_by_four_floors_rather_than_truncates() {
        // Y1 + Y2 = -1: floor(-1/4) = -1 gives G = 1, so (R, G, B) = (1, 1, 0).
        // Truncation toward zero would give G = 0, hence (0, 0, -1).
        assert_eq!(invert(0, -1, 0), (1, 1, 0));
        // Y1 + Y2 = -5: floor(-5/4) = -2 gives G = 2. Truncation gives G = 1.
        assert_eq!(invert(0, -3, -2), (0, 2, -1));
        // A positive sum agrees either way, so it cannot catch the bug.
        assert_eq!(invert(0, 3, 2), (1, -1, 2));
    }

    #[test]
    fn transform_applies_only_to_the_first_three_components() {
        let mut components = reversible(&[&[0], &[0], &[0], &[42], &[-7]]);
        inverse_rct(&mut components).unwrap();
        let out = plain(&components);
        assert_eq!(out[3], vec![42]);
        assert_eq!(out[4], vec![-7]);
    }

    #[test]
    fn extreme_samples_saturate_rather_than_overflow() {
        // `Y1 + Y2` and `Y2 + G` both leave `i32` here; a wrapping build would
        // panic in debug and produce nonsense in release.
        let mut components = reversible(&[&[i32::MAX], &[i32::MAX], &[i32::MAX]]);
        inverse_rct(&mut components).expect("saturates");
        let mut components = reversible(&[&[i32::MIN], &[i32::MIN], &[i32::MIN]]);
        inverse_rct(&mut components).expect("saturates");
    }

    /// The forward ICT of §G.3 (equations G-4 to G-6): the transform the inverse
    /// must undo, within the float round-trip tolerance.
    ///
    /// ```text
    /// Y  =  0.299·R + 0.587·G + 0.114·B
    /// Cb = -0.16875·R − 0.33126·G + 0.5·B
    /// Cr =  0.5·R − 0.41869·G − 0.08131·B
    /// ```
    fn forward_ict(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        (
            0.299 * r + 0.587 * g + 0.114 * b,
            -0.16875 * r - 0.33126 * g + 0.5 * b,
            0.5 * r - 0.41869 * g - 0.08131 * b,
        )
    }

    fn irreversible(values: &[&[f32]]) -> Vec<Samples> {
        values
            .iter()
            .map(|v| Samples::Irreversible(v.to_vec()))
            .collect()
    }

    fn floats(components: &[Samples]) -> Vec<Vec<f32>> {
        components
            .iter()
            .map(|c| match c {
                Samples::Irreversible(v) => v.clone(),
                Samples::Reversible(_) => panic!("ICT never sees the 5/3 arm"),
            })
            .collect()
    }

    fn invert_ict(y: f32, cb: f32, cr: f32) -> (f32, f32, f32) {
        let mut components = irreversible(&[&[y], &[cb], &[cr]]);
        inverse_ict(&mut components).expect("three equal components");
        let out = floats(&components);
        (out[0][0], out[1][0], out[2][0])
    }

    /// The inverse ICT undoes the forward transform to within the float
    /// round-trip error; unlike RCT it is not exact, so this is a tolerance
    /// check, not equality.
    #[test]
    fn inverse_ict_undoes_the_forward_transform_within_tolerance() {
        for r in [0.0, 1.0, 17.0, 128.0, 255.0] {
            for g in [0.0, 3.0, 64.0, 200.0, 255.0] {
                for b in [0.0, 7.0, 99.0, 254.0, 255.0] {
                    let (y, cb, cr) = forward_ict(r, g, b);
                    let (r2, g2, b2) = invert_ict(y, cb, cr);
                    // The forward/inverse constant pairs are rounded independently
                    // (OpenJPEG's `0.34413` is not the exact inverse of `0.33126`),
                    // so a few tenths of drift is expected and within any
                    // compliance-class bound.
                    assert!(
                        (r2 - r).abs() < 0.5 && (g2 - g).abs() < 0.5 && (b2 - b).abs() < 0.5,
                        "RGB ({r}, {g}, {b}) round-tripped to ({r2}, {g2}, {b2})",
                    );
                }
            }
        }
    }

    /// The exact inverse-ICT constants and evaluation order match OpenJPEG's
    /// `opj_mct_decode_real`, so a fixed input reproduces the oracle's output.
    #[test]
    fn inverse_ict_matches_the_reference_formula() {
        // R = Y + 1.402·Cr; G = Y − 0.34413·Cb − 0.71414·Cr; B = Y + 1.772·Cb.
        let (r, g, b) = invert_ict(100.0, 20.0, -30.0);
        assert!((r - (100.0 + 1.402 * -30.0)).abs() < 1e-4);
        assert!((g - (100.0 - 0.34413 * 20.0 - 0.71414 * -30.0)).abs() < 1e-4);
        assert!((b - (100.0 + 1.772 * 20.0)).abs() < 1e-4);
    }

    #[test]
    fn ict_applies_only_to_the_first_three_components() {
        let mut components = irreversible(&[&[0.0], &[0.0], &[0.0], &[42.0], &[-7.0]]);
        inverse_ict(&mut components).unwrap();
        let out = floats(&components);
        assert_eq!(out[3], vec![42.0]);
        assert_eq!(out[4], vec![-7.0]);
    }

    #[test]
    fn inverse_ict_rejects_the_integer_arm() {
        // RCT samples on the ICT path: the wavelet/transform pairing is broken.
        let mut components = reversible(&[&[1], &[2], &[3]]);
        assert!(matches!(
            inverse_ict(&mut components),
            Err(Error::Inconsistent(_))
        ));
    }

    #[test]
    fn fewer_than_three_components_is_inconsistent() {
        let mut components = reversible(&[&[1], &[2]]);
        assert!(matches!(
            inverse_rct(&mut components),
            Err(Error::Inconsistent(_))
        ));
    }

    #[test]
    fn unequal_component_sizes_are_inconsistent() {
        let mut components = reversible(&[&[1, 2], &[3], &[4, 5]]);
        assert!(matches!(
            inverse_rct(&mut components),
            Err(Error::Inconsistent(_))
        ));
    }

    // --- geometry validation -------------------------------------------------

    fn component(bit_depth: u8, signed: bool, x_sampling: u8, y_sampling: u8) -> SizComponent {
        SizComponent {
            bit_depth,
            signed,
            x_sampling,
            y_sampling,
        }
    }

    #[test]
    fn three_matching_components_pass() {
        let comps = vec![component(8, false, 1, 1); 3];
        assert!(check_geometry(&comps).is_ok());
        // Extra components are not part of the transform and are not constrained.
        let mut more = comps.clone();
        more.push(component(12, true, 2, 2));
        assert!(check_geometry(&more).is_ok());
    }

    #[test]
    fn fewer_than_three_components_is_a_marker_error() {
        for count in 0..3 {
            let comps = vec![component(8, false, 1, 1); count];
            assert!(
                matches!(check_geometry(&comps), Err(Error::Marker(_))),
                "{count} component(s) should reject"
            );
        }
    }

    #[test]
    fn mismatched_depth_sign_or_sub_sampling_is_a_marker_error() {
        for odd in [
            component(12, false, 1, 1), // depth
            component(8, true, 1, 1),   // sign
            component(8, false, 2, 1),  // sub-sampling
        ] {
            let comps = vec![component(8, false, 1, 1), odd, component(8, false, 1, 1)];
            assert!(
                matches!(check_geometry(&comps), Err(Error::Marker(_))),
                "{odd:?} should reject"
            );
        }
    }
}
