//! Tier-2 packet-decoding tests.
//!
//! Two oracles, both dependency-free: hand-built packet headers (every field
//! crafted, so a misread is caught directly) and the seed codestream, where the
//! parse self-check — packets must tile the tile-part exactly — proves the whole
//! header walk against a real OpenJPEG-produced bitstream.

use super::*;
use crate::codestream::MainHeader;
use crate::codestream::markers::{Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform};
use crate::tier2::bio::BitReader;

/// The encode side of [`BitReader`]: MSB-first packing with the Annex B.10.1
/// `0xFF` bit-stuffing and the matching flush, so a crafted header round-trips
/// through the decoder exactly (the inverse of `align`).
struct PackedHeader {
    out: Vec<u8>,
    buf: u32,
    ct: i32,
}

impl PackedHeader {
    fn new() -> Self {
        PackedHeader {
            out: Vec::new(),
            buf: 0,
            ct: 8,
        }
    }

    fn byteout(&mut self) {
        self.ct = if self.buf == 0xFF { 7 } else { 8 };
        self.out.push(self.buf as u8);
        self.buf = 0;
    }

    fn bit(&mut self, b: u32) {
        if self.ct == 0 {
            self.byteout();
        }
        self.ct -= 1;
        self.buf |= (b & 1) << self.ct;
    }

    fn bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((value >> i) & 1);
        }
    }

    /// Flush the partial byte and the trailing stuff byte (Annex B.10.1), the
    /// mirror of `BitReader::align`, and return the header bytes.
    fn finish(mut self) -> Vec<u8> {
        self.byteout();
        if self.ct == 7 {
            self.byteout();
        }
        self.out
    }
}

fn header(x_size: u32, y_size: u32, levels: u8, cblk_exp: u8) -> MainHeader {
    MainHeader {
        siz: Siz {
            x_size,
            y_size,
            x_offset: 0,
            y_offset: 0,
            tile_width: x_size,
            tile_height: y_size,
            tile_x_offset: 0,
            tile_y_offset: 0,
            components: vec![SizComponent {
                bit_depth: 8,
                signed: false,
                x_sampling: 1,
                y_sampling: 1,
            }],
        },
        cod: Cod {
            progression: Progression::Lrcp,
            layers: 1,
            decomposition_levels: levels,
            code_block_width: cblk_exp - 2,
            code_block_height: cblk_exp - 2,
            code_block_style: 0,
            transform: Transform::Reversible53,
            precinct_sizes: Vec::new(),
        },
        qcd: Qcd {
            style: QuantStyle::None,
            guard_bits: 2,
            steps: vec![(8, 0)],
        },
    }
}

fn single_block_band(kind: BandKind, width: usize, height: usize) -> BandGeom {
    BandGeom {
        kind,
        origin: (0, 0),
        width,
        height,
        block_cols: 1,
        block_rows: 1,
        blocks: vec![(0, 0, width, height)],
    }
}

// ---- Geometry (ISO Eq. B-15, code-block grid B.7) ----

/// A small image with 2^6 code-blocks gives one block per subband, and the
/// subband dimensions follow the standard's half-resolution split.
#[test]
fn geometry_single_block_per_subband() {
    let geoms = resolution_geoms(&header(100, 100, 2, 6)).unwrap();
    assert_eq!(geoms.len(), 3); // NL = 2 → resolutions 0,1,2

    // Resolution 0: the NLLL band at level 2, ceil(100/4) = 25 square.
    let ll = &geoms[0][0];
    assert_eq!(ll.kind, BandKind::Ll);
    assert_eq!((ll.width, ll.height), (25, 25));
    assert_eq!((ll.block_cols, ll.block_rows), (1, 1));

    // Resolution 1 detail bands sit at level 2 as well: 25 square.
    assert_eq!(geoms[1].len(), 3);
    assert_eq!(geoms[1][0].kind, BandKind::Hl);
    for b in &geoms[1] {
        assert_eq!((b.width, b.height), (25, 25));
    }

    // Resolution 2 detail bands at level 1: ceil(99/2) = 50 square.
    for b in &geoms[2] {
        assert_eq!((b.width, b.height), (50, 50));
    }
}

