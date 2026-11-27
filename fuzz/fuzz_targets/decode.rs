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
//! per-component decode are fuzzed as well. The same hash also raises the
//! packet work bound on the decode with `strict` off, and asks a JP2 file's
//! window or tile decode for sRGB, which a sub-sampled sYCC image refuses
//! there. The hash changes with any byte, so `tmin` of a crash can flip these.
//!
//! An input whose decodes are refused at the packet work bound is held to 5
//! packet bounds (`budget::PacketBounds`): the first decode refused there,
//! and the one with `strict` off at its factor. Where the plain decode is
//! the one refused, the strict decode is not run, and the part runs only
//! where a bound is left.

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
use inconsistent::checked;

#[path = "../common/budget.rs"]
mod budget;
#[path = "../common/partial.rs"]
mod partial;

fuzz_target!(|data: &[u8]| {
    // Up to four decodes of the input run one after another: the plain one, the
    // strict one, the lenient one in `partial::check`, and `part`. The strict
    // image is held while the lenient one decodes.
    if budget::over(data, 2) {
        return;
    }
    // In code-block bounds: the plain decode at 2, so that mid-size images
    // reach pixels, and the other three at 1 each.
    let base = budget::options(5);
    // In packet bounds, once a decode is refused at the packet bound: that
    // decode (the plain or the strict one) and the one with `strict` off at
    // up to 4.
    let mut packets = budget::PacketBounds::new(5);
    let plain = base
        .clone()
        .with_code_block_work_limit(rust_j2k::WorkLimit::Raised(2));
    // Only the plain decode's packet refusal is kept, never its image, so
    // no more images are held at once than the sample budget allows.
    let plain_refusal = match checked(rust_j2k::decode_with(data, &plain)) {
        Err(error) if budget::packet_refusal(&error) => Some(error),
        _ => None,
    };
    packets.charge(1, plain_refusal.is_some());
    let hash = fnv1a(data);
    if hash & 1 == 1 {
        let layers = 1 + ((hash >> 1) % 3) as u16;
        let options = base.clone().with_max_quality_layers(Some(layers));
        // The strict decode under a layer cap walks the packets the plain
        // decode walks: a layer cap cuts code-block work and not packet work
        // (`a_layer_cap_cuts_block_work_and_not_packet_work` in
        // `src/pipeline/decode.rs` holds that). So where the plain decode was
        // refused at the packet bound, the strict one fails too, at that
        // bound or earlier at the code-block bound. The plain refusal then
        // stands in for it, and the bound goes to the decode with `strict`
        // off. Otherwise it runs, and is charged.
        let strict = match &plain_refusal {
            Some(error) => Err(error.clone()),
            None => {
                let strict = checked(rust_j2k::decode_with(data, &options));
                packets.charge(1, strict.as_ref().is_err_and(budget::packet_refusal));
                strict
            }
        };
        // One input in four raises the packet work bound (2 or 4 times the
        // default) for the decode with `strict` off only, so the walks the
        // default refuses are decoded instead. A strict image must then
        // match the one with `strict` off, which a raised bound only makes
        // likelier to decode. One decode only: `Raised(4)` on every decode
        // would take an input that reaches the bound past the fuzz timeout.
        let (lenient, factor) = if hash & 0x60 == 0x60 {
            let factor = 2 + 2 * ((hash >> 7) & 1) as u32;
            let raised = rust_j2k::WorkLimit::Raised(factor);
            (options.with_packet_work_limit(raised), u64::from(factor))
        } else {
            (options, 1)
        };
        // The same cap with `strict` off, where a block's last kept pass may
        // be cut short by the cap (#358). That bug gave a wrong image, not a
        // panic, so the decode is checked against the strict one.
        let refused = partial::check(data, &lenient, &strict);
        packets.charge(factor, refused);
    }
    if hash & 2 == 2 && packets.allows(1) {
        let mut part = part(base, hash >> 8);
        // For a JP2 file, one in four of the window and tile ones asks for
        // sRGB too, which a window or a tile of a sub-sampled sYCC image
        // refuses (its chroma replication needs the whole image). A
        // codestream has no color space to convert from.
        if hash & 0x18 == 0x18 && (hash >> 8) & 3 < 2 && data.get(4..8) == Some(b"jP  ".as_slice())
        {
            part = part.with_color(rust_j2k::ColorOutput::Srgb);
        }
        let _ = checked(rust_j2k::decode_with(data, &part));
    }
});

/// A window, a tile or a component subset, with a reduction of 0 or 1, from
/// the bits of `bits`. The window's corners are small numbers, so that they
/// land inside the small images the fuzzer makes.
fn part(base: rust_j2k::DecodeOptions, bits: u64) -> rust_j2k::DecodeOptions {
    let small = |shift: u32| ((bits >> shift) & 0x3F) as u32;
    let options = base.with_resolution_reduction(((bits >> 2) & 1) as u8);
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
