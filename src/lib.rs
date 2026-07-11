//! Pure-Rust JPEG 2000 codec, built GRIB2-decode-first toward OpenJPEG-level
//! coverage. No C dependency, so it cross-compiles cleanly to every target.
//!
//! JPEG 2000 (ISO/IEC 15444-1 and later parts) is a large standard. The first
//! deliverable decodes the slice GRIB2 §5.40 (`grid_jpeg`) needs, which keeps
//! the initial surface tractable while exercising the whole pipeline:
//!
//! - the **raw codestream** (Annex A), not yet the JP2 file format (no boxes);
//! - **components**, each with its own bit depth, sign, and sub-sampling, plus
//!   the reversible color transform (RCT) that decorrelates the first three;
//!   its irreversible twin (ICT) is not decoded yet;
//! - **integer** samples, signed up to 32 bits, unsigned up to 31 (the
//!   samples land in an `i32` container);
//! - **both** the reversible 5/3 (lossless) and irreversible 9/7 (lossy)
//!   wavelet paths (the 9/7 path is graded by re-encoding a real grid with
//!   OpenJPEG, since no operational GRIB2 producer ships lossy 9/7).
//!
//! ICT, JP2 boxes, HTJ2K, and an encoder are later-phase work, not permanent
//! non-goals. See `docs/roadmap.md` and `docs/scope.md`.
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
//! out. An [`Image`] is the image area on the reference grid plus one
//! [`Component`] per codestream component; a sub-sampled component holds fewer
//! samples than that area, so size sample buffers from the component, not from
//! the image. Malformed input never panics; it comes back as a typed [`Error`].
//!
//! ```
//! use rust_j2k::{decode, Error};
//!
//! // In real use these are the bytes of a `.j2k` codestream, or the GRIB2 §7
//! // data section of a `grid_jpeg` message. Invalid input is rejected cleanly:
//! match decode(b"not a codestream") {
//!     Ok(image) => {
//!         let first = image.component(0).expect("at least one component");
//!         println!(
//!             "image area {}x{}, first component {}x{}",
//!             image.width, image.height, first.width, first.height,
//!         );
//!     }
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
pub(crate) mod mct;
pub(crate) mod quant;
pub(crate) mod tier1;
pub(crate) mod tier2;

// Structured fuzz entry points for the detached fuzz/ workspace. Compiled
// unconditionally so that workspace builds under plain cargo (rust-analyzer,
// `cargo check` in fuzz/), but hidden from docs and exempt from semver: this
// is a test seam, not public API.
#[doc(hidden)]
pub mod fuzz;

pub use error::{Error, Result};
pub use image::{Component, Image};

/// Decode a JPEG 2000 **codestream** (Annex A, no JP2 wrapper) into an
/// [`Image`]: the reference-grid image area plus one [`Component`] per SIZ
/// component, each on its own sample grid.
///
/// This is the whole public surface for the GRIB2 use case: the §7 data
/// section of a `grid_jpeg` message is exactly such a codestream. That subset is
/// single-component, so its images carry one component; a codestream declaring
/// more decodes each component onto its own grid, independently.
///
/// Pass the bare codestream, not a `.jp2` file: a JP2 wrapper is valid JPEG 2000
/// but is rejected with [`Error::Unsupported`] rather than unwrapped. Anything
/// else outside the decoded subset is rejected the same way, never half-decoded.
pub fn decode(codestream: &[u8]) -> Result<Image> {
    decode_with(codestream, DecodeOptions::default())
}

/// Options for [`decode_with`]. The default decodes at full resolution —
/// [`decode`] is exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecodeOptions {
    /// How many of the finest resolution levels to discard. Each level halves
    /// the output in both axes (rounding up), so `1` decodes a half-size image,
    /// `2` a quarter-size, and so on — the wavelet pyramid's own lower
    /// resolutions, not a resample. Every component must keep at least one
    /// resolution: a reduction that consumes some component's whole pyramid is
    /// rejected as [`Error::Unsupported`].
    pub resolution_reduction: u8,
}

/// Decode a JPEG 2000 codestream like [`decode`], governed by `options`.
pub fn decode_with(codestream: &[u8], options: DecodeOptions) -> Result<Image> {
    let cs = codestream::parse(codestream)?;

    // A reduction must leave every component a resolution to decode, checked
    // against each component's own decomposition count: a COC can give one
    // component a shallower pyramid than the rest (p0_08 does exactly that).
    let reduction = options.resolution_reduction;
    for (index, params) in cs.header.components.iter().enumerate() {
        let resolutions = u32::from(params.coding.decomposition_levels) + 1;
        if u32::from(reduction) >= resolutions {
            return Err(Error::Unsupported(format!(
                "resolution reduction {reduction} discards all {resolutions} resolutions \
                 of component {index}"
            )));
        }
    }

    // Each tile is a whole decode of its own: its own header, its own packets,
    // its own wavelet pyramid in its own coordinate frame. Nothing crosses a
    // tile boundary — that is what tiling is for — so the tiles meet only at the
    // end, when each writes its rectangle of the canvas.
    let mut canvas = image::Canvas::new(&cs.header, reduction)?;
    for tile in &cs.tiles {
        // Tier-2: parse packets into per-code-block coded segments, per
        // component. The walk covers every resolution whatever the reduction:
        // packet order is the codestream's framing, so the dropped levels'
        // packets must still be stepped over to reach the kept ones.
        let coded = tier2::decode_packets(tile)?;
        // Tier-1: MQ + EBCOT bit-plane decode each code block into subband
        // coeffs. The dropped resolutions' code-blocks are skipped, never
        // decoded.
        let coeffs = tier1::decode_code_blocks(&tile.header, &coded, reduction)?;

        // Dequantize, then invert the DWT per resolution level into samples.
        let mut samples = coeffs
            .into_iter()
            .enumerate()
            .map(|(component, coeffs)| {
                let dequant = quant::dequantize(&tile.header, component, coeffs)?;
                dwt::inverse(&tile.header, component, dequant)
            })
            .collect::<Result<Vec<_>>>()?;

        // Undo the inter-component decorrelation, if any, before the DC level
        // shift (ISO/IEC 15444-1 G.1). It is a per-tile transform: the flag
        // lives in COD, which a tile-part header can override. Components past
        // the third are untouched.
        if tile.header.cod.multiple_component_transform {
            mct::inverse_rct(&mut samples)?;
        }

        for (component, samples) in samples.into_iter().enumerate() {
            let rect = cs
                .header
                .siz
                .tile_component_rect(tile.index, component)
                .ok_or_else(|| {
                    Error::Inconsistent(format!(
                        "no tile {} or no component {component} in SIZ",
                        tile.index
                    ))
                })?;
            canvas.place(component, rect.reduced(reduction), samples)?;
        }
    }

    image::assemble(&cs.header, canvas.into_planes(), reduction)
}
