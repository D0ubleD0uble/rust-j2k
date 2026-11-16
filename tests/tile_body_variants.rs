//! The code-block style each `tile_body` fuzz variant builds into its pinned
//! header: the `CAP` marker segment and the `COD` code-block style byte. A
//! change to either would leave the target fuzzing a different tier-2 path
//! with no failure to say so.

#[allow(clippy::duplicate_mod)]
#[path = "support/synth.rs"]
mod synth;
#[allow(clippy::duplicate_mod)]
#[path = "support/tile_body.rs"]
mod tile_body;

use tile_body::wrap;

/// The `CAP` segment (marker, `Lcap`, `Pcap`, `Ccap15`) of the codestream
/// `wrap` builds, if it has one, and the `COD` code-block style byte.
fn cap_and_style(variant: u8) -> (Option<Vec<u8>>, u8) {
    let bytes = wrap(variant, &[]);
    let at = |marker: [u8; 2]| bytes.windows(2).position(|w| w == marker);
    let cap = at([0xFF, 0x50]).map(|i| bytes[i..i + 10].to_vec());
    // `COD`: marker, `Lcod`, `Scod`, progression, layers (2), transform,
    // levels, code-block width and height, then the code-block style.
    let cod = at([0xFF, 0x52]).expect("COD");
    (cap, bytes[cod + 4 + 1 + 1 + 2 + 1 + 1 + 2])
}

#[test]
fn the_variants_pick_the_code_block_style() {
    // Part 1: no `CAP`, style 0.
    assert_eq!(cap_and_style(0x00), (None, 0x00));
    // HT: `Pcap` bit 17, `Ccap15` 0x001F (B = 74), the HT style bit.
    let ht = vec![0xFF, 0x50, 0, 8, 0, 2, 0, 0, 0x00, 0x1F];
    assert_eq!(cap_and_style(0x80), (Some(ht.clone()), 0x40));
    // MIXED: `Ccap15` bits 15-14 are `11`, the style adds the mixed bit.
    let mixed = vec![0xFF, 0x50, 0, 8, 0, 2, 0, 0, 0xC0, 0x1F];
    assert_eq!(cap_and_style(0xC0), (Some(mixed.clone()), 0xC0));
    // The other shapes keep it, and a wavelet or delimiter bit does not
    // change it.
    assert_eq!(cap_and_style(0xC0 | 1 | 0b110), (Some(mixed), 0xC0));
    assert_eq!(cap_and_style(0x80 | 0x08), (Some(ht.clone()), 0x40));
    // Shape 7 reads bit 6 as PPM, so it stays HT. Bit 6 without the HT bit
    // is no coder choice.
    assert_eq!(cap_and_style(0xC0 | (7 << 3)), (Some(ht), 0x40));
    assert_eq!(cap_and_style(0x40), (None, 0x00));
}
