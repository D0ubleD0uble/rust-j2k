//! Pure-Rust JPEG 2000 codec, built GRIB2-decode-first toward OpenJPEG-level
//! coverage. No C dependency, so it cross-compiles cleanly to every target.
//!
//! JPEG 2000 (ISO/IEC 15444-1 and later parts) is a large standard. The first
//! deliverable decodes the slice GRIB2 §5.40 (`grid_jpeg`) needs, which keeps
//! the initial surface tractable while exercising the whole pipeline:
//!
//! - the **raw codestream** (Annex A), not yet the JP2 file format (no boxes);
//! - a **single component** (one scalar grid), so no multi-component or color
//!   transform (MCT) yet;
//! - **integer** samples, signed or unsigned, up to 32 bits;
//! - **both** the reversible 5/3 (lossless) and irreversible 9/7 (lossy)
//!   wavelet paths (the 9/7 path is graded by re-encoding a real grid with
//!   OpenJPEG, since no operational GRIB2 producer ships lossy 9/7).
//!
//! Multi-component/color, JP2 boxes, HTJ2K, and an encoder are later-phase
//! work, not permanent non-goals. See `docs/roadmap.md` and `docs/scope.md`.
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
//! # Example
//!
//! The entire public surface is [`decode`]: codestream bytes in, an [`Image`]
//! out. Malformed input never panics; it comes back as a typed [`Error`].
//!
//! ```
//! use rust_j2k::{decode, Error};
//!
//! // In real use these are the bytes of a `.j2k` codestream, or the GRIB2 §7
//! // data section of a `grid_jpeg` message. Invalid input is rejected cleanly:
//! match decode(b"not a codestream") {
//!     Ok(image) => println!("decoded {}x{}", image.width, image.height),
//!     Err(Error::Unsupported(what)) => println!("outside the decoded subset: {what}"),
//!     Err(e) => println!("decode failed: {e}"),
//! }
//! ```
//!
//! # Status
//!
//! The GRIB2 §5.40 decode path described above is implemented end to end and
//! passes the conformance gate (bit-exact 5/3, within tolerance 9/7). Anything
//! outside that subset is rejected with [`Error::Unsupported`], never
//! half-decoded. See each module's docs for the ISO §reference and what it owns.
//! Correctness is defined by the conformance harness in `tests/` (cross-check
//! against OpenJPEG / eccodes), not by self-consistency. The plan for widening
//! this same engine toward general Part 1 is in `docs/roadmap.md`; the feature
//! map is in `docs/scope.md`.
#![warn(missing_docs)]

// The pipeline modules are crate-internal: the public API is `decode`, `Image`,
// `Error`, and `Result`. Keeping the stages private lets each one evolve freely
// (the roadmap widens all of them) without churning the crate's committed
// surface, and keeps the docs.rs page to what a caller can actually use.
pub(crate) mod codestream;
pub(crate) mod dwt;
pub(crate) mod error;
pub(crate) mod image;
pub(crate) mod quant;
pub(crate) mod tier1;
pub(crate) mod tier2;

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
