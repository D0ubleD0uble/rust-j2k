//! Pure-Rust JPEG 2000 decoder, scoped to the GRIB2 §5.40 (`grid_jpeg`) subset.
//!
//! JPEG 2000 (ISO/IEC 15444-1) is a large standard; this crate deliberately
//! decodes only the slice that GRIB2 uses, which keeps it tractable and
//! C-dependency-free:
//!
//! - the **raw codestream** (Annex A), not the JP2 file format (no boxes);
//! - a **single component** (one scalar grid), so no multi-component or color
//!   transform (MCT);
//! - **integer** samples, signed or unsigned, up to 32 bits;
//! - **both** the reversible 5/3 (lossless) and irreversible 9/7 (lossy)
//!   wavelet paths — operational GRIB2 (e.g. HRRR) uses lossy.
//!
//! Encoding, JP2 boxes, and multi-component imagery are out of scope.
//!
//! # Pipeline
//!
//! ```text
//! bytes ─▶ codestream ─▶ tier-2 ─▶ tier-1 ─▶ dequant ─▶ inverse DWT ─▶ Image
//!         (markers)     (packets)  (MQ+EBCOT) (quant)    (5/3 | 9/7)
//! ```
//!
//! Each stage is a module below. [`decode`] wires them together.
//!
//! # Status
//!
//! Skeleton. Every stage compiles but is `todo!()`; see each module's docs for
//! the ISO §reference and what it owns. Correctness is defined by the
//! conformance harness in `tests/` (cross-check against OpenJPEG / eccodes).

// TODO(skeleton): remove once the stages are implemented and surfaces settle.
#![allow(dead_code, unused_variables)]

pub mod codestream;
pub mod dwt;
pub mod error;
pub mod image;
pub mod quant;
pub mod tier1;
pub mod tier2;

pub use error::{Error, Result};
pub use image::Image;

/// Decode a JPEG 2000 **codestream** (Annex A, no JP2 wrapper) into a single
/// integer-component [`Image`].
///
/// This is the whole public surface for the GRIB2 use case: the §7 data
/// section of a `grid_jpeg` message is exactly such a codestream.
pub fn decode(codestream: &[u8]) -> Result<Image> {
    let cs = codestream::parse(codestream)?;

    // Tier-2: parse packets into per-code-block coded segments.
    let coded = tier2::decode_packets(&cs)?;
    // Tier-1: MQ + EBCOT bit-plane decode each code block into subband coeffs.
    let coeffs = tier1::decode_code_blocks(&cs.header, &coded)?;
    // Dequantize, then invert the DWT per resolution level into samples.
    let dequant = quant::dequantize(&cs.header, coeffs)?;
    let samples = dwt::inverse(&cs.header, dequant)?;

    image::assemble(&cs.header, samples)
}
