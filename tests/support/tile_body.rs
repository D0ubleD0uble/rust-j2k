//! The valid 64x64 codestream scaffolding the `tile_body` fuzz target wraps
//! around its input. Std only; the fuzz target and `tests/tile_body_variants.rs`
//! include it by `#[path]`, next to `synth.rs` as `mod synth`.

#![allow(dead_code)]

use super::synth::seg;

/// A full `SOT` marker segment. `rest_len` is everything the tile-part holds
/// after this segment — any tile-part-header markers, the two `SOD` bytes, and
/// the packet data — so `Psot` (which counts from the marker's first byte) is
/// `12 + rest_len`.
pub fn sot(isot: u16, tpsot: u8, tnsot: u8, rest_len: usize) -> Vec<u8> {
    super::synth::sot(isot, (12 + rest_len) as u32, tpsot, tnsot)
}

/// Pop the first byte of `*body`, or 0 when it is exhausted, so a short input
/// still assembles a complete codestream.
fn steer(body: &mut &[u8]) -> u8 {
    match body.split_first() {
        Some((&b, rest)) => {
            *body = rest;
            b
        }
        None => 0,
    }
}

/// A valid 64×64 main header + tile-part scaffolding around `body`.
///
/// `variant` bits: 0 wavelet (5/3 vs 9/7), 1-2 SOP/EPH delimiters (as `Scod`),
/// 3-5 geometry shape — 0 the single-component default, 1 three components with
/// the color transform, 2 three quality layers, 3 an explicit precinct
/// partition, 4 the bypass (lazy) code-block style, 5 POC volumes over the
/// widened geometry, 6 a 2×2 tile grid with interleaved tile-parts, 7 packed
/// packet headers (bit 6 picks PPM over PPT). Shapes 4, 5, and 7 read steering
/// bytes off the front of `body`. Every shape's main and tile-part headers
/// are read, whatever the body, so the body reaches the packet reader: the
/// `PPM` and `PPT` segments split only where A.7.4 and A.7.5 allow, and none
/// is empty. Bit 7 makes the code-blocks HT, and bit 6
/// with it (shapes 0 to 6) makes them MIXED; MIXED shape 4 drops the bypass and
/// terminate-each-pass flags, which A.4 forbids.
pub fn wrap(variant: u8, body: &[u8]) -> Vec<u8> {
    wrap_roi(variant, 0, body)
}

