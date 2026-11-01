#![no_main]
//! Fuzz the JP2 box layer.
//!
//! The `decode` target would have to mutate its way to both a valid box
//! structure and a valid codestream. Here the codestream is pinned (one of six
//! small committed fixtures) and the fuzzer controls the boxes around it, so
//! every execution reaches the box reader and a valid file is one mutation
//! away from a full decode. Same contract as `decode`: a typed result, never a
//! panic, an allocation sized from a box length, or a hang.
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
//!   sYCC 4:2:0 and 4:2:2 codestreams. Values 6 and 7 pick 0. The 8-bit ones
//!   are what the ICC profile paths of `ColourOutput::Srgb` need: a
//!   monochrome profile applies at 8 bits and an RGB one at 8 or 16 bits with
//!   three channels of one format.
//!
//! Every input that decodes as coded is decoded again with
//! `ColourOutput::Srgb`, which reads the `colr` profile bytes. That result is
//! only checked for being a result: it may differ from the first, and most
//! inputs end in a typed `Unsupported`.
//!
//! `scripts/gen-jp2-fuzz-seeds.py` writes seeds in both shapes from the
//! ISO/IEC 15444-4 JP2 reader files, plus built ICC and sYCC `colr` seeds.

use libfuzzer_sys::fuzz_target;

const ONE_COMPONENT: &[u8] = include_bytes!("../../tests/fixtures/regular_latlon_lossy.j2k");
const THREE_COMPONENTS: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_subsampled_lossless.j2k");
const ONE_COMPONENT_8: &[u8] = include_bytes!("../../tests/fixtures/derived_quant_lossy.j2k");
const THREE_COMPONENTS_8: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
const SYCC_420: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_420_odd.jp2");
const SYCC_422: &[u8] = include_bytes!("../../tests/fixtures/jp2_sycc_422_odd.jp2");

const SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// A box with a 32-bit `LBox`. The fuzz input is far below 4 GiB.
fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// An `ihdr` box matching a codestream's SIZ: the image area and `Csiz`.
fn ihdr_for(codestream: &[u8]) -> Vec<u8> {
    let be32 = |at: usize| u32::from_be_bytes(codestream[at..at + 4].try_into().unwrap());
    let (xsiz, ysiz, x0, y0) = (be32(8), be32(12), be32(16), be32(20));
    let mut payload = (ysiz - y0).to_be_bytes().to_vec();
    payload.extend_from_slice(&(xsiz - x0).to_be_bytes());
    payload.extend_from_slice(&codestream[40..42]); // Csiz
    payload.extend_from_slice(&[7, 7, 0, 0]); // BPC, C, UnkC, IPR
    bx(b"ihdr", &payload)
}

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
        _ => ONE_COMPONENT,
    }
}

fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    let codestream = codestream(variant);

    let mut file = Vec::new();
    if variant & 0x04 != 0 {
        file.extend_from_slice(&SIGNATURE);
        file.extend_from_slice(&bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
        if variant & 0x08 != 0 {
            let mut children = ihdr_for(codestream);
            children.extend_from_slice(body);
            file.extend_from_slice(&bx(b"jp2h", &children));
        } else {
            file.extend_from_slice(body);
        }
    } else {
        file.extend_from_slice(body);
    }

    match variant & 0x03 {
        0 => file.extend_from_slice(&bx(b"jp2c", codestream)),
        1 => {
            file.extend_from_slice(&1u32.to_be_bytes());
            file.extend_from_slice(b"jp2c");
            file.extend_from_slice(&(16 + codestream.len() as u64).to_be_bytes());
            file.extend_from_slice(codestream);
        }
        2 => {
            file.extend_from_slice(&0u32.to_be_bytes());
            file.extend_from_slice(b"jp2c");
            file.extend_from_slice(codestream);
        }
        _ => {
            file.extend_from_slice(&bx(b"jp2c", codestream));
            file.extend_from_slice(body);
        }
    }
    file
}

fuzz_target!(|data: &[u8]| {
    let Some((&variant, body)) = data.split_first() else {
        return;
    };
    let file = wrap(variant, body);
    // A file that fails as coded fails before any colour conversion, so only
    // one that decodes goes through the profile reader and the sYCC and ICC
    // conversions. The sRGB result may be a typed error, and its samples
    // differ from the first decode's when converted; these checks hold either
    // way.
    let Ok(coded) = rust_j2k::decode(&file) else {
        return;
    };
    let mut options = rust_j2k::DecodeOptions::default();
    options.colour = rust_j2k::ColourOutput::Srgb;
    let Ok(srgb) = rust_j2k::decode_with(&file, options) else {
        return;
    };
    assert_eq!(
        srgb.components.len(),
        coded.components.len(),
        "channel count"
    );
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
    // Colour left as declared means the samples were passed through.
    let colour = |i: &rust_j2k::Image| i.jp2.as_ref().map(|m| m.colour.clone());
    if colour(&srgb) == colour(&coded) {
        assert_eq!(
            srgb.components, coded.components,
            "pass-through changed samples"
        );
    }
});
