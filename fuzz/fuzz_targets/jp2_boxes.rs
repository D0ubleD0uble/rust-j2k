#![no_main]
//! Fuzz the JP2 box layer.
//!
//! The `decode` target would have to mutate its way to both a valid box
//! structure and a valid codestream. Here the codestream is pinned (one of two
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
//! - bit 4, which codestream: a 16x31 single-component one or a 23x15
//!   three-component one.
//!
//! `scripts/gen-jp2-fuzz-seeds.py` writes seeds in both shapes from the
//! ISO/IEC 15444-4 JP2 reader files.

use libfuzzer_sys::fuzz_target;

const ONE_COMPONENT: &[u8] = include_bytes!("../../tests/fixtures/regular_latlon_lossy.j2k");
const THREE_COMPONENTS: &[u8] =
    include_bytes!("../../tests/fixtures/multicomponent_subsampled_lossless.j2k");

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

fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    let codestream = if variant & 0x10 != 0 {
        THREE_COMPONENTS
    } else {
        ONE_COMPONENT
    };

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
    let _ = rust_j2k::decode(&wrap(variant, body));
});
