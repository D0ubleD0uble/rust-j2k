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
//! Every input is decoded without a cap. Half of them, picked by a hash of
//! the input, are then decoded again under a `max_quality_layers` cap of 1 to
//! 3, in strict mode and with `strict` off, so the walk that reads a layer's
//! packets without keeping their passes is fuzzed too. Another half are
//! decoded through a window, a tile or a subset of components, all picked by
//! the same hash, so the tile skip, the clipped placement and the
//! per-component decode are fuzzed as well.

use libfuzzer_sys::fuzz_target;

#[path = "../common/budget.rs"]
mod budget;
#[path = "../common/partial.rs"]
mod partial;

fuzz_target!(|data: &[u8]| {
    // Up to four decodes of the input run one after another: the plain one, the
    // strict one, the lenient one in `partial::check`, and `part`.
    if budget::over(data, 4) {
        return;
    }
    let _ = rust_j2k::decode(data);
    let hash = fnv1a(data);
    if hash & 1 == 1 {
        let layers = 1 + ((hash >> 1) % 3) as u16;
        let options = rust_j2k::DecodeOptions::default().with_max_quality_layers(Some(layers));
        let strict = rust_j2k::decode_with(data, &options);
        // The same cap with `strict` off, where a block's last kept pass may
        // be cut short by the cap (#358). That bug gave a wrong image, not a
        // panic, so the decode is checked against the strict one.
        partial::check(data, &options, &strict);
    }
    if hash & 2 == 2 {
        let _ = rust_j2k::decode_with(data, &part(hash >> 8));
    }
});

/// A window, a tile or a component subset, with a reduction of 0 or 1, from
/// the bits of `bits`. The window's corners are small numbers, so that they
/// land inside the small images the fuzzer makes.
fn part(bits: u64) -> rust_j2k::DecodeOptions {
    let small = |shift: u32| ((bits >> shift) & 0x3F) as u32;
    let options =
        rust_j2k::DecodeOptions::default().with_resolution_reduction(((bits >> 2) & 1) as u8);
    match bits & 3 {
        0 => {
            let (x0, y0) = (small(4), small(10));
            options.with_region(rust_j2k::Region::Window(rust_j2k::Window::new(
                x0,
                y0,
                x0 + 1 + small(16),
                y0 + 1 + small(22),
            )))
        }
        1 => options.with_region(rust_j2k::Region::Tile(small(28))),
        2 => options.with_components(Some(vec![(small(34) % 4) as u16])),
        _ => options
            .with_components(Some(vec![2, 0]))
            .with_region(rust_j2k::Region::Window(rust_j2k::Window::new(
                small(40),
                small(46),
                64,
                64,
            ))),
    }
}

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
