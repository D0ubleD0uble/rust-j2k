//! Stage 4 — dequantization (ISO/IEC 15444-1 Annex E.1).
//!
//! Maps the quantized integers from Tier-1 back to wavelet-coefficient
//! magnitudes before the inverse transform. For the **reversible** path (5/3,
//! `QuantStyle::None`) there is no scaling — only the implicit bit-shift from
//! the decoded bit-planes — and the inverse must stay exact. For the
//! **irreversible** path (9/7) each subband has a scalar step size, derived or
//! expounded, reconstructed from the QCD/QCC (exponent, mantissa) and the
//! number of guard bits.

use crate::Result;
use crate::codestream::MainHeader;
use crate::tier1::SubbandCoeffs;

/// Apply per-subband dequantization in place. Reversible: identity. Irreversible:
/// multiply by the subband step (with the standard mid-point reconstruction
/// bias). Returns coefficients ready for the inverse DWT.
pub fn dequantize(header: &MainHeader, coeffs: SubbandCoeffs) -> Result<SubbandCoeffs> {
    todo!("reversible = identity; irreversible = scalar step per subband from QCD")
}
