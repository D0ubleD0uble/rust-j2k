//! The code-block style each `tile_body` fuzz variant builds into its pinned
//! header: the `CAP` marker segment and the `COD` code-block style byte. A
//! change to either would leave the target fuzzing a different tier-2 path
//! with no failure to say so.

#[path = "support/synth.rs"]
mod synth;
#[path = "support/tile_body.rs"]
mod tile_body;

use tile_body::wrap;

/// The `CAP` segment (marker, `Lcap`, `Pcap`, `Ccap15`) of the codestream
/// `wrap` builds, if it has one, and the `COD` code-block style byte.
fn cap_and_style(variant: u8) -> (Option<Vec<u8>>, u8) {
    cap_and_style_of(variant, &[])
}

fn cap_and_style_of(variant: u8, body: &[u8]) -> (Option<Vec<u8>>, u8) {
    let bytes = wrap(variant, body);
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
    // A wavelet or delimiter bit, or another shape (here 1), does not change
    // it.
    assert_eq!(cap_and_style(0xC0 | 1 | 0b110), (Some(mixed.clone()), 0xC0));
    assert_eq!(cap_and_style(0xC0 | 0x08), (Some(mixed.clone()), 0xC0));
    assert_eq!(cap_and_style(0x80 | 0x08), (Some(ht.clone()), 0x40));
    // Shape 7 reads bit 6 as PPM, so it stays HT. Bit 6 without the HT bit
    // is no coder choice.
    assert_eq!(cap_and_style(0xC0 | (7 << 3)), (Some(ht), 0x40));
    assert_eq!(cap_and_style(0x40), (None, 0x00));
}

/// Shape 4 steers the other style flags from the body's first byte. Under
/// HT they are OR-ed with the HT bit as they are; under MIXED the bypass and
/// terminate-each-pass flags (bits 0 and 2) go, since A.4 refuses a header
/// with either, so the variant still reaches tier-2.
#[test]
fn mixed_shape_4_drops_the_flags_a4_forbids() {
    let (_, style) = cap_and_style_of(0xC0 | (4 << 3), &[0xFF]);
    assert_eq!(style, 0xC0 | 0x3A);
    let (_, style) = cap_and_style_of(0x80 | (4 << 3), &[0xFF]);
    assert_eq!(style, 0x40 | 0x3F);
    // Every MIXED variant's header is read.
    for variant in 0xC0..=0xFFu8 {
        if (variant >> 3) & 7 == 7 {
            continue;
        }
        let bytes = wrap(variant, &[0xFF; 8]);
        rust_j2k::read_header(&bytes).unwrap_or_else(|e| panic!("{variant:#04x}: {e}"));
    }
}

/// A nonzero `roi` byte writes one `RGN` per covered component after the
/// `QCD`; the HT variants and a zero byte write none. The header is still
/// read.
#[test]
fn the_roi_byte_writes_rgn_markers() {
    let count = |bytes: &[u8]| bytes.windows(2).filter(|w| *w == [0xFF, 0x5E]).count();
    let rgn_at = |bytes: &[u8]| bytes.windows(2).position(|w| w == [0xFF, 0x5E]);
    assert_eq!(count(&tile_body::wrap_roi(0x00, 0, &[])), 0);
    assert_eq!(tile_body::wrap_roi(0x00, 0, &[1]), wrap(0x00, &[1]));
    // One component: Lrgn 5, Crgn 0, Srgn 0, SPrgn 7.
    let one = tile_body::wrap_roi(0x00, 7, &[]);
    assert_eq!(count(&one), 1);
    let at = rgn_at(&one).unwrap();
    assert_eq!(&one[at..at + 7], [0xFF, 0x5E, 0, 5, 0, 0, 7]);
    // Three components (shape 1), all or only the first.
    assert_eq!(count(&tile_body::wrap_roi(1 << 3, 7, &[])), 3);
    assert_eq!(count(&tile_body::wrap_roi(1 << 3, 0x20 | 7, &[])), 1);
    // HT: no `RGN`.
    assert_eq!(count(&tile_body::wrap_roi(0x80, 7, &[])), 0);
    for variant in 0..=0x3Fu8 {
        let bytes = tile_body::wrap_roi(variant, 0x1F, &[0xFF; 8]);
        rust_j2k::read_header(&bytes).unwrap_or_else(|e| panic!("{variant:#04x}: {e}"));
    }
}
