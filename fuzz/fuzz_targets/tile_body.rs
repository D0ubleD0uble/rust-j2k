#![no_main]
//! Fuzz tier-2 packet parsing and everything downstream of it.
//!
//! The `decode` target must mutate its way through a byte-perfect main header
//! before anything reaches the packet reader, so most executions die in
//! `parse_main_header`. Here the header is pinned — a small valid image with
//! the first input byte steering the wavelet, the SOP/EPH delimiters, and the
//! **tile-component geometry** — and the rest of the input becomes the tile-part
//! body: packet headers, tag trees, code-block segments. Same contract as
//! `decode`: typed result or bust, never a panic, unbounded allocation, or hang.
//!
//! The `variant` byte also picks the geometry shape, so one fuzzed body reaches
//! the widened Phase 2 tier-2 paths — the multi-component + colour transform
//! walk, the multi-layer packet accumulation, and the precinct partition — not
//! only the single-component single-layer default. Multi-*tile* geometry is left
//! to the `decode` target (seeded with multi-tile codestreams): a tile-part body
//! is a single tile-part, so it cannot drive several tiles from here.

use libfuzzer_sys::fuzz_target;

/// `marker + Lmarker (counting itself) + body`.
fn seg(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut s = marker.to_be_bytes().to_vec();
    s.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    s.extend_from_slice(body);
    s
}

/// A valid single-tile 64×64 main header + SOT + SOD around `body`.
///
/// `variant` bits: 0 wavelet (5/3 vs 9/7), 1-2 SOP/EPH delimiters (as `Scod`),
/// 3-4 geometry shape — 0 the single-component default, 1 three components with
/// the colour transform, 2 three quality layers, 3 an explicit precinct
/// partition. Every shape stays a byte-perfect header so the body reaches the
/// packet reader.
fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    let reversible = variant & 1 != 0;
    let mut scod = (variant >> 1) & 0x06; // bits 1-2: SOP / EPH delimiters
    let shape = (variant >> 3) & 0x03;

    let components: u16 = if shape == 1 { 3 } else { 1 };
    let layers: u16 = if shape == 2 { 3 } else { 1 };
    let mct = u8::from(shape == 1); // colour transform needs the three components
    let precincts = shape == 3;
    if precincts {
        scod |= 0x01; // Scod bit 0: precinct sizes follow in SPcod
    }
    const LEVELS: u8 = 2;

    let mut siz = vec![0, 0]; // Rsiz
    for v in [64u32, 64, 0, 0, 64, 64, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&components.to_be_bytes()); // Csiz
    for _ in 0..components {
        siz.extend_from_slice(&[7, 1, 1]); // 8-bit unsigned, unit sampling
    }

    // LRCP, `layers` layer(s), colour transform per `mct`, 2 levels, 2^6 blocks,
    // default style. Explicit precincts append one PPx/PPy byte per resolution.
    let mut cod = vec![scod, 0];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[mct, LEVELS, 4, 4, 0, u8::from(reversible)]);
    if precincts {
        // 2^4 × 2^4 at every resolution (non-zero above resolution 0, as A-21
        // requires).
        cod.extend(std::iter::repeat(0x44).take(usize::from(LEVELS) + 1));
    }

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
