#![no_main]
//! Fuzz tier-2 packet parsing and everything downstream of it.
//!
//! The `decode` target must mutate its way through a byte-perfect main header
//! before anything reaches the packet reader, so most executions die in
//! `parse_main_header`. Here the header is pinned — a small valid image with
//! the first input byte steering the wavelet, the SOP/EPH delimiters, and the
//! **tile-component geometry** — and the rest of the input becomes the tile-part
//! body: packet headers, tag trees, code-block segments. Same contract as
//! `decode`: typed result or bust, never a panic, unbounded allocation, or hang.
//!
//! The `variant` byte also picks the geometry shape, so one fuzzed body reaches
//! the widened tier-2 paths, not only the single-component single-layer
//! default:
//!
//! - the multi-component + color transform walk and the multi-layer packet
//!   accumulation (shapes 1 and 2);
//! - the explicit precinct partition (shape 3);
//! - the selective arithmetic coding bypass (lazy) segment split, alone and
//!   combined with the other five code-block style flags (shape 4);
//! - the POC progression-volume walk over non-trivial layer / resolution /
//!   component / precinct geometry, with the volumes themselves fuzzed
//!   (shape 5);
//! - a 2×2 multi-tile grid with two tile-parts per tile, interleaved across
//!   the tiles (shape 6);
//! - the PPM / PPT packed packet-header split, each stitched from two
//!   out-of-order marker segments (shape 7).
//!
//! Variant bit 7 turns any shape's code-blocks into HT ones (a `CAP` marker and
//! the HT bit of the code-block style). With bit 7, bit 6 makes them MIXED
//! (style `0xC0`, `Ccap15` bits 15-14 `11`), so each block is HT or Part 1 as
//! its length fields say; shape 7 keeps bit 6 for PPM, so it has no MIXED
//! variant.
//!
//! Each input is decoded strict and then with `strict` off, and the two are
//! checked against each other as `decode_partial` checks them
//! (`common/partial.rs`).

#[path = "../../tests/support/synth.rs"]
mod synth;
// `wrap` and its helpers, shared with `tests/tile_body_variants.rs`, which pins
// the header bytes each variant builds.
#[path = "../../tests/support/tile_body.rs"]
mod tile_body;
use libfuzzer_sys::fuzz_target;
use tile_body::wrap;

#[path = "../common/partial.rs"]
mod partial;

fuzz_target!(|data: &[u8]| {
    let Some((&variant, body)) = data.split_first() else {
        return;
    };
    // The strict decode, then the same bytes with `strict` off, which reads
    // a body that runs short or long instead of refusing it.
    let bytes = wrap(variant, body);
    let options = rust_j2k::DecodeOptions::default();
    let strict = rust_j2k::decode_with(&bytes, &options);
    partial::check(&bytes, &options, &strict);
});
