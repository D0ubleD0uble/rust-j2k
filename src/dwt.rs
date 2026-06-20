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

use crate::Result;
use crate::codestream::MainHeader;
use crate::tier1::SubbandCoeffs;

/// Inverse-transform all resolution levels into the final raster of samples
/// (pre level-shift), driven by the COD transform choice and decomposition
/// level count.
pub fn inverse(header: &MainHeader, coeffs: SubbandCoeffs) -> Result<Vec<i32>> {
    todo!("per level: 2-D inverse via 1-D lifting on rows then columns, LL up to full res")
}

/// One-dimensional inverse 5/3 reversible lifting over `signal` in place
/// (F.3.8.2). Exact integer arithmetic.
fn inverse_5_3(signal: &mut [i32]) {
    todo!("reversible 5/3 lifting: undo predict/update with floor-rounded integer steps")
}

/// One-dimensional inverse 9/7 irreversible lifting over `signal` in place
/// (F.3.8.1).
fn inverse_9_7(signal: &mut [f32]) {
    todo!("irreversible 9/7 lifting: scaling then two predict/update lifting pairs")
}
