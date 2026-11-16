//! Code-blocks first included at layer 999 or later.
//!
//! OpenJPEG 2.5.0 starts every tag-tree node at the value 999 (`tgt.c`), so
//! at a layer threshold of 1000 or more it takes a code-block that has not
//! been included yet as included, without reading the inclusion bit the
//! encoder wrote. The rest of that packet header is then read out of step:
//! `opj_decompress` refuses these files ("read: segment too long (2) with
//! max (0) for codeblock 1"). The standard puts no limit on the layer count
//! below the 65535 of `COD` (B.10.4, Table A-14), so this decoder reads the
//! bit, and the files decode.
//!
//! The oracle is the same code-block data in fewer layers. With one coding
//! pass per code-block, the layer a code-block is first included in does not
//! change its samples, so a file whose blocks are first included at layers
//! 0, 999 and never decodes as its twin with the layers 0, 1 and never.
//! OpenJPEG 2.5.0 decodes each twin to [`expected`]'s image
//! (`opj_decompress -i twin.j2k -o twin.pgm`, on the bytes [`codestream`]
//! writes for the twin); the twins are decoded here too, and their bytes are
//! pinned by SHA-256 so a change to the writer can't move them away from what
//! OpenJPEG decoded. So are two 999-layer files with a code-block first
//! included at layer 998, the last threshold OpenJPEG reads correctly: it
//! decodes both to [`expected`]'s image as well. Found in the review of
//! PR #249 with a 2048 x 2048, 1000-layer file built the same way.

mod support;
use support::synth::{self, Bits, TagTree, seg};

use rust_j2k::decode;
use support::jp2::sha256_hex;

/// A 12 x 4 image of one unsigned 8-bit component: one tile, no wavelet
/// levels, three 4 x 4 code-blocks in one precinct, `layers` LRCP layers.
/// Code-block `b` is first included at layer `first[b]`, if any, with no
/// zero bit-planes and one cleanup pass of the one byte 0x12.
fn codestream(layers: u16, first: [Option<u16>; 3]) -> Vec<u8> {
    let never = u32::MAX;
    let first = first.map(|f| f.map_or(never, u32::from));
    let mut inclusion = TagTree::new(&first);
    let mut zero_planes = TagTree::new(&[0; 3]);
    let mut data = Vec::new();
    for layer in 0..u32::from(layers) {
        let mut bits = Bits::new();
        bits.put(1);
        let mut bodies = 0;
        for (block, &at) in first.iter().enumerate() {
            if at < layer {
                // Included before: one bit, not included again.
                bits.put(0);
                continue;
            }
            inclusion.encode(&mut bits, block, layer + 1);
            if at == layer {
                zero_planes.encode(&mut bits, block, 999);
                bits.put(0); // one coding pass
                bits.put(0); // Lblock stays 3
                for bit in [0, 0, 1] {
                    bits.put(bit); // a length of 1 byte
                }
                bodies += 1;
            }
        }
        data.extend(bits.finish());
        data.extend(std::iter::repeat_n(0x12u8, bodies));
    }

    let siz = synth::siz_body([12, 4, 0, 0, 12, 4, 0, 0], &[[7, 1, 1]]);
    let cod = synth::cod_body(layers, 0, 0, 0);
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(seg(0xFF51, &siz));
    bytes.extend(seg(0xFF52, &cod));
    bytes.extend(seg(0xFF5C, &synth::qcd_none_body(2, &[8])));
    let psot = (12 + 2 + data.len()) as u32;
    bytes.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    bytes.extend_from_slice(&psot.to_be_bytes());
    bytes.extend_from_slice(&[0, 1, 0xFF, 0x93]);
    bytes.extend(data);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

/// OpenJPEG 2.5.0's image of the twins: each included code-block's first
/// column reads 255 then 0, and every other sample is 128.
fn expected(included: [bool; 3]) -> Vec<i32> {
    let mut samples = vec![128; 12 * 4];
    for block in (0..3).filter(|&b| included[b]) {
        samples[4 * block] = 255;
        samples[12 + 4 * block] = 0;
    }
    samples
}

/// `bytes` as written when OpenJPEG 2.5.0 decoded them to [`expected`].
fn assert_pinned(bytes: &[u8], sha256: &str) {
    assert_eq!(sha256_hex(bytes), sha256, "the bytes OpenJPEG decoded");
}

fn samples(bytes: &[u8]) -> Vec<i32> {
    let image = decode(bytes).expect("decodes");
    assert_eq!((image.width, image.height), (12, 4));
    image.components[0].samples.clone()
}

/// A code-block first included at layer 999 next to one never included,
/// and its twin at layer 1.
#[test]
fn a_code_block_first_included_at_layer_999_decodes() {
    let twin = codestream(2, [Some(0), Some(1), None]);
    assert_pinned(
        &twin,
        "9763825b89a69b9c49a4a273b91e115c11634218bb2a31fa596af7c673fefc57",
    );
    assert_eq!(samples(&twin), expected([true, true, false]));
    let file = codestream(1000, [Some(0), Some(999), None]);
    assert_eq!(samples(&file), expected([true, true, false]));
}

/// Every code-block included, the last at layer 999, against its twin with
/// one new code-block a layer. OpenJPEG refuses this file too ("segment too
/// long (4) with max (1) for codeblock 2").
#[test]
fn every_code_block_included_the_last_at_layer_999_decodes() {
    let twin = codestream(3, [Some(0), Some(1), Some(2)]);
    assert_pinned(
        &twin,
        "cf1a7e288a08f1ec72d97e9bc5b856ffc75b8ef2c734c29e36af62a583b54ec5",
    );
    assert_eq!(samples(&twin), expected([true; 3]));
    let file = codestream(1000, [Some(0), Some(1), Some(999)]);
    assert_eq!(samples(&file), expected([true; 3]));
}

/// 999 layers, a code-block first included in the last one: OpenJPEG 2.5.0
/// reads these, since no threshold reaches its 999, and decodes both to
/// [`expected`]'s image.
#[test]
fn a_code_block_first_included_at_layer_998_decodes_as_in_openjpeg() {
    let file = codestream(999, [Some(0), Some(998), None]);
    assert_pinned(
        &file,
        "0ad0d419445213a249ea5ae248799d799aa6de5aa533efde0f4a4ab280f3a74c",
    );
    assert_eq!(samples(&file), expected([true, true, false]));
    let file = codestream(999, [Some(0), Some(1), Some(998)]);
    assert_pinned(
        &file,
        "053286107dd7d442c10d3918742292b0e2eb9e26068b8e2a093939fee4b1aff0",
    );
    assert_eq!(samples(&file), expected([true; 3]));
}

/// At the 65535 layers `COD` allows, a code-block first included in the last
/// layer still decodes.
#[test]
fn a_code_block_first_included_in_layer_65534_decodes() {
    let file = codestream(u16::MAX, [Some(0), Some(65534), None]);
    assert_eq!(samples(&file), expected([true, true, false]));
}
