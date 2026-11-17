//! Keeps a fuzz run under libFuzzer's RSS limit.

use rust_j2k::{DecodeOptions, WorkLimit};

/// The most samples a fuzz input may declare. A decode peaks at about 1.35
/// times its output for a square tile and up to about 3.2 times for a tile
/// one sample wide, so 2^28 samples (1 GiB as `i32`) stay under the 4 GiB
/// `-rss_limit_mb` the fuzz job passes. The decoder's default limit is twice
/// that, so inputs between the two are skipped here.
const MAX_FUZZ_SAMPLES: u64 = 1 << 28;

/// Whether `data` declares more output samples than [`MAX_FUZZ_SAMPLES`] at
/// full resolution, which bounds every window, tile and reduction of it.
/// An input whose header does not read is not over: it fails cheaply.
pub fn over(data: &[u8]) -> bool {
    let options = DecodeOptions::default().with_sample_limit(WorkLimit::Unlimited);
    rust_j2k::read_header_with(data, &options).is_ok_and(|header| {
        header
            .components
            .iter()
            .map(|c| u64::from(c.width) * u64::from(c.height))
            .sum::<u64>()
            > MAX_FUZZ_SAMPLES
    })
}
