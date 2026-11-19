#![no_main]
//! Fuzz the JP2 box layer.
//!
//! The `decode` target would have to mutate its way to both a valid box
//! structure and a valid codestream. Here the codestream is pinned (one of
//! eight small committed fixtures) and the fuzzer controls the boxes around
//! it, so every execution reaches the box reader and a valid file is one
//! mutation away from a full decode. Same contract as `decode`: a typed
//! result, never a panic, an allocation sized from a box length, or a hang.
//!
//! The first input byte (`variant`) steers the shape; the rest is `body`:
//!
//! - bits 0-1, how the `jp2c` box holding the pinned codestream is written:
//!   0 a 32-bit `LBox`, 1 a 64-bit `XLBox`, 2 `LBox = 0` (to the end of the
//!   file), 3 a 32-bit `LBox` followed by `body` again (boxes after the
//!   codestream, which are walked and their header boxes read again);
//! - bit 2, a pinned prefix: the signature and a `jp2 ` `ftyp` come first and
//!   `body` follows them. Clear, `body` is the whole file before `jp2c`,
//!   signature included (the shape of the seeds cut from real files);
//! - bit 3 (with bit 2), a pinned header: `body` becomes the children of a
//!   `jp2h` box after an `ihdr` that matches the codestream, so every mutation
//!   lands in the `jp2h` child walk (`colr`, `pclr`, `cmap`, `cdef`, ...);
//! - bits 4-6, which codestream (see `codestream`): 0 a 16x31 one-component
//!   16-bit one, 1 a 23x15 three-component one sub-sampled 2x2, 2 a 32x24
//!   one-component 8-bit one, 3 a 24x16 three-component 8-bit one, 4 and 5 the
//!   sYCC 4:2:0 and 4:2:2 codestreams at the odd origin (5, 3), 6 an 11x9
//!   four-component 16-bit one, 7 the sYCC 4:2:0 codestream at the even
//!   origin (6, 2). The 8-bit ones are what the ICC profile paths of
//!   `ColorOutput::Srgb` need: a monochrome profile applies at 8 bits and an
//!   RGB one at 8 or 16 bits with three channels of one format;
//! - bit 7, every decode is one resolution level down
//!   (`resolution_reduction = 1`).
//!
//! Every input is decoded with `raw_channels` (no palette, no channel
//! definitions), and every input that decodes as coded is decoded again with
//! `ColorOutput::Srgb`, which reads the `colr` profile bytes. Without a
//! `pclr`, `cmap` or `cdef` the `raw_channels` image must equal the default
//! one, or the raw decode may refuse only the boxes after the codestream.
//! The `Srgb` results are only checked for being a result: they may differ
//! from the first, and most end in a typed `Unsupported`.
//!
//! `scripts/gen-jp2-fuzz-seeds.py` writes seeds in both shapes from the
//! ISO/IEC 15444-4 JP2 reader files, plus built ICC and sYCC `colr` seeds.

#[path = "../../tests/support/synth.rs"]
mod synth;
use libfuzzer_sys::fuzz_target;

#[path = "../common/budget.rs"]
mod budget;

const ONE_COMPONENT: &[u8] = include_bytes!("../../tests/fixtures/regular_latlon_lossy.j2k");
const THREE_COMPONENTS: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_subsampled_lossless.j2k");
const ONE_COMPONENT_8: &[u8] = include_bytes!("../../tests/fixtures/derived_quant_lossy.j2k");
const THREE_COMPONENTS_8: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
const SYCC_420: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_420_odd.jp2");
const SYCC_422: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_422_odd.jp2");
const FOUR_COMPONENTS_16: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_16bit_lossless.j2k");
const SYCC_420_EVEN: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_420_even.jp2");

/// The `jp2c` payload of a committed JP2 file whose boxes all have a 32-bit
/// `LBox`.
fn jp2c_payload(file: &[u8]) -> &[u8] {
    let mut at = 0;
    while at + 8 <= file.len() {
        let len = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        if &file[at + 4..at + 8] == b"jp2c" {
            return &file[at + 8..at + len];
        }
        at += len;
    }
    unreachable!("the committed fixture has a jp2c box")
}

fn codestream(variant: u8) -> &'static [u8] {
    match (variant >> 4) & 7 {
        1 => THREE_COMPONENTS,
        2 => ONE_COMPONENT_8,
        3 => THREE_COMPONENTS_8,
        4 => jp2c_payload(SYCC_420),
        5 => jp2c_payload(SYCC_422),
        6 => FOUR_COMPONENTS_16,
        7 => jp2c_payload(SYCC_420_EVEN),
        _ => ONE_COMPONENT,
    }
}

fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    synth::wrap_jp2_boxes(variant, codestream(variant), body)
}

fuzz_target!(|data: &[u8]| {
    let Some((&variant, body)) = data.split_first() else {
        return;
    };
    let file = wrap(variant, body);
    // `body` may carry its own `jp2c`, which wins over the pinned one, so the
    // budget counts the whole file. It also keeps the three images below
    // (`raw`, `coded`, `srgb`) from adding up past the RSS limit.
    if budget::over(&file, 4) {
        return;
    }
    let reduction = variant >> 7;
    let options = rust_j2k::DecodeOptions::default().with_resolution_reduction(reduction);
    // The codestream's own channels, whatever the palette and channel
    // definitions say.
    let raw = rust_j2k::decode_with(&file, &options.clone().with_raw_channels(true));
    if let Ok(raw) = &raw {
        assert!(!raw.jp2.as_ref().is_some_and(|m| m.palette_applied));
        for c in &raw.components {
            assert_eq!(
                c.samples.len() as u64,
                u64::from(c.width) * u64::from(c.height)
            );
        }
    }
    // A file that fails as coded fails before any color conversion, so only
    // one that decodes goes through the profile reader and the sYCC and ICC
    // conversions. The sRGB result may be a typed error, and its samples
    // differ from the first decode's when converted; these checks hold either
    // way.
    let Ok(coded) = rust_j2k::decode_with(&file, &options) else {
        return;
    };
    // With no `pclr`, `cmap` or `cdef`, the default decode's channels are the
    // codestream's own, so the raw decode gives the same image. Its one
    // legitimate raw-only refusal is the check on the boxes after the
    // codestream: the default decode drops a `pclr` that has no `cmap`, but
    // `raw_channels` keeps it, so a second `pclr` after the codestream is an
    // error only there. Any other raw-only error is a bug.
    let boxes = coded.jp2.as_ref().is_some_and(|m| {
        m.palette.is_some() || m.channel_mapping.is_some() || m.channel_definitions.is_some()
    });
    if !boxes {
        match &raw {
            Ok(raw) => assert_eq!(raw.components, coded.components, "raw channels"),
            Err(e) => assert!(
                matches!(e, rust_j2k::Error::FileFormat(m) if m.message().starts_with("after the codestream")),
                "raw-only error: {e:?}"
            ),
        }
    }
    let options = options.with_color(rust_j2k::ColorOutput::Srgb);
    let Ok(srgb) = rust_j2k::decode_with(&file, &options) else {
        return;
    };
    let color = |i: &rust_j2k::Image| i.jp2.as_ref().map(|m| m.color.clone());
    // One exception to equal channel counts: a gray ICC profile whose sRGB
    // output is not neutral gives R, G and B in place of the gray channel,
    // two more channels, reported as sRGB, as OpenJPEG does. Only on one or
    // two coded channels: a gray profile on more is refused.
    let gray_profile = coded.jp2.as_ref().is_some_and(|m| match &m.color {
        rust_j2k::ColorSpace::RestrictedIcc(p) => p.get(16..20) == Some(b"GRAY"),
        _ => false,
    });
    let non_neutral_gray = gray_profile
        && color(&srgb) == Some(rust_j2k::ColorSpace::Srgb)
        && coded.components.len() <= 2
        && srgb.components.len() == coded.components.len() + 2;
    if !non_neutral_gray {
        assert_eq!(
            srgb.components.len(),
            coded.components.len(),
            "channel count"
        );
    }
    let palette = srgb.jp2.as_ref().is_some_and(|m| m.palette_applied);
    for c in &srgb.components {
        assert_eq!(
            c.samples.len() as u64,
            u64::from(c.width) * u64::from(c.height)
        );
        // A palette channel's samples may fall outside its declared depth.
        if !c.signed && !palette {
            let max = (1i64 << c.bit_depth) - 1;
            assert!(
                c.samples.iter().all(|&v| (0..=max).contains(&i64::from(v))),
                "sample outside its bit depth"
            );
        }
    }
    // Color left as declared means the samples were passed through.
    if color(&srgb) == color(&coded) {
        assert_eq!(
            srgb.components, coded.components,
            "pass-through changed samples"
        );
    }
});
