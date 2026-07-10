#![no_main]
//! Fuzz the tier-1 EBCOT block decoder directly: block shape, coding
//! parameters, and the coded segment all come from the input, via the hidden
//! hook the library exposes for exactly this (`rust_j2k::fuzz`). The MQ coder
//! and the three coding passes are the hairiest state machines in the crate,
//! and the byte-oriented targets rarely reach them with interesting state.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    rust_j2k::fuzz::tier1_block(data);
});
