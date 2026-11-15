#![no_main]
//! Fuzz the HTJ2K block decoder directly: block shape (up to the 4096 samples
//! of a code-block, 1x1024 to 1024x4 included), `Mb`, `P`, the coding passes,
//! the causal flag, a maxshift, the `CAP` bound and the cleanup and
//! refinement segments (to 5000 bytes each, in chunks, dense in 0xFF) all
//! come from the input, via the hidden hook the library exposes for exactly
//! this (`rust_j2k::fuzz::ht_block`, which documents the byte layout).
//!
//! Seed it with code-blocks sliced from the HT corpus:
//! `RUST_J2K_HT_SEED_DIR=fuzz/corpus/ht_block cargo test --lib ht_block_seeds`.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    rust_j2k::fuzz::ht_block(data);
});
