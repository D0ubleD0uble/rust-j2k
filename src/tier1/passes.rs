//! The three EBCOT coding passes and their context formation
//! (ISO/IEC 15444-1 Annex D).
//!
//! A code-block is decoded bit-plane by bit-plane, MSB→LSB. Each plane runs, in
//! order, the passes that apply: significance propagation (D.3.1), magnitude
//! refinement (D.3.2), and cleanup (D.3.3) with its run-length mode. Contexts
//! come from the significance state of the 3×3 neighbourhood (D.3.* tables),
//! and sign coding uses the neighbour-sign context (D.3.4). The cleanup pass
//! of the top plane runs first; the optional code-block style flags
//! (bypass/lazy, reset, restart, vertically-causal, segmentation) modulate
//! termination and context reset.

use crate::tier1::mq::MqDecoder;

/// A code-block's decode state: the coefficient magnitudes, sign bits, and the
/// significance/visited flags the passes consult and update.
#[derive(Debug)]
pub struct BlockState {
    pub width: u32,
    pub height: u32,
    pub coeffs: Vec<i32>,
    // significance + "visited this plane" + sign bitsets, parallel to coeffs.
}

impl BlockState {
    pub fn new(width: u32, height: u32) -> Self {
        BlockState {
            width,
            height,
            coeffs: vec![0; (width * height) as usize],
        }
    }
}

/// Decode one code-block: run the bit-plane passes over `mq` into `state`.
///
/// `num_passes` and `zero_bit_planes` come from the Tier-2 packet headers;
/// `style` is the COD/COC code-block style flags.
pub fn decode_block(
    mq: &mut MqDecoder<'_>,
    state: &mut BlockState,
    num_passes: u32,
    zero_bit_planes: u32,
    style: u8,
) {
    todo!("MSB→LSB bit-planes: significance-propagation, magnitude-refinement, cleanup")
}
