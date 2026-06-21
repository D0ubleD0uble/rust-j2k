#![no_main]
//! Fuzz target over the public [`rust_j2k::decode`] entry point (issue #18).
//!
//! `decode` parses bytes it did not produce, so the robustness contract is:
//! every input is either a decoded `Image` or a typed `Error` — never a panic,
//! abort, unbounded allocation, or hang. libFuzzer feeds arbitrary byte slices;
//! we discard the `Result` because the only property under test here is "does
//! not crash". A panic, an allocation past libFuzzer's `-rss_limit_mb`, or a
//! hang past `-timeout` fails the run and is dumped as a reproducer artifact.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = rust_j2k::decode(data);
});
