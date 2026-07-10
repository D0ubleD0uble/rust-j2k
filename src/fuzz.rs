//! Structured fuzz entry points (see `fuzz/`).
//!
//! The `decode` fuzz target must assemble a byte-perfect main header before a
//! mutation reaches tier-2 or tier-1, so those state machines see little of
//! the fuzzer's throughput. The helpers here pin the well-formed scaffolding
//! and hand the fuzzer only the untrusted bytes one stage consumes. The
//! contract under test is the crate-wide one — typed errors, never a panic,
//! bounded work — so results are discarded.
//!
//! Compiled for `cargo fuzz` builds (`--cfg fuzzing`) and for `cargo test` (a
//! smoke test keeps the surface building); absent from a normal build.

use crate::tier1::mq::{Context, MqDecoder};
use crate::tier1::passes::{BlockParams, BlockState, Orientation, decode_block};

/// Drive the raw MQ decoder over `data`: context transitions, BYTEIN
/// stuffing, marker handling, and past-end synthesis, with bounded work.
pub fn mq_stream(data: &[u8]) {
    let mut mq = MqDecoder::new(data);
    let mut contexts: [Context; 19] = [Context::default(); 19];
    // Enough decisions to consume every coded byte and run well past the end
    // into marker synthesis, capped so a large input cannot hang an exec.
    let decisions = (data.len() * 8).clamp(256, 1 << 20);
    for i in 0..decisions {
        let _ = mq.decode(&mut contexts[i % contexts.len()]);
    }
}

/// Decode one code-block whose shape, coding parameters, and coded bytes are
/// all steered by `data`, the way tier-2 would hand them to tier-1.
pub fn tier1_block(data: &[u8]) {
    let [a, b, c, d, e, coded @ ..] = data else {
        return;
    };
    use crate::codestream::markers::code_block_style::{PTERM, SEGSYM, TERMALL};

    let width = u32::from(a & 0x3F) + 1; // 1..=64, the subset's block ceiling
    let height = u32::from(b & 0x3F) + 1;
    let orient = match a >> 6 {
        0 => Orientation::Ll,
        1 => Orientation::Hl,
        2 => Orientation::Lh,
        _ => Orientation::Hh,
    };
    let params = BlockParams {
        // Anything else is rejected by decode_cod, and decode_block
        // debug-asserts that precondition.
        style: b & (TERMALL | PTERM | SEGSYM),
        roi_shift: c & 0x3F,
    };
    let num_passes = u32::from(*c) % 110; // tier-2's 109-pass segment cap
    let numbps = u32::from(*d) % 31; // MAX_BIT_PLANES
    // One past the largest agreeing value, so the zero_bit_planes > numbps
    // typed reject is exercised alongside in-range decodes.
    let zero_bit_planes = u32::from(*e) % 32;

    let mut state = BlockState::new(width, height);
    let segments = [(coded.to_vec(), num_passes)];
    let _ = decode_block(
        &segments,
        &mut state,
        orient,
        numbps,
        num_passes,
        zero_bit_planes,
        params,
    );
}

#[cfg(test)]
mod tests {
    /// Smoke-run both hooks so the fuzz surface builds and stays panic-free in
    /// CI, where `--cfg fuzzing` never is set.
    #[test]
    fn fuzz_hooks_run_on_sample_inputs() {
        let samples: [&[u8]; 5] = [
            &[],
            &[0xFF; 40],
            &[0x00; 40],
            &[0x55, 0x1E, 0x22, 0x0A, 0x03, 0x80, 0xFF, 0x91],
            &[0xC0, 0x24, 0x6D, 0x1E, 0x21, 0xFF, 0xD9, 0x00],
        ];
        for sample in samples {
            super::mq_stream(sample);
            super::tier1_block(sample);
        }
    }
}