/// `wrap` with a region of interest: when `roi` bit 7 is set and the variant
/// is not HT (a `CAP` that says an `RGN` may be present is a header this does
/// not build), the main header gets one `RGN` marker segment (`Srgn` 0, the
/// maxshift) after the `QCD`. `SPrgn` is the low 5 bits of `roi` modulo 20, so
/// 0 to 19; bit 5 limits the marker to component 0, and otherwise every
/// component gets one. With bit 7 clear nothing is written. Packets then read as
/// they do for any coded region, with the shift raising each block's
/// bit-plane count, so the body reaches the ROI path through the real packet
/// reader.
pub fn wrap_roi(variant: u8, roi: u8, mut body: &[u8]) -> Vec<u8> {
    let reversible = variant & 1 != 0;
    // Variant bits 1-2 land on Scod bits 1 (SOP may precede packets) and 2
    // (EPH shall follow packet headers) directly, so the delimiter choice is
    // independent of the shape bits.
    let mut scod = variant & 0x06;
    let shape = (variant >> 3) & 0x07;
    let use_ppm = variant & 0x40 != 0; // shape 7 only: PPM instead of PPT

    // Shape 5 layers POC volumes over every axis at once, so it takes the
    // multi-component, multi-layer, and precinct geometry together.
    let components: u16 = if matches!(shape, 1 | 5) { 3 } else { 1 };
    let layers: u16 = if matches!(shape, 2 | 5) { 3 } else { 1 };
    let mct = u8::from(shape == 1); // color transform needs the three components
    let precincts = matches!(shape, 3 | 5);
    if precincts {
        scod |= 0x01; // Scod bit 0: precinct sizes follow in SPcod
    }
    const LEVELS: u8 = 2;

    // Shape 4: selective arithmetic coding bypass (LAZY, bit 0), OR-ed with a
    // fuzzed choice of the other five decoded style flags (reset, termall,
    // vcausal, pterm, segsym). Tier-2 then builds the lazy 10, 2, 1, 2, 1, …
    // codeword-segment split from the fuzzed packet headers and hands tier-1
    // raw and MQ segments through the real decode path.
    let mut style = if shape == 4 {
        0x01 | (steer(&mut body) & 0x3E)
    } else {
        0
    };
    // Variant bit 7: the HT code-block style (0x40) with a `CAP` marker, so the
    // body is read as HT packets (placeholder passes, cleanup and refinement
    // segments) and reaches the HT block coder through the real tier-2 split.
    let ht = variant & 0x80 != 0;
    // With the HT bit, variant bit 6 makes the style MIXED (0xC0) and the
    // `CAP`'s `Ccap15` bits 15-14 `11`, so each block is HT or Part 1 as its
    // length fields say (ISO/IEC 15444-15 A.4). Shape 7 keeps bit 6 for PPM.
    let mixed = ht && shape != 7 && variant & 0x40 != 0;
    if mixed {
        // A.4 allows a Part 1 block of a MIXED tile-component neither bypass
        // (LAZY) nor termination on each pass (TERMALL), and a header with
        // either is refused. The other shape 4 flags stay.
        style &= !0x05;
    }
    if ht {
        style |= if mixed { 0xC0 } else { 0x40 };
    }

    // Shape 6: a 32×32 tile on the 64×64 image — a 2×2 grid, four tiles.
    let multi_tile = shape == 6;
    let tile: u32 = if multi_tile { 32 } else { 64 };

    let mut siz = vec![0, 0]; // Rsiz
    for v in [64u32, 64, 0, 0, tile, tile, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&components.to_be_bytes()); // Csiz
    for _ in 0..components {
        siz.extend_from_slice(&[7, 1, 1]); // 8-bit unsigned, unit sampling
    }

    // LRCP, `layers` layer(s), color transform per `mct`, 2 levels, 2^6 blocks,
    // style per `shape`. Explicit precincts append one PPx/PPy byte per
    // resolution.
    let mut cod = vec![scod, 0];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[mct, LEVELS, 4, 4, style, u8::from(reversible)]);
    if precincts {
        // 2^4 × 2^4 at every resolution (non-zero above resolution 0, as A-21
        // requires).
        cod.extend(std::iter::repeat_n(0x44, usize::from(LEVELS) + 1));
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

    // Shape 5: a main-header POC whose volume count and per-volume bounds are
    // all fuzzed, masked so every volume passes the marker's field validation
    // and the *walk* gets driven — bounds may still overshoot the real
    // resolution/component/layer counts to hit the clamps, volumes may
    // duplicate and overlap to hit the dedup, and up to 32 volumes lean on the
    // volume-cap budget from below.
    let poc = (shape == 5).then(|| {
        let count = 1 + usize::from(steer(&mut body) & 0x1F); // 1..=32
        let mut p = Vec::with_capacity(count * 7);
        for _ in 0..count {
            let res_start = steer(&mut body) % 3;
            let res_end = res_start + 1 + steer(&mut body) % 3;
            let comp_start = steer(&mut body) % 3;
            let comp_end = comp_start + 1 + steer(&mut body) % 3;
            let layer_end = 1 + u16::from(steer(&mut body) % 5);
            let progression = steer(&mut body) % 5;
            p.push(res_start); // RSpoc
            p.push(comp_start); // CSpoc (one byte: < 257 components)
            p.extend_from_slice(&layer_end.to_be_bytes()); // LYEpoc
            p.push(res_end); // REpoc
            p.push(comp_end); // CEpoc
            p.push(progression); // Ppoc
        }
        p
    });

    // Shape 7: the first `cut` body bytes become the packed packet headers and
    // the rest stays inline as the packet bodies, so the fuzzer controls both
    // streams and where the boundary falls.
    let packed = if shape == 7 {
        let cut = usize::from(steer(&mut body)).min(body.len());
        let (packed, rest) = body.split_at(cut);
        body = rest;
        packed
    } else {
        &[]
    };

    let mut bytes = 0xFF4Fu16.to_be_bytes().to_vec(); // SOC
    bytes.extend_from_slice(&seg(0xFF51, &siz));
    if ht {
        // Part 15 only, one HT set per block, no `RGN`, and the loosest
        // magnitude bound (`B` = 74); MIXED sets `Ccap15` bits 15-14.
        let ccap15_high = if mixed { 0xC0 } else { 0 };
        bytes.extend_from_slice(&seg(0xFF50, &[0, 2, 0, 0, ccap15_high, 0x1F]));
    }
    bytes.extend_from_slice(&seg(0xFF52, &cod));
    bytes.extend_from_slice(&seg(0xFF5C, &quant));
    if roi & 0x80 != 0 && !ht {
        let shift = (roi & 0x1F) % 20;
        let covered = if roi & 0x20 != 0 { 1 } else { components };
        for component in 0..covered {
            // `Crgn` is one byte below 257 components.
            bytes.extend_from_slice(&seg(0xFF5E, &[component as u8, 0, shift]));
        }
    }
    if let Some(poc) = poc {
        bytes.extend_from_slice(&seg(0xFF5F, &poc));
    }
    if shape == 7 && use_ppm {
        // PPM: `Nppm` (the chunk length for the single tile-part) + the packed
        // headers. With any packed bytes they are carried by two marker
        // segments emitted in reverse `Zppm` order, split inside the chunk's
        // bytes, so the stitch (sort by `Zppm`, join, then read chunk lengths)
        // is exercised end to end. The split never falls inside `Nppm`, which
        // A.7.4 forbids, and neither segment is empty (`Lppm` 3 is refused).
        // An empty chunk (`Nppm` 0) is one segment.
        let mut joined = (packed.len() as u32).to_be_bytes().to_vec();
        joined.extend_from_slice(packed);
        if packed.is_empty() {
            bytes.extend_from_slice(&seg(0xFF60, &[&[0u8], &joined[..]].concat()));
        } else {
            let mid = 4 + packed.len() / 2;
            bytes.extend_from_slice(&seg(0xFF60, &[&[1u8], &joined[mid..]].concat()));
            bytes.extend_from_slice(&seg(0xFF60, &[&[0u8], &joined[..mid]].concat()));
        }
    }

    if multi_tile {
        // Two tile-parts per tile, interleaved across the grid: every tile's
        // part 0 in tile order, then every tile's part 1 — the arrival order a
        // resolution-progressive multi-tile encoder emits, and the one that
        // drives the per-tile accumulation and concatenation hardest.
        let chunk = |i: usize| &body[body.len() * i / 8..body.len() * (i + 1) / 8];
        for part in 0..2u8 {
            for tile_index in 0..4u16 {
                let data = chunk(usize::from(part) * 4 + usize::from(tile_index));
                bytes.extend_from_slice(&sot(tile_index, part, 2, 2 + data.len()));
                bytes.extend_from_slice(&0xFF93u16.to_be_bytes()); // SOD
                bytes.extend_from_slice(data);
            }
        }
    } else {
        // Any tile-part-header markers + SOD + data, so Psot can count them.
        let mut rest = Vec::new();
        if shape == 7 && !use_ppm {
            // PPT: the packed headers split across two marker segments emitted
            // in reverse `Zppt` order, exercising the in-tile-part stitch. A
            // `PPT` holds at least one byte (`Lppt` 3 is refused), so one byte
            // is one segment, and no packed byte is no `PPT`: the headers are
            // then inline.
            if packed.len() >= 2 {
                let mid = packed.len() / 2;
                rest.extend_from_slice(&seg(0xFF61, &[&[1u8], &packed[mid..]].concat()));
                rest.extend_from_slice(&seg(0xFF61, &[&[0u8], &packed[..mid]].concat()));
            } else if !packed.is_empty() {
                rest.extend_from_slice(&seg(0xFF61, &[&[0u8], packed].concat()));
            }
        }
        rest.extend_from_slice(&0xFF93u16.to_be_bytes()); // SOD
        rest.extend_from_slice(body);
        bytes.extend_from_slice(&sot(0, 0, 1, rest.len()));
        bytes.extend_from_slice(&rest);
    }
    bytes.extend_from_slice(&0xFFD9u16.to_be_bytes()); // EOC
    bytes
}
