//! Tier-1 keeps its per-block buffers from block to block: a decode of many
//! small code-blocks makes a handful of allocations per block, not one for
//! every buffer of every block.
//!
//! `tests/fixtures/bench/small_blocks_128_b4.j2k` is a 128x128 8-bit image
//! in 4x4 code-blocks (`scripts/gen-small-block-fixture.py`). The expected
//! file holds an FNV-1a digest of `opj_decompress`'s samples.
//!
//! Allocations are counted by a counting global allocator, so this file holds
//! a single test: a second one running in parallel would count into the same
//! total.
//!
//! A global allocator can only be written with `unsafe`. The shim in
//! `tests/support/alloc.rs` forwards every call to `System` unchanged, which
//! the project conventions allow (`.claude/rules/conventions.md`, "Stay
//! pure-Rust"). The library itself has no `unsafe`.

#[path = "support/alloc.rs"]
mod alloc;
#[path = "support/skip.rs"]
mod skip;
use alloc::ALLOCATIONS;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

#[global_allocator]
static ALLOCATOR: alloc::CountAlloc = alloc::CountAlloc;

/// FNV-1a over the 8-bit samples, in raster order. The generator script
/// computes the same digest.
fn fnv1a64(samples: &[i32]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &sample in samples {
        let byte = u8::try_from(sample).expect("an 8-bit sample");
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[test]
fn many_small_blocks_allocate_little_per_block() {
    // `tests/fixtures/bench/` is left out of the published package.
    if skip::excluded() {
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bench");
    let bytes = std::fs::read(dir.join("small_blocks_128_b4.j2k")).expect("the bench fixture");
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("small_blocks_128_b4.expected.json")).unwrap(),
    )
    .unwrap();

    let counted = |options: &rust_j2k::DecodeOptions| {
        let before = ALLOCATIONS.load(Ordering::SeqCst);
        let image = rust_j2k::decode_with(&bytes, options).expect("the fixture decodes");
        (image, ALLOCATIONS.load(Ordering::SeqCst) - before)
    };
    let (image, full) = counted(&rust_j2k::DecodeOptions::default());
    let component = &image.components[0];
    assert_eq!((component.width, component.height), (128, 128));
    assert_eq!(fnv1a64(&component.samples), expected["fnv1a64"]);

    // Reduced by all 5 levels, the decode still reads every packet, so tier-2
    // makes the same allocations, but tier-1 decodes the one LL code-block.
    // The difference is what the other 1023 code-blocks cost tier-1 and the
    // wavelet: a few allocations per subband and level, where a set of
    // buffers for every block made over 4000.
    let (_, reduced) = counted(&rust_j2k::DecodeOptions::default().with_resolution_reduction(5));
    let blocks_cost = full.saturating_sub(reduced);
    eprintln!("tier1_alloc: {full} allocations, {reduced} reduced by 5 levels");
    assert!(
        blocks_cost <= 64,
        "decoding the code-blocks of 5 levels made {blocks_cost} allocations \
         ({full} in all, {reduced} reduced by 5 levels)"
    );
}