/// A band wider than the code-block tiles into a grid; the trailing row/column
/// blocks are clipped to the band edge.
#[test]
fn geometry_multi_block_grid() {
    // 200×200, one level, 2^5 = 32 blocks. LL is ceil(200/2) = 100 square.
    let geoms = resolution_geoms(&header(200, 200, 1, 5)).unwrap();
    let ll = &geoms[0][0];
    assert_eq!((ll.width, ll.height), (100, 100));
    // ceil(100/32) = 4 blocks each way.
    assert_eq!((ll.block_cols, ll.block_rows), (4, 4));
    // First block is a full 32×32 cell at the origin.
    assert_eq!(ll.blocks[0], (0, 0, 32, 32));
    // The bottom-right block is clipped to 100 − 96 = 4 on each axis.
    let last = ll.blocks[ll.block_cols * ll.block_rows - 1];
    assert_eq!(last, (96, 96, 4, 4));
}

// ---- Number-of-passes prefix code (ISO Table B.4) ----

#[test]
fn num_passes_table() {
    // (passes, the exact bits the encoder writes for that count).
    let cases: &[(u32, &[u32])] = &[
        (1, &[0]),
        (2, &[1, 0]),
        (3, &[1, 1, 0, 0]),
        (5, &[1, 1, 1, 0]),
        (6, &[1, 1, 1, 1, 0, 0, 0, 0, 0]),
        (36, &[1, 1, 1, 1, 1, 1, 1, 1, 0]),
        (37, &[1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0]),
        (164, &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]),
    ];
    for (passes, pattern) in cases {
        let mut w = PackedHeader::new();
        for &b in *pattern {
            w.bit(b);
        }
        let bytes = w.finish();
        let mut bio = BitReader::new(&bytes);
        assert_eq!(read_num_passes(&mut bio), *passes, "passes={passes}");
    }
}

// ---- Hand-built packets ----

/// An included single block: inclusion, zero-bitplane run, pass count, Lblock,
/// and length all decode, and the body slice is the bytes after the header.
#[test]
fn packet_one_included_block() {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // inclusion tag tree (1×1): value 0 → included at layer 0
    w.bits(0b001, 3); // zero-bitplane tag tree: value 2 (two 0s then a 1)
    w.bit(0); // num_passes = 1
    w.bit(0); // Lblock stays 3
    w.bits(5, 3); // length: Lblock(3) + floor(log2 1)=0 → 3 bits, value 5
    let mut data = w.finish();
    let header_len = data.len();
    let body = [0xDE, 0xAD, 0xBE, 0xEF, 0x42];
    data.extend_from_slice(&body);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_packet(&data, 0, &bands).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 2);
    assert_eq!(block.segment, &body);
    assert_eq!(next, header_len + body.len());
}

/// A larger pass count widens the length field by floor(log2 passes).
#[test]
fn packet_length_field_width_tracks_passes() {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // included
    w.bit(1); // zero-bitplane value 0
    // num_passes = 5 → bits 1110; floor(log2 5) = 2, so length is Lblock+2 = 5 bits.
    w.bits(0b1110, 4);
    w.bit(0); // Lblock stays 3
    w.bits(20, 5); // length = 20
    let mut data = w.finish();
    let body = vec![7u8; 20];
    data.extend_from_slice(&body);

    let bands = [single_block_band(BandKind::Hh, 16, 16)];
    let (subbands, _next) = parse_packet(&data, 0, &bands).unwrap();
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 5);
    assert_eq!(block.zero_bit_planes, 0);
    assert_eq!(block.segment.len(), 20);
}

/// The Lblock unary run widens the length field one bit per `1`.
#[test]
fn packet_lblock_increment() {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // included
    w.bit(1); // zero-bitplane value 0
    w.bit(0); // num_passes = 1
    w.bits(0b110, 3); // Lblock += 2 (two 1s then a 0) → Lblock = 5
    w.bits(9, 5); // length = 5 bits, value 9
    let mut data = w.finish();
    let body = vec![1u8; 9];
    data.extend_from_slice(&body);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, _next) = parse_packet(&data, 0, &bands).unwrap();
    assert_eq!(subbands[0].blocks[0].segment.len(), 9);
}

