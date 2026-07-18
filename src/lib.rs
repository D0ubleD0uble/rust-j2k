//! Pure-Rust JPEG 2000 codec, built GRIB2-decode-first toward OpenJPEG-level
//! coverage. No C dependency, so it cross-compiles cleanly to every target.
//!
//! The decoder reads a **raw codestream** (ISO/IEC 15444-1 Annex A, not yet
//! the JP2 file format) into **integer** samples: one grid per component,
//! signed up to 32 bits, unsigned up to 31 (an `i32` container), each with its
//! own bit depth, sign, and sub-sampling. Both wavelet paths decode — the
//! reversible 5/3 bit-exactly, the irreversible 9/7 within the standard's
//! compliance bounds — as do tiling and tile-parts, precincts, quality layers,
//! all five progression orders and POC changes between them, packed packet
//! headers (PPM/PPT), every Part 1 code-block coding style, region of interest
//! (maxshift), non-zero canvas offsets, resolution-reduced decoding, and the
//! reversible (RCT) and irreversible (ICT) colour transforms.
//!
//! JP2 boxes, HTJ2K, and an encoder are later work, not permanent non-goals.
//! See `docs/roadmap.md` and `docs/scope.md`.
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
//! All 23 entries of the ISO/IEC 15444-4 conformance suite decode within their
//! class-1 error bounds, and that gate runs in `tests/` against committed
//! oracle snapshots, with no external tools. Correctness is defined by
//! cross-checking a reference decoder (OpenJPEG / eccodes), not by
//! self-consistency. A codestream using a feature outside the decoded set — a
//! JP2 wrapper, HTJ2K, a Part 2 extension — is rejected with
//! [`Error::Unsupported`], never half-decoded. GRIB2 §5.40 (`grid_jpeg`)
//! payloads, the crate's original target, are raw codestreams and decode
//! directly. See each module's docs for the ISO §reference and what it owns;
//! the feature map is in `docs/scope.md`.
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
///
/// Start from [`DecodeOptions::default`] and set what differs, with the
/// `with_*` methods or the fields directly. The struct is `#[non_exhaustive]`,
/// so a new option is not a breaking change.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// How many of the finest resolution levels to discard. Each level halves
    /// the output in both axes (rounding up), so `1` decodes a half-size image,
    /// `2` a quarter-size, and so on — the wavelet pyramid's own lower
    /// resolutions, not a resample. Every component must keep at least one
    /// resolution: a reduction that consumes some component's whole pyramid is
    /// rejected as [`Error::InvalidOptions`].
    pub resolution_reduction: u8,
}

impl DecodeOptions {
    /// These options with
    /// [`resolution_reduction`](Self::resolution_reduction) set to `levels`.
    #[must_use]
    pub fn with_resolution_reduction(mut self, levels: u8) -> Self {
        self.resolution_reduction = levels;
        self
    }
}

/// Decode a JPEG 2000 codestream like [`decode`], governed by `options`.
pub fn decode_with(codestream: &[u8], options: DecodeOptions) -> Result<Image> {
    let cs = codestream::parse(codestream)?;

    let reduction = options.resolution_reduction;

    // Each tile is a whole decode of its own: its own header, its own packets,
    // its own wavelet pyramid in its own coordinate frame. Nothing crosses a
    // tile boundary — that is what tiling is for — so the tiles meet only at the
    // end, when each writes its rectangle of the canvas.
    let mut canvas = image::Canvas::new(&cs.header, reduction)?;
    for (index, tile) in cs.tiles.iter().enumerate() {
        // The tile's own parameters: the main header with this tile's tile-part
        // header resolved over it. Held for one tile at a time — a header is
        // sized by the component count, and a codestream can declare tens of
        // thousands of tiles.
        let header = cs.tile_header(index)?;

        // A reduction must leave every component a resolution to decode, checked
        // against each component's own decomposition count: a COC can give one
        // component a shallower pyramid than the rest (p0_08 does exactly that),
        // and a tile COD can give one tile a shallower pyramid than the rest — so
        // this is per tile, not once for the image.
        for (component, params) in header.components.iter().enumerate() {
            let resolutions = u32::from(params.coding.decomposition_levels) + 1;
            if u32::from(reduction) >= resolutions {
                return Err(Error::InvalidOptions(format!(
                    "resolution reduction {reduction} discards all {resolutions} resolutions \
                     of component {component} in tile {index}"
                )));
            }
        }

        // Tier-2: parse packets into per-code-block coded segments, per
        // component. The walk covers every resolution whatever the reduction:
        // packet order is the codestream's framing, so the dropped levels'
        // packets must still be stepped over to reach the kept ones.
        let coded = tier2::decode_packets(&header, tile)?;
        // Tier-1: MQ + EBCOT bit-plane decode each code block into subband
        // coeffs. The dropped resolutions' code-blocks are skipped, never
        // decoded.
        let coeffs = tier1::decode_code_blocks(&header, &coded, reduction)?;

        // Dequantize, then invert the DWT per resolution level into samples.
        let mut samples = coeffs
            .into_iter()
            .enumerate()
            .map(|(component, coeffs)| {
                let dequant = quant::dequantize(&header, component, coeffs)?;
                dwt::inverse(&header, component, dequant)
            })
            .collect::<Result<Vec<_>>>()?;

        // Undo the inter-component decorrelation, if any, before the DC level
        // shift (ISO/IEC 15444-1 G.1). It is a per-tile transform: the flag
        // lives in COD, which a tile-part header can override. Components past
        // the third are untouched.
        if header.cod.multiple_component_transform {
            // The wavelet picks the transform (G.1): 5/3 reconstructs integers
            // and inverts with the reversible RCT, 9/7 reconstructs floats and
            // inverts with the irreversible ICT. All three colour components
            // share the wavelet (checked at parse), so component 0's decides.
            match header.components[0].coding.transform {
                codestream::markers::Transform::Reversible53 => mct::inverse_rct(&mut samples)?,
                codestream::markers::Transform::Irreversible97 => mct::inverse_ict(&mut samples)?,
            }
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
