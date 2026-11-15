//! The Part 1 block coder, EBCOT (ISO/IEC 15444-1 Annexes C and D).
//!
//! Each code-block's coded bytes are an MQ arithmetic-coded bit-plane stream.
//! From the most significant non-zero plane down, every plane is decoded by up
//! to three passes (significance propagation, magnitude refinement, cleanup),
//! each forming contexts from the 3×3 neighborhood and reading binary
//! decisions from the [`mq`] decoder.
//!
//! The rest of tier-1 reaches this module only through
//! [`BlockCoder`](super::BlockCoder): [`decode`] and [`passes_run`].

pub(crate) mod mq;
pub(crate) mod passes;

use crate::tier2::{BandKind, CodeBlock};
use crate::{Error, Result};
use passes::{BlockParams, BlockState, MAX_BIT_PLANES, Orientation, decode_block, top_coded_plane};

/// Decode one code-block's codeword segments into its coefficients, row-major
/// over the block, at twice their weight (see [`decode_block`]).
///
/// `numbps` is the subband's magnitude bit-plane count `Mb`, `style` the
/// code-block style flags and `roi_shift` the component's maxshift.
pub(super) fn decode(
    block: &CodeBlock<'_>,
    kind: BandKind,
    numbps: u32,
    style: u8,
    roi_shift: u8,
) -> Result<Vec<i32>> {
    let orient = match kind {
        BandKind::Ll => Orientation::Ll,
        BandKind::Hl => Orientation::Hl,
        BandKind::Lh => Orientation::Lh,
        BandKind::Hh => Orientation::Hh,
    };
    // Maxshift lifts every region-of-interest coefficient above every
    // background one, so the block starts `roi_shift` planes higher (ISO
    // H.2's Kmax = Mb + s); see `top_coded_plane` for the interplay with the
    // zero bit-planes.
    //
    // The double-scale reconstruction shifts `1 << top`, so reject
    // high-dynamic-range subbands that would overflow `i32` rather than
    // panic — the same rejection OpenJPEG makes with `bpno_plus_one >= 31`.
    // This does not bound the maxshift itself: a block with more zero
    // bit-planes than `Mb` keeps `top` small under any `SPrgn`, and
    // `undo_maxshift` carries the oracle's `roishift >= 31` arm for it.
    let top = top_coded_plane(numbps, block.zero_bit_planes, roi_shift);
    if top > MAX_BIT_PLANES {
        return Err(Error::Limit(format!(
            "code-block needs {top} bit-planes, over the {MAX_BIT_PLANES}-plane limit"
        )));
    }
    let mut state = BlockState::new(block.width as u32, block.height as u32);
    // Each codeword segment is decoded from its own MQ stream; a segment's
    // bytes may still arrive in several layer-sized chunks, so they are
    // concatenated first. Without termination there is exactly one segment.
    let segments: Vec<(Vec<u8>, u32)> = block
        .segments
        .iter()
        .map(|segment| (segment.bytes(), segment.passes))
        .collect();
    // A segment the layer cap cut short, or one a missing packet may have
    // cut with `strict` off, can end before the encoder terminated it, so it
    // cannot be held to `predictable termination`. Only the last segment can
    // be cut; the block's earlier segments are whole and stay checked.
    let params = BlockParams {
        style,
        roi_shift,
        last_segment_cut: block.cut,
    };
    decode_block(
        &segments,
        &mut state,
        orient,
        numbps,
        block.num_passes,
        block.zero_bit_planes,
        params,
    )?;
    Ok(state.coeffs)
}

/// The coding passes [`decode_block`] runs on `block`: its passes, but no
/// more than its coded bit-planes hold, the first plane one pass and each
/// later one three.
pub(super) fn passes_run(numbps: u32, block: &CodeBlock<'_>, roi_shift: u8) -> u64 {
    if block.num_passes == 0 {
        return 0;
    }
    let top = top_coded_plane(numbps, block.zero_bit_planes, roi_shift);
    if !(1..=MAX_BIT_PLANES).contains(&top) {
        return 0;
    }
    u64::from(block.num_passes.min(3 * top - 2))
}
