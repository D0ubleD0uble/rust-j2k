#![no_main]
//! Fuzz tier-2 packet parsing and everything downstream of it.
//!
//! The `decode` target must mutate its way through a byte-perfect main header
//! before anything reaches the packet reader, so most executions die in
//! `parse_main_header`. Here the header is pinned — a small valid image with
//! the first input byte steering the wavelet and the SOP/EPH delimiters — and
//! the rest of the input becomes the tile-part body: packet headers, tag
//! trees, code-block segments. Same contract as `decode`: typed result or
//! bust, never a panic, unbounded allocation, or hang.

use libfuzzer_sys::fuzz_target;

/// `marker + Lmarker (counting itself) + body`.
fn seg(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut s = marker.to_be_bytes().to_vec();
    s.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(body);
    s
}

/// A valid single-tile 64×64 main header + SOT + SOD around `body`.
fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    let reversible = variant & 1 != 0;
    let scod = (variant >> 1) & 0x06; // bits 1-2: SOP / EPH delimiters

    let mut siz = vec![0, 0]; // Rsiz
    for v in [64u32, 64, 0, 0, 64, 64, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&1u16.to_be_bytes()); // Csiz
    siz.extend_from_slice(&[7, 1, 1]); // 8-bit unsigned, unit sampling

    // LRCP, 1 layer, no MCT, 2 levels, 2^6 blocks, default style.
    let cod = vec![scod, 0, 0, 1, 0, 2, 4, 4, 0, u8::from(reversible)];

    let quant: Vec<u8> = if reversible {
        // Style 0: one exponent byte per subband (3·2 + 1 = 7).
        std::iter::once(2 << 5).chain([8 << 3; 7]).collect()
    } else {
        // Scalar expounded: a 16-bit (exponent, mantissa) per subband.
        let mut q = vec![(2 << 5) | 2];
        for _ in 0..7 {
            q.extend_from_slice(&((10u16 << 11) | 42).to_be_bytes());
        }
        q
    };

    let mut sot = 0u16.to_be_bytes().to_vec(); // Isot
    sot.extend_from_slice(&((12 + 2 + body.len()) as u32).to_be_bytes()); // Psot
    sot.extend_from_slice(&[0, 1]); // TPsot, TNsot

    let mut bytes = 0xFF4Fu16.to_be_bytes().to_vec(); // SOC
    bytes.extend_from_slice(&seg(0xFF51, &siz));
    bytes.extend_from_slice(&seg(0xFF52, &cod));
    bytes.extend_from_slice(&seg(0xFF5C, &quant));
    bytes.extend_from_slice(&seg(0xFF90, &sot));
    bytes.extend_from_slice(&0xFF93u16.to_be_bytes()); // SOD
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(&0xFFD9u16.to_be_bytes()); // EOC
    bytes
}

fuzz_target!(|data: &[u8]| {
    let Some((&variant, body)) = data.split_first() else {
        return;
    };
    let _ = rust_j2k::decode(&wrap(variant, body));
});