/// An empty packet (present bit 0) contributes nothing and is one byte long.
#[test]
fn packet_empty() {
    let mut w = PackedHeader::new();
    w.bit(0); // not present
    let data = w.finish();
    assert_eq!(data.len(), 1);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_packet(&data, 0, &bands).unwrap();
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 0);
    assert!(block.segment.is_empty());
    assert_eq!(next, 1);
}

/// A present packet whose single block is not included in the one layer: the
/// inclusion tag tree never resolves below the layer-0 threshold.
#[test]
fn packet_block_not_included() {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(0); // inclusion value ≥ 1 → not included at layer 0
    let data = w.finish();

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_packet(&data, 0, &bands).unwrap();
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 0);
    assert!(block.segment.is_empty());
    assert_eq!(next, data.len());
}

/// Two blocks in one band, only the second included: the body byte belongs to
/// the second block, the first stays empty. Exercises the shared-root tag tree
/// and per-block body assignment.
#[test]
fn packet_two_blocks_partial_inclusion() {
    // 2×1 inclusion tag tree, values L0=1 (not included at layer 0), L1=0.
    // Root min 0. Reading block 0 at threshold 1: root `1` (min 0), node0 `0`
    // (raise to 1) → not below 1 → not included. Reading block 1: node1 `1`
    // (value 0) → included.
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // inclusion root resolves to min 0
    w.bit(0); // block 0 node: value ≥ 1 → not included
    w.bit(1); // block 1 node: value 0 → included
    // Block 1's zero-bitplane tag tree is also 2×1, so reading its leaf first
    // resolves the (fresh) root: root `1` then leaf `1` for value 0.
    w.bit(1); // zero-bitplane root → min 0
    w.bit(1); // zero-bitplane leaf → value 0
    w.bit(0); // block 1 num_passes = 1
    w.bit(0); // Lblock stays 3
    w.bits(2, 3); // block 1 length = 2
    let mut data = w.finish();
    let body = [0xAA, 0xBB];
    data.extend_from_slice(&body);

    let band = BandGeom {
        kind: BandKind::Ll,
        origin: (0, 0),
        width: 16,
        height: 8,
        block_cols: 2,
        block_rows: 1,
        blocks: vec![(0, 0, 8, 8), (8, 0, 8, 8)],
    };
    let (subbands, _next) = parse_packet(&data, 0, &[band]).unwrap();
    let blocks = &subbands[0].blocks;
    assert_eq!(blocks[0].num_passes, 0);
    assert!(blocks[0].segment.is_empty());
    assert_eq!(blocks[1].num_passes, 1);
    assert_eq!(blocks[1].segment, &body);
}

// ---- Seed codestream (the real-bitstream oracle) ----

/// The seed fixture parses end to end: the right packet count, every subband
/// present, and — implicitly, via `decode_packets`' self-check — the packets
/// tiling the tile-part data exactly. A field misread would leave trailing
/// bytes and error out here.
#[test]
fn seed_codestream_parses() {
    let bytes = include_bytes!("../../tests/fixtures/jpeg2000_regular_latlon.j2k");
    let cs = crate::codestream::parse(bytes).unwrap();
    let coded = decode_packets(&cs).unwrap();

    // opj_dump reports numresolutions = 5 (NL = 4): one LL packet + four detail
    // levels.
    assert_eq!(coded.resolutions.len(), 5);
    assert_eq!(coded.resolutions[0].subbands.len(), 1);
    assert_eq!(coded.resolutions[0].subbands[0].kind, BandKind::Ll);
    for res in &coded.resolutions[1..] {
        assert_eq!(res.subbands.len(), 3);
    }

    let total_blocks: usize = coded
        .resolutions
        .iter()
        .flat_map(|r| &r.subbands)
        .map(|s| s.blocks.len())
        .sum();
    assert_eq!(total_blocks, 13); // 1 + 4×3, one block per subband at this size

    // At least one block actually carries coded data.
    let included = coded
        .resolutions
        .iter()
        .flat_map(|r| &r.subbands)
        .flat_map(|s| &s.blocks)
        .any(|b| b.num_passes > 0 && !b.segment.is_empty());
    assert!(included, "expected some included code-block");
}
