#![no_main]
//! Fuzz target over [`rust_j2k::IncrementalDecoder::header`] fed in pieces.
//!
//! The decoder keeps what its header read found between calls (the walk of
//! the tile-parts, the main header's tile counts, what each whole first
//! tile-part gave) and goes on from there when more bytes come in. Each call
//! must still give what `read_header_with` gives with `strict` off on the
//! bytes fed so far, under the decoder's options: the same header, the same
//! error, and `None` where that read finds the main header incomplete. The
//! other targets never call `header()`, and the committed differential test
//! (`tests/incremental_header.rs`) uses the default limits only.
//!
//! Layout: 12 control bytes, then the file.
//!
//! - Byte 0, mod 3: the resolution reduction.
//! - Byte 1: bit 0 is `strict` (the oracle reads with it off, as `header()`
//!   documents), bits 1 and 2 the sample limit (`Default`, `Raised`,
//!   `Unlimited` or `AtMost`, sized by byte 4).
//! - Bytes 2 and 3: the region (the whole image, a tile, or one of two kinds
//!   of window).
//! - Byte 5: bit 0 sRGB output, bit 1 raw channels, bit 2 a component subset,
//!   bit 3 a layer cap.
//! - Bytes 6 to 11: the seed of the stream that picks the sizes of the pieces
//!   and the changes of options between them.
//!
//! The file is fed in pieces of random sizes, about 24 on average, so a
//! tile-part is often cut in the middle. Now and then the region, the
//! reduction or the layer cap changes. After each feed `header()` is checked
//! against the oracle on the prefix fed so far, and on one feed in five a
//! second call is checked too, which must give the same again.
//!
//! Random bytes seldom reach a tile over a cap under the main header, which
//! is what the cache exists for, so seed it from
//! `scripts/gen-header-split-seeds.py` (see `fuzz/README.md`).

use libfuzzer_sys::fuzz_target;
use rust_j2k::{
    ColorOutput, DecodeOptions, Error, IncrementalDecoder, Region, Window, WorkLimit,
    read_header_with,
};

/// splitmix64: the same stream on every platform, from a seed in the input.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// A region from two input bytes, and `salt` (the bytes fed or the file's
/// length) for the small window. The windows are laid out for tiles 1040
/// wide and 512 high, which the synthetic seeds use.
fn region(code: u8, arg: u8, salt: usize) -> Region {
    match code % 4 {
        0 => Region::Whole,
        1 => Region::Tile(u32::from(arg % 80)),
        2 => {
            let x = u32::from(arg) * 1040 / 8;
            Region::Window(Window::new(x, 0, x + 1040 * 3, 512))
        }
        _ => {
            let x = (salt as u32 % 7) * 3;
            let size = 1 + u32::from(arg);
            Region::Window(Window::new(x, x, x + size, x + size))
        }
    }
}

/// What `header()` must give on `prefix` under `options`, as text (`Header`
/// and `Error` have no `PartialEq`, and their `Debug` text holds every
/// field): `read_header_with` with `strict` off, an incomplete main header
/// as `None`.
fn oracle(prefix: &[u8], options: &DecodeOptions) -> String {
    format!(
        "{:?}",
        match read_header_with(prefix, &options.clone().with_strict(false)) {
            Ok(header) => Ok(Some(header)),
            Err(Error::Incomplete(_)) => Ok(None),
            Err(error) => Err(error),
        }
    )
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 12 {
        return;
    }
    let (c, data) = input.split_at(12);
    let mut options = DecodeOptions::default()
        .with_resolution_reduction(c[0] % 3)
        .with_strict(c[1] & 1 == 1)
        .with_region(region(c[2], c[3], data.len()));
    options = options.with_sample_limit(match c[1] >> 1 & 3 {
        0 => WorkLimit::Default,
        1 => WorkLimit::Raised(1 + u32::from(c[4] % 8)),
        2 => WorkLimit::Unlimited,
        _ => WorkLimit::AtMost(1 + (u64::from(c[4]) << 16)),
    });
    if c[5] & 1 == 1 {
        options = options.with_color(ColorOutput::Srgb);
    }
    if c[5] & 2 == 2 {
        options = options.with_raw_channels(true);
    }
    if c[5] & 4 == 4 {
        options = options.with_components(Some(vec![0]));
    }
    if c[5] & 8 == 8 {
        options = options.with_max_quality_layers(Some(1));
    }
    // Options the decoder refuses at the start leave nothing to check.
    let Ok(mut decoder) = IncrementalDecoder::new(options) else {
        return;
    };
    let mut rng = Rng(u64::from_le_bytes([
        c[6], c[7], c[8], c[9], c[10], c[11], 0, 0,
    ]));
    // About 24 feeds on average, so the oracle's reads from the start cost
    // about that many passes over the input.
    let most = (data.len() / 24).max(1);
    let mut fed = 0;
    while fed < data.len() {
        let piece = 1 + (rng.next() as usize) % (2 * most);
        let to = (fed + piece).min(data.len());
        decoder.feed(&data[fed..to]);
        fed = to;
        let r = rng.next();
        if r % 8 == 0 {
            let arg = (r >> 8) as u8;
            // A change that is refused leaves the options as they were, and
            // the oracle reads them back from the decoder.
            let _ = match (r >> 16) % 3 {
                0 => decoder.set_region(region((r >> 24) as u8, arg, fed)),
                1 => decoder.set_resolution_reduction(arg % 3),
                _ => decoder.set_max_quality_layers(Some(1 + u16::from(arg % 3))),
            };
        }
        let want = oracle(&data[..fed], decoder.options());
        let got = format!("{:?}", decoder.header());
        assert_eq!(got, want, "at {fed} of {}", data.len());
        if r % 5 == 0 {
            assert_eq!(format!("{:?}", decoder.header()), want, "again at {fed}");
        }
    }
});
