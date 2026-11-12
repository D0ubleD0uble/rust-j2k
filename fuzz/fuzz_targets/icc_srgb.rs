#![no_main]
//! Fuzz the ICC profile reader and the LittleCMS port behind
//! `ColorOutput::Srgb`.
//!
//! The codestream and the boxes are pinned; the fuzzer writes the ICC profile
//! in the `colr` box. Same contract as `decode`: a typed result, never a
//! panic, an allocation sized from a tag field, or a hang.
//!
//! Building the transform to sRGB costs milliseconds (a 33-point table of
//! 35,937 nodes, or the 8 and 16-bit tables), thousands of times the cost of
//! reading the profile, and it was most of an execution. So every input goes
//! through the profile reader alone (`rust_j2k::fuzz::icc_profile`), and one
//! input in `BUILD_EVERY`, chosen by a hash of the profile, also goes through
//! `decode_with` to build the transform and convert the pixels. The hash
//! keeps the choice stable for an input, so a seed is always either fast or
//! full, and a mutation moves between the two on its own.
//!
//! The first input byte picks the codestream: bits 0-1 choose 24x16 RGB at 8
//! bits, 64x64 RGB at 16 bits, or 64x64 gray at 8 bits (3 is RGB at 8 bits).
//! The rest is the profile.

use libfuzzer_sys::fuzz_target;
use rust_j2k::{ColorOutput, DecodeOptions};

const RGB8: &[u8] = include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
const RGB16: &[u8] = include_bytes!("../../tests/fixtures/icc/rgb16_lossless.j2k");
const GRAY8: &[u8] = include_bytes!("../../tests/fixtures/icc/gray8_lossless.j2k");

/// One profile in this many also builds the transform and decodes.
const BUILD_EVERY: u64 = 16;

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fn wrap(
    codestream: &[u8],
    size: (u32, u32),
    components: u16,
    depth: u8,
    profile: &[u8],
) -> Vec<u8> {
    let (width, height) = size;
    let mut ihdr = height.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&components.to_be_bytes());
    ihdr.extend_from_slice(&[depth - 1, 7, 0, 0]);
    let mut colr = vec![2, 0, 0];
    colr.extend_from_slice(profile);
    let mut jp2h = bx(b"ihdr", &ihdr);
    jp2h.extend(bx(b"colr", &colr));
    let mut file = vec![
        0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A,
    ];
    file.extend(bx(b"ftyp", b"jp2 \0\0\0\0jp2 "));
    file.extend(bx(b"jp2h", &jp2h));
    file.extend(bx(b"jp2c", codestream));
    file
}

fuzz_target!(|data: &[u8]| {
    let Some((&variant, profile)) = data.split_first() else {
        return;
    };
    rust_j2k::fuzz::icc_profile(profile);
    if fnv1a(profile) % BUILD_EVERY != 0 {
        return;
    }
    let (codestream, size, components, depth) = match variant & 0x03 {
        1 => (RGB16, (64, 64), 3, 16),
        2 => (GRAY8, (64, 64), 1, 8),
        _ => (RGB8, (24, 16), 3, 8),
    };
    let options = DecodeOptions::default().with_color(ColorOutput::Srgb);
    let _ = rust_j2k::decode_with(
        &wrap(codestream, size, components, depth, profile),
        &options,
    );
});
