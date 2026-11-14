#![no_main]
//! Fuzz target over the public [`rust_j2k::decode`] entry point (issue #18).
//!
//! `decode` parses bytes it did not produce, so the robustness contract is:
//! every input is either a decoded `Image` or a typed `Error` — never a panic,
//! abort, unbounded allocation, or hang. libFuzzer feeds arbitrary byte slices;
//! we discard the `Result` because the only property under test here is "does
//! not crash". A panic, an allocation past libFuzzer's `-rss_limit_mb`, or a
//! hang past `-timeout` fails the run and is dumped as a reproducer artifact.
//!
//! Half the inputs, picked by a hash of the input, decode under a
//! `max_quality_layers` cap of 1 to 3 instead, so the walk that reads a
//! layer's packets without keeping their passes is fuzzed too.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let hash = fnv1a(data);
    if hash & 1 == 0 {
        let _ = rust_j2k::decode(data);
    } else {
        let layers = 1 + ((hash >> 1) % 3) as u16;
        let options = rust_j2k::DecodeOptions::default().with_max_quality_layers(Some(layers));
        let _ = rust_j2k::decode_with(data, &options);
    }
});

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
