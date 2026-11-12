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
//! writes for the twin); the twins are decoded here too. Found in the review
//! of PR #249 with a 2048 x 2048, 1000-layer file built the same way.

use rust_j2k::decode;

/// A packet-header bit writer (B.10.1): after a 0xFF byte, the next byte
/// holds only 7 bits.
struct Bits {
    out: Vec<u8>,
    byte: u32,
    room: u32,
}

impl Bits {
    fn new() -> Self {
        Bits {
            out: Vec::new(),
            byte: 0,
            room: 8,
        }
    }

    fn put(&mut self, bit: u32) {
        self.byte = (self.byte << 1) | bit;
        self.room -= 1;
        if self.room == 0 {
            self.out.push(self.byte as u8);
            self.room = if self.byte == 0xFF { 7 } else { 8 };
            self.byte = 0;
        }
    }

    /// The header's bytes, the last one padded with 0 bits, and a 0x00
    /// after a final 0xFF.
    fn finish(mut self) -> Vec<u8> {
        let full = if self.out.last() == Some(&0xFF) { 7 } else { 8 };
        if self.room != full {
            let room = self.room;
            for _ in 0..room {
                self.put(0);
            }
        }
        if self.out.last() == Some(&0xFF) {
            self.out.push(0);
        }
        self.out
    }
}

/// A tag tree over a row of leaves (B.10.2), encoded as OpenJPEG's
/// `opj_tgt_encode` does.
struct TagTree {
    /// `(parent, value, low, known)` per node, the leaves first.
    nodes: Vec<(Option<usize>, u32, u32, bool)>,
}

impl TagTree {
    fn new(leaves: &[u32]) -> Self {
        let mut nodes: Vec<(Option<usize>, u32, u32, bool)> =
            leaves.iter().map(|&v| (None, v, 0, false)).collect();
        let (mut start, mut width) = (0, leaves.len());
        while width > 1 {
            let parents = width.div_ceil(2);
            let first = nodes.len();
            for p in 0..parents {
                let value = (2 * p..(2 * p + 2).min(width))
                    .map(|c| nodes[start + c].1)
                    .min()
                    .unwrap();
                nodes.push((None, value, 0, false));
                for c in 2 * p..(2 * p + 2).min(width) {
                    nodes[start + c].0 = Some(first + p);
                }
            }
            (start, width) = (first, parents);
        }
        TagTree { nodes }
    }

    fn encode(&mut self, bits: &mut Bits, leaf: usize, threshold: u32) {
        let mut path = vec![leaf];
        while let Some(parent) = self.nodes[*path.last().unwrap()].0 {
            path.push(parent);
        }
        let mut low = 0;
        for &at in path.iter().rev() {
            let node = &mut self.nodes[at];
            low = low.max(node.2);
            while low < threshold {
                if low >= node.1 {
                    if !node.3 {
                        bits.put(1);
                        node.3 = true;
                    }
                    break;
                }
                bits.put(0);
                low += 1;
            }
            node.2 = low;
        }
    }
}

fn segment(marker: u16, body: &[u8]) -> Vec<u8> {
    let mut s = marker.to_be_bytes().to_vec();
    s.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
    s.extend_from_slice(body);
    s
}

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

    let mut siz = vec![0, 0];
    for v in [12u32, 4, 0, 0, 12, 4, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    siz.extend_from_slice(&[0, 1, 7, 1, 1]);
    let mut cod = vec![0, 0];
    cod.extend_from_slice(&layers.to_be_bytes());
    cod.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(segment(0xFF51, &siz));
    bytes.extend(segment(0xFF52, &cod));
    bytes.extend(segment(0xFF5C, &[0x40, 0x40]));
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
    assert_eq!(samples(&twin), expected([true; 3]));
    let file = codestream(1000, [Some(0), Some(1), Some(999)]);
    assert_eq!(samples(&file), expected([true; 3]));
}

/// At the 65535 layers `COD` allows, a code-block first included in the last
/// layer still decodes.
#[test]
fn a_code_block_first_included_in_layer_65534_decodes() {
    let file = codestream(u16::MAX, [Some(0), Some(65534), None]);
    assert_eq!(samples(&file), expected([true, true, false]));
}
