#![no_main]
//! Fuzz target over [`rust_j2k::decode_with`] with `strict` off: the decode
//! that takes missing data as missing and reports it.
//!
//! Same contract as `decode`: a typed result for every input, never a panic,
//! an allocation sized from an unchecked field, or a hang. Every work and
//! sample bound holds with `strict` off as with it on, so this target runs
//! under the default limits and libFuzzer's `-rss_limit_mb` and `-timeout`
//! catch a bound that does not.
//!
//! Each input is decoded whole and as a prefix (cut at a point a hash of the
//! input picks), at reduction 0 or 1, and checked against the strict decode
//! of the same bytes (`common/partial.rs`): when the strict decode gives an
//! image, the decode with `strict` off gives the same image with the
//! complete report, and the report stays within its bound.

use libfuzzer_sys::fuzz_target;

#[path = "../common/partial.rs"]
mod partial;

/// Inputs above this many bytes are decoded whole only.
const MAX_CUT_INPUT: usize = 1 << 16;

fuzz_target!(|data: &[u8]| {
    let hash = fnv1a(data);
    let options = rust_j2k::DecodeOptions::default().with_resolution_reduction((hash & 1) as u8);
    partial::check(data, &options, &rust_j2k::decode_with(data, &options));
    if data.len() > 1 && data.len() <= MAX_CUT_INPUT {
        let cut = &data[..(hash >> 1) as usize % data.len()];
        partial::check(cut, &options, &rust_j2k::decode_with(cut, &options));
    }
});

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
