#![no_main]
//! Fuzz the ICC profile reader and the LittleCMS port behind
//! `ColourOutput::Srgb`.
//!
//! The codestream and the boxes are pinned; the fuzzer writes the ICC profile
//! in the `colr` box, so every execution reaches the profile parser and,
//! for a profile it accepts, builds and runs the transform. Same contract as
//! `decode`: a typed result, never a panic, an allocation sized from a tag
//! field, or a hang.
//!
//! The first input byte picks the codestream: bits 0-1 choose 24x16 RGB at 8
//! bits, 64x64 RGB at 16 bits, or 64x64 grey at 8 bits (3 is RGB at 8 bits).
//! The rest is the profile.

use libfuzzer_sys::fuzz_target;
use rust_j2k::{ColourOutput, DecodeOptions};

const RGB8: &[u8] = include_bytes!("../../tests/fixtures/multicomponent_lossless.j2k");
const RGB16: &[u8] = include_bytes!("../../tests/fixtures/icc/rgb16_lossless.j2k");
const GREY8: &[u8] = include_bytes!("../../tests/fixtures/icc/grey8_lossless.j2k");

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
    let (codestream, size, components, depth) = match variant & 0x03 {
        1 => (RGB16, (64, 64), 3, 16),
        2 => (GREY8, (64, 64), 1, 8),
        _ => (RGB8, (24, 16), 3, 8),
    };
    let options = DecodeOptions::default().with_colour(ColourOutput::Srgb);
    let _ = rust_j2k::decode_with(&wrap(codestream, size, components, depth, profile), options);
});
