//! Tier-2 packet-decoding tests.
//!
//! Two oracles, both dependency-free: hand-built packet headers (every field
//! crafted, so a misread is caught directly) and the seed codestream, where the
//! parse self-check — packets must tile the tile-part exactly — proves the whole
//! header walk against a real OpenJPEG-produced bitstream.

use std::collections::BTreeSet;

use super::*;

/// Every byte of a code-block, across all its codeword segments.
fn all_bytes(block: &CodeBlock<'_>) -> Vec<u8> {
    block.segments.iter().flat_map(|s| s.bytes()).collect()
}

/// The byte chunks of a block's single codeword segment (the default style).
fn only_segment<'a, 'b>(block: &'a CodeBlock<'b>) -> &'a CodedSegment<'b> {
    assert_eq!(block.segments.len(), 1, "the default style has one segment");
    &block.segments[0]
}

/// Parse `data` as a one-layer packet over `bands`, returning the subbands and
/// the next byte offset — the shape `parse_packet` had before quality layers.
fn parse_one_packet<'a>(data: &'a [u8], bands: &[BandGeom]) -> Result<(Vec<Subband<'a>>, usize)> {
    let (subbands, next) = parse_layers(data, bands, 1)?;
    Ok((subbands, next))
}

/// Parse `layers` consecutive packets over `bands`, carrying the precinct state
/// across them the way `decode_packets` does. No packet delimiters.
fn parse_layers<'a>(
    data: &'a [u8],
    bands: &[BandGeom],
    layers: u32,
) -> Result<(Vec<Subband<'a>>, usize)> {
    parse_layers_with(
        data,
        bands,
        layers,
        Delimiters {
            sop: false,
            eph: false,
        },
    )
}

/// As [`parse_layers`], with `COD`'s packet delimiters signalled.
fn parse_layers_with<'a>(
    data: &'a [u8],
    bands: &[BandGeom],
    layers: u32,
    delimiters: Delimiters,
) -> Result<(Vec<Subband<'a>>, usize)> {
    parse_layers_styled(data, bands, layers, delimiters, 0)
}

/// As [`parse_layers`], with a code-block style — `restart` splits a block's
/// passes across codeword segments.
fn parse_layers_styled<'a>(
    data: &'a [u8],
    bands: &[BandGeom],
    layers: u32,
    delimiters: Delimiters,
    style: u8,
) -> Result<(Vec<Subband<'a>>, usize)> {
    let mut states: Vec<BandState<'a>> = bands.iter().map(BandState::new).collect();
    let mut cursor = 0usize;
    for layer in 0..layers {
        cursor = parse_packet(
            data,
            cursor,
            layer,
            layer,
            delimiters,
            style,
            bands,
            0,
            &mut states,
        )?;
    }
    Ok((build_subbands(bands, states)?, cursor))
}
use crate::codestream::markers::{Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform};
use crate::codestream::{MainHeader, Tile, TileHeader};
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
    MainHeader::new(
        Siz {
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
        Cod {
            progression: Progression::Lrcp,
            layers: 1,
            decomposition_levels: levels,
            code_block_width: cblk_exp - 2,
            code_block_height: cblk_exp - 2,
            code_block_style: 0,
            use_sop: false,
            use_eph: false,
            multiple_component_transform: false,
            transform: Transform::Reversible53,
            precinct_sizes: Vec::new(),
        },
        Qcd {
            style: QuantStyle::None,
            guard_bits: 2,
            steps: vec![(8, 0)],
        },
    )
}

/// Tile 0, carrying `data`. A tile keeps only its own tile-part-header markers —
/// none, here — so the header `decode_packets` decodes under is passed alongside.
fn tile(data: &[u8]) -> Tile<'_> {
    Tile {
        index: 0,
        header: TileHeader::default(),
        data: std::borrow::Cow::Borrowed(data),
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
        precincts: one_precinct(1, 1),
    }
}

/// The single maximal precinct that owns a band's whole `cols × rows` block
/// grid — what every hand-built band below has, since these tests craft packet
/// *headers* and the precinct partition is exercised against the oracle instead.
fn one_precinct(cols: usize, rows: usize) -> Vec<PrecinctGeom> {
    vec![PrecinctGeom {
        cols,
        rows,
        blocks: (0..cols * rows).collect(),
    }]
}

/// [`resolution_geoms`] with a full precinct budget — the budget is a
/// DoS guard, exercised separately, and irrelevant to the geometry these tests
/// check.
fn geoms_of(header: &MainHeader, tile: u32, comp: usize) -> Result<Vec<ResolutionGeom>> {
    let mut budget = usize::MAX;
    resolution_geoms(header, tile, comp, &mut budget)
}

// ---- Geometry (ISO Eq. B-15, code-block grid B.7) ----

/// A small image with 2^6 code-blocks gives one block per subband, and the
/// subband dimensions follow the standard's half-resolution split.
#[test]
fn geometry_single_block_per_subband() {
    let geoms = geoms_of(&header(100, 100, 2, 6), 0, 0).unwrap();
    assert_eq!(geoms.len(), 3); // NL = 2 → resolutions 0,1,2

    // Resolution 0: the NLLL band at level 2, ceil(100/4) = 25 square.
    let ll = &geoms[0].bands[0];
    assert_eq!(ll.kind, BandKind::Ll);
    assert_eq!((ll.width, ll.height), (25, 25));
    assert_eq!((ll.block_cols, ll.block_rows), (1, 1));

    // Resolution 1 detail bands sit at level 2 as well: 25 square.
    assert_eq!(geoms[1].bands.len(), 3);
    assert_eq!(geoms[1].bands[0].kind, BandKind::Hl);
    for b in &geoms[1].bands {
        assert_eq!((b.width, b.height), (25, 25));
    }

    // Resolution 2 detail bands at level 1: ceil(99/2) = 50 square.
    for b in &geoms[2].bands {
        assert_eq!((b.width, b.height), (50, 50));
    }
}

/// A band wider than the code-block tiles into a grid; the trailing row/column
/// blocks are clipped to the band edge.
#[test]
fn geometry_multi_block_grid() {
    // 200×200, one level, 2^5 = 32 blocks. LL is ceil(200/2) = 100 square.
    let geoms = geoms_of(&header(200, 200, 1, 5), 0, 0).unwrap();
    let ll = &geoms[0].bands[0];
    assert_eq!((ll.width, ll.height), (100, 100));
    // ceil(100/32) = 4 blocks each way.
    assert_eq!((ll.block_cols, ll.block_rows), (4, 4));
    // First block is a full 32×32 cell at the origin.
    assert_eq!(ll.blocks[0], (0, 0, 32, 32));
    // The bottom-right block is clipped to 100 − 96 = 4 on each axis.
    let last = ll.blocks[ll.block_cols * ll.block_rows - 1];
    assert_eq!(last, (96, 96, 4, 4));
}

/// The maximum decomposition depth builds without overflowing the grid shifts
/// and yields one resolution per level plus the base.
#[test]
fn geometry_max_decomposition_levels() {
    let geoms = geoms_of(&header(2, 2, 32, 6), 0, 0).unwrap();
    assert_eq!(geoms.len(), 33);
    // The coarsest LL collapses to a single sample; nothing panics on the way.
    assert_eq!((geoms[0].bands[0].width, geoms[0].bands[0].height), (1, 1));
}

/// A tile-component larger than one maximal precinct (2^15 on either axis) now
/// splits into two — before #61 this was the case the decoder had to reject,
/// because the packet walk visited a single precinct per resolution and would
/// have desynchronized (Eq. B-16).
#[test]
fn geometry_splits_past_the_maximal_precinct_extent() {
    let wide = geoms_of(&header(32769, 16, 1, 6), 0, 0).unwrap();
    assert_eq!(
        (wide[1].precincts_wide, wide[1].precincts_high),
        (2, 1),
        "the finest resolution spans two maximal precincts across"
    );
    let tall = geoms_of(&header(16, 32769, 1, 6), 0, 0).unwrap();
    assert_eq!((tall[1].precincts_wide, tall[1].precincts_high), (1, 2));
}

/// Exactly 2^15 still fits in the single maximal precinct.
#[test]
fn geometry_accepts_full_precinct_extent() {
    let geoms = geoms_of(&header(32768, 16, 1, 6), 0, 0).unwrap();
    assert_eq!(geoms[1].bands[0].width, 16384);
}

/// A hostile geometry — a tall, thin tile-component under a 2^1 precinct — has
/// ~one precinct per two rows, tens of millions of them, each its own
/// allocation. The budget stops it *before* the precincts are built, not after:
/// with the guard enforced inside the geometry walk this returns in microseconds,
/// where the after-the-fact check it replaced spent seconds and gigabytes first.
#[test]
fn geometry_rejects_a_precinct_bomb_before_allocating() {
    use std::time::Instant;
    let mut header = header(1, 1 << 24, 1, 6);
    header.components[0].coding.precinct_sizes = vec![(1, 1), (1, 1)];

    let mut budget = super::MAX_PRECINCTS;
    let start = Instant::now();
    let result = resolution_geoms(&header, 0, 0, &mut budget);
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(crate::Error::Unsupported(_))),
        "a 1×2^24 tile-component at a 2^1 precinct must be rejected"
    );
    assert!(
        elapsed.as_millis() < 100,
        "the guard must fire before the precincts are built, not after ({elapsed:?})"
    );
}

/// The budget is the tile's, not a component's: two components each individually
/// under the cap but together over it are still rejected, because
/// `resolution_geoms` spends one running total across them.
#[test]
fn the_precinct_budget_is_shared_across_components() {
    // A 1024×1024 tile-component under a 2^1 precinct is ~512×512 = 2^18
    // precincts at the finest resolution, right at the cap. Give the header two
    // such components; the shared budget must not admit both.
    let mut header = header(1024, 1024, 1, 6);
    header.siz.components.push(header.siz.components[0]);
    header.components.push(header.components[0].clone());
    for comp in &mut header.components {
        comp.coding.precinct_sizes = vec![(1, 1), (1, 1)];
    }

    let mut budget = super::MAX_PRECINCTS;
    let first = resolution_geoms(&header, 0, 0, &mut budget);
    let second = resolution_geoms(&header, 0, 1, &mut budget);
    assert!(
        first.is_err() || matches!(second, Err(crate::Error::Unsupported(_))),
        "two components sharing the budget must not both allocate their precincts"
    );
}

/// A code-block size past the standard's 2^10 / xcb+ycb≤12 limit is rejected,
/// not silently clamped (which would also keep the grid shifts well-defined).
#[test]
fn geometry_rejects_oversized_code_block() {
    // code_block_width field 9 → exponent 11 (> 10).
    let err = geoms_of(&header(64, 64, 1, 13), 0, 0);
    assert!(matches!(err, Err(crate::Error::Marker(_))));
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
    let (subbands, next) = parse_one_packet(&data, &bands).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 2);
    assert_eq!(all_bytes(block), body);
    assert_eq!(next, header_len + body.len());
}

/// Zero bit-planes count against the raised `Kmax = Mb + SPrgn` (ISO H.2), so
/// under a maxshift a background-only block can signal far more of them than
/// any band has magnitude planes — up to 292 (`Mb` ≤ 37, `SPrgn` ≤ 255). A
/// count in that range must parse; only a run past it is a malformed header.
#[test]
fn packet_zero_bitplane_count_can_exceed_the_magnitude_planes() {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // included at layer 0
    for _ in 0..292 {
        w.bit(0); // zero-bitplane tag tree: 292 zeros...
    }
    w.bit(1); // ...then the terminating 1: value 292, the largest possible
    w.bit(0); // num_passes = 1
    w.bit(0); // Lblock stays 3
    w.bits(5, 3); // length: 3 bits, value 5
    let mut data = w.finish();
    let body = [0xDE, 0xAD, 0xBE, 0xEF, 0x42];
    data.extend_from_slice(&body);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, _next) = parse_one_packet(&data, &bands).unwrap();
    assert_eq!(subbands[0].blocks[0].zero_bit_planes, 292);
}

/// A run of zero bits past every value a conformant stream can signal is a
/// malformed header, rejected rather than walked to the end of the buffer.
#[test]
fn packet_zero_bitplane_run_past_the_limit_is_rejected() {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // included at layer 0
    for _ in 0..400 {
        w.bit(0); // a zero run that never resolves inside the limit
    }
    let data = w.finish();

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let err = parse_one_packet(&data, &bands).expect_err("should reject");
    assert!(matches!(err, Error::Codestream(_)), "{err:?}");
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
    let (subbands, _next) = parse_one_packet(&data, &bands).unwrap();
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 5);
    assert_eq!(block.zero_bit_planes, 0);
    assert_eq!(all_bytes(block).len(), 20);
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
    let (subbands, _next) = parse_one_packet(&data, &bands).unwrap();
    assert_eq!(all_bytes(&subbands[0].blocks[0]).len(), 9);
}

/// An empty packet (present bit 0) contributes nothing and is one byte long.
#[test]
fn packet_empty() {
    let mut w = PackedHeader::new();
    w.bit(0); // not present
    let data = w.finish();
    assert_eq!(data.len(), 1);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_one_packet(&data, &bands).unwrap();
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 0);
    assert!(block.segments.is_empty());
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
    let (subbands, next) = parse_one_packet(&data, &bands).unwrap();
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 0);
    assert!(block.segments.is_empty());
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
        precincts: one_precinct(2, 1),
    };
    let (subbands, _next) = parse_one_packet(&data, &[band]).unwrap();
    let blocks = &subbands[0].blocks;
    assert_eq!(blocks[0].num_passes, 0);
    assert!(all_bytes(&blocks[0]).is_empty());
    assert_eq!(blocks[1].num_passes, 1);
    assert_eq!(all_bytes(&blocks[1]), &body);
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
    let coded = decode_packets(&cs.header, &cs.tiles[0]).unwrap();

    // opj_dump reports numresolutions = 5 (NL = 4): one LL packet + four detail
    // levels.
    assert_eq!(coded.components[0].resolutions.len(), 5);
    assert_eq!(coded.components[0].resolutions[0].subbands.len(), 1);
    assert_eq!(
        coded.components[0].resolutions[0].subbands[0].kind,
        BandKind::Ll
    );
    for res in &coded.components[0].resolutions[1..] {
        assert_eq!(res.subbands.len(), 3);
    }

    let total_blocks: usize = coded.components[0]
        .resolutions
        .iter()
        .flat_map(|r| &r.subbands)
        .map(|s| s.blocks.len())
        .sum();
    assert_eq!(total_blocks, 13); // 1 + 4×3, one block per subband at this size

    // At least one block actually carries coded data.
    let included = coded.components[0]
        .resolutions
        .iter()
        .flat_map(|r| &r.subbands)
        .flat_map(|s| &s.blocks)
        .any(|b| b.num_passes > 0 && !all_bytes(b).is_empty());
    assert!(included, "expected some included code-block");
}

/// Garbage tile-part data must resolve to an `Ok`/`Err` result, never a panic —
/// every shift, slice, and loop is bounded. (Full fuzzing is issue #18.)
#[test]
fn malformed_tile_data_never_panics() {
    let cases: [Vec<u8>; 5] = [
        Vec::new(),
        vec![0xFF; 64],
        vec![0x00; 64],
        (0..=255u8).cycle().take(300).collect(),
        vec![0x80, 0x01, 0xFF, 0xFE, 0x00, 0x7F],
    ];
    let h = header(32, 32, 3, 6);
    for data in &cases {
        let t = tile(data);
        // The result is allowed to be either Ok or Err; only a panic would fail.
        let _ = decode_packets(&h, &t);
    }
}

/// A header pairing a large image with tiny legal 4×4 code-blocks would drive
/// millions of per-block states and tag trees before a single tile-part byte
/// is read; the block-count guard rejects it up front instead.
#[test]
fn block_count_guard_rejects_metadata_bomb() {
    // 4096×4096 with 4×4 blocks: ~1.4M blocks across the ladder, over the 2^19 cap.
    let h = header(4096, 4096, 1, 2);
    let t = tile(&[]);
    let err = decode_packets(&h, &t).expect_err("guard fires before any packet parse");
    assert!(matches!(err, crate::Error::Unsupported(_)), "{err}");
}

// ---- Per-component geometry (issue #57) ----

/// Each component's subbands derive from *its own* sub-sampling, so components
/// of one image can have different band sizes at the same resolution.
///
/// Mixed per-component sub-sampling has no end-to-end oracle yet: OpenJPEG's raw
/// encoder cannot produce one (see `scripts/gen-multicomponent-fixtures.py`) and
/// every conformance entry that carries it also needs a progression or layer
/// feature that is not decoded. So it is pinned here, at the geometry, where a
/// decoder that reused component 0's factors for every component would show up.
#[test]
fn resolution_geoms_honour_each_components_sub_sampling() {
    let mut h = header(64, 64, 1, 6);
    h.siz.components = vec![
        SizComponent {
            bit_depth: 8,
            signed: false,
            x_sampling: 1,
            y_sampling: 1,
        },
        SizComponent {
            bit_depth: 8,
            signed: false,
            x_sampling: 2,
            y_sampling: 1,
        },
        SizComponent {
            bit_depth: 8,
            signed: false,
            x_sampling: 1,
            y_sampling: 2,
        },
        SizComponent {
            bit_depth: 8,
            signed: false,
            x_sampling: 2,
            y_sampling: 2,
        },
    ];

    // Re-resolve the per-component parameters against the widened SIZ: they are
    // derived from the component count, not stored beside it.
    let h = MainHeader::new(h.siz, h.cod, h.qcd);

    // One decomposition level, so resolution 0 is the LL at half the
    // tile-component extent in each axis.
    let expected_ll = [(32, 32), (16, 32), (32, 16), (16, 16)];
    for (component, (want_width, want_height)) in expected_ll.into_iter().enumerate() {
        let geoms = geoms_of(&h, 0, component).unwrap();
        let ll = &geoms[0].bands[0];
        assert_eq!(ll.kind, BandKind::Ll);
        assert_eq!(
            (ll.width, ll.height),
            (want_width, want_height),
            "component {component} LL band",
        );
    }

    // A component index past the SIZ list is an internal inconsistency, not a
    // silent fall back to component 0.
    assert!(geoms_of(&h, 0, 4).is_err());
}

// ---- Quality layers (issue #64) ----

/// Assemble `(header, body)` pairs into a tile-part. Each packet's header is a
/// whole number of bytes and its body follows immediately, so the layers cannot
/// share one bit-writer.
fn packets(parts: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (header, body) in parts {
        data.extend_from_slice(header);
        data.extend_from_slice(body);
    }
    data
}

/// A block included in layer 0 signals its later contributions with a single
/// bit, not another inclusion tag-tree read, and its coding passes accumulate.
#[test]
fn layers_accumulate_passes_and_segments() {
    let mut l0 = PackedHeader::new();
    l0.bit(1); // present
    l0.bit(1); // inclusion tag tree (1x1): value 0
    l0.bits(0b001, 3); // zero-bitplane value 2
    l0.bit(0); // num_passes = 1
    l0.bit(0); // Lblock stays 3
    l0.bits(3, 3); // length 3

    let mut l1 = PackedHeader::new();
    l1.bit(1); // present
    l1.bit(1); // already included, so one bit: contributes
    l1.bit(0); // num_passes = 1
    l1.bit(0); // Lblock stays 3
    l1.bits(2, 3); // length 2

    let data = packets(&[(l0.finish(), vec![1, 2, 3]), (l1.finish(), vec![4, 5])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 2, "passes accumulate across layers");
    assert_eq!(block.zero_bit_planes, 2, "read once, on first inclusion");
    assert_eq!(only_segment(block).chunks.len(), 2, "one chunk per layer");
    // The MQ codeword is the concatenation, in layer order.
    assert_eq!(all_bytes(block), vec![1, 2, 3, 4, 5]);
    assert_eq!(next, data.len());
}

/// A block absent from layer 0 is resolved by the inclusion tag tree at a rising
/// threshold: the tree keeps its partial decode between packets. Rebuilding it
/// per packet would re-read the same bit and desynchronise the header.
#[test]
fn a_block_first_included_in_a_later_layer() {
    let mut l0 = PackedHeader::new();
    l0.bit(1); // present
    l0.bit(0); // inclusion: not yet (raises the tree's bound to 1)

    let mut l1 = PackedHeader::new();
    l1.bit(1); // present
    l1.bit(1); // inclusion resolves: value 1 <= layer 1
    l1.bit(1); // zero-bitplane value 0
    l1.bit(0); // num_passes = 1
    l1.bit(0); // Lblock stays 3
    l1.bits(2, 3); // length 2

    let data = packets(&[(l0.finish(), vec![]), (l1.finish(), vec![0xAA, 0xBB])]);
    let bands = [single_block_band(BandKind::Hl, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 0);
    assert_eq!(all_bytes(block), vec![0xAA, 0xBB]);
    assert_eq!(next, data.len());
}

/// An included block that contributes nothing to a layer costs one zero bit and
/// leaves its accumulated state alone.
#[test]
fn an_included_block_may_skip_a_layer() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bit(0); // 1 pass
    l0.bit(0); // Lblock 3
    l0.bits(2, 3); // length 2

    let mut l1 = PackedHeader::new();
    l1.bit(1); // present
    l1.bit(0); // included, but no contribution this layer

    let data = packets(&[(l0.finish(), vec![9, 9]), (l1.finish(), vec![])]);
    let bands = [single_block_band(BandKind::Lh, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 1);
    assert_eq!(all_bytes(block), vec![9, 9]);
    assert_eq!(next, data.len());
}

/// `Lblock` grows by a unary run and carries into later layers: the width of the
/// length field in layer 1 depends on what layer 0 signalled. A decoder that
/// reset it would read the wrong number of bits and desynchronise.
#[test]
fn lblock_persists_across_layers() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bit(0); // 1 pass
    l0.bit(1); // Lblock 3 -> 4
    l0.bit(0); // end of the unary run
    l0.bits(2, 4); // the length field is now 4 bits wide

    let mut l1 = PackedHeader::new();
    l1.bit(1);
    l1.bit(1); // contributes
    l1.bit(0); // 1 pass
    l1.bit(0); // no further Lblock growth
    l1.bits(3, 4); // still 4 bits, because Lblock persisted

    let data = packets(&[(l0.finish(), vec![1, 2]), (l1.finish(), vec![3, 4, 5])]);
    let bands = [single_block_band(BandKind::Hh, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 2);
    assert_eq!(all_bytes(block), vec![1, 2, 3, 4, 5]);
    assert_eq!(next, data.len());
}

/// The length field's width is `Lblock + floor(log2(passes))` for the passes of
/// *this* contribution, not the running total. Layer 1 adds 3 passes to a block
/// that already has 3: a decoder that used the accumulated 6 would read a 5-bit
/// length where 4 were written, and desynchronise.
#[test]
fn length_field_width_uses_this_layers_passes_not_the_total() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bits(0b1100, 4); // num_passes = 3
    l0.bit(0); // Lblock stays 3
    l0.bits(2, 4); // length 2, in 3 + floor(log2 3) = 4 bits

    let mut l1 = PackedHeader::new();
    l1.bit(1);
    l1.bit(1); // contributes
    l1.bits(0b1100, 4); // num_passes = 3 again (total becomes 6)
    l1.bit(0); // Lblock stays 3
    l1.bits(3, 4); // still 4 bits: floor(log2 3) = 1, not floor(log2 6) = 2

    let data = packets(&[(l0.finish(), vec![1, 2]), (l1.finish(), vec![3, 4, 5])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 6, "the total still accumulates");
    assert_eq!(all_bytes(block), vec![1, 2, 3, 4, 5]);
    assert_eq!(next, data.len());
}

/// An empty packet costs its layer a single bit and touches no state, so the
/// inclusion tree still resolves at value 0 in the next layer.
#[test]
fn an_empty_packet_leaves_the_tag_trees_alone() {
    let mut l0 = PackedHeader::new();
    l0.bit(0); // empty packet

    let mut l1 = PackedHeader::new();
    l1.bit(1);
    l1.bit(1); // inclusion resolves at value 0 — the tree was never advanced
    l1.bit(1); // zero-bitplane 0
    l1.bit(0); // 1 pass
    l1.bit(0); // Lblock 3
    l1.bits(1, 3); // length 1

    let data = packets(&[(l0.finish(), vec![]), (l1.finish(), vec![0x7F])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 1);
    assert_eq!(all_bytes(block), vec![0x7F]);
    assert_eq!(next, data.len());
}

/// A layer may add coding passes and no bytes. The MQ codeword is continuous
/// across layers, so the encoder's rate split can put a block's passes in one
/// layer and the bytes carrying them in the next. OpenJPEG reads such a length
/// without complaint, and so must this: rejecting it would false-reject a valid
/// codestream.
#[test]
fn a_layer_may_contribute_passes_without_bytes() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bit(0); // 1 pass
    l0.bit(0); // Lblock 3
    l0.bits(0, 3); // length 0 — no bytes this layer

    let mut l1 = PackedHeader::new();
    l1.bit(1);
    l1.bit(1); // contributes
    l1.bit(0); // 1 pass
    l1.bit(0); // Lblock 3
    l1.bits(2, 3); // length 2 — the bytes arrive now

    let data = packets(&[(l0.finish(), vec![]), (l1.finish(), vec![0xC0, 0xDE])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 2).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 2, "both layers' passes count");
    assert_eq!(all_bytes(block), vec![0xC0, 0xDE]);
    assert_eq!(next, data.len());
}

/// A block that takes coding passes but no bytes from *any* layer would hand
/// Tier-1 an empty MQ stream. That is the case worth rejecting.
#[test]
fn a_block_with_passes_and_no_bytes_at_all_is_codestream() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bit(0); // 1 pass
    l0.bit(0); // Lblock 3
    l0.bits(0, 3); // length 0

    let mut l1 = PackedHeader::new();
    l1.bit(1);
    l1.bit(1); // contributes
    l1.bit(0); // 1 pass
    l1.bit(0); // Lblock 3
    l1.bits(0, 3); // length 0 again — no bytes ever arrive

    let data = packets(&[(l0.finish(), vec![]), (l1.finish(), vec![])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    assert!(matches!(
        parse_layers(&data, &bands, 2),
        Err(Error::Codestream(_))
    ));
}

/// A tile-part that ends before its declared layers are read is truncation, not
/// a silently short decode.
#[test]
fn a_tile_part_shorter_than_its_layers_is_codestream() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bit(0); // 1 pass
    l0.bit(0); // Lblock 3
    l0.bits(1, 3); // length 1

    let data = packets(&[(l0.finish(), vec![0x01])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    assert!(matches!(
        parse_layers(&data, &bands, 2),
        Err(Error::Codestream(_))
    ));
}

// ---- Progression orders (issue #62) and the precinct partition (#61) ----

const ORDERS: [Progression; 5] = [
    Progression::Lrcp,
    Progression::Rlcp,
    Progression::Rpcl,
    Progression::Pcrl,
    Progression::Cprl,
];

/// The knobs the packet walk actually reads: how big the tile-component grids
/// are, how deep the pyramid is, and how the precincts cut it up. The positional
/// orders derive their sweep from all three, so a progression test that fixes an
/// axis count instead of a geometry cannot exercise them.
struct Layout {
    size: (u32, u32),
    /// One `(XRsiz, YRsiz)` per component.
    sampling: Vec<(u8, u8)>,
    levels: u8,
    /// `(PPx, PPy)` per resolution, coarsest first. Empty = maximal.
    precincts: Vec<(u8, u8)>,
}

impl Layout {
    /// One component, no sub-sampling, maximal precincts.
    fn plain(size: (u32, u32), levels: u8) -> Self {
        Layout {
            size,
            sampling: vec![(1, 1)],
            levels,
            precincts: Vec::new(),
        }
    }

    fn components(mut self, count: usize) -> Self {
        self.sampling = vec![(1, 1); count];
        self
    }

    /// The same precinct exponent at every resolution.
    fn precincts(mut self, ppx: u8, ppy: u8) -> Self {
        self.precincts = vec![(ppx, ppy); usize::from(self.levels) + 1];
        self
    }

    fn header(&self) -> MainHeader {
        MainHeader::new(
            Siz {
                x_size: self.size.0,
                y_size: self.size.1,
                x_offset: 0,
                y_offset: 0,
                tile_width: self.size.0,
                tile_height: self.size.1,
                tile_x_offset: 0,
                tile_y_offset: 0,
                components: self
                    .sampling
                    .iter()
                    .map(|&(x_sampling, y_sampling)| SizComponent {
                        bit_depth: 8,
                        signed: false,
                        x_sampling,
                        y_sampling,
                    })
                    .collect(),
            },
            Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: self.levels,
                code_block_width: 4, // 2^6
                code_block_height: 4,
                code_block_style: 0,
                use_sop: false,
                use_eph: false,
                multiple_component_transform: false,
                transform: Transform::Reversible53,
                precinct_sizes: self.precincts.clone(),
            },
            Qcd {
                style: QuantStyle::None,
                guard_bits: 2,
                steps: vec![(8, 0); 3 * usize::from(self.levels) + 1],
            },
        )
    }
}

/// The `(layer, resolution, component, precinct)` sequence one order visits over
/// a real tile geometry.
fn order_of(
    layout: &Layout,
    progression: Progression,
    layers: u32,
) -> Vec<(u32, usize, usize, usize)> {
    let header = layout.header();
    let geoms: Vec<Vec<ResolutionGeom>> = (0..header.siz.components.len())
        .map(|c| geoms_of(&header, 0, c).expect("geometry"))
        .collect();
    let walk = PacketWalk {
        siz: &header.siz,
        tile: header.siz.tile_rect(0).expect("tile 0"),
        geoms: &geoms,
    };
    let mut seen = Vec::new();
    for_each_packet(progression, layers, &walk, |l, r, c, p| {
        seen.push((l, r, c, p));
        Ok(())
    })
    .unwrap();
    seen
}

/// Each order is a distinct permutation of the layer / resolution / component
/// nesting (ISO B.12.1). With one precinct per resolution the position axis has a
/// single value and drops out, which is the shape every pre-#61 codestream had.
#[test]
fn each_progression_nests_its_axes_in_the_right_order() {
    // Two resolutions, two components, two layers, and a single precinct each.
    let layout = Layout::plain((32, 32), 1).components(2);
    let first_four = |p| {
        order_of(&layout, p, 2)[..4]
            .iter()
            .map(|&(l, r, c, _)| (l, r, c))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        first_four(Progression::Lrcp),
        [(0, 0, 0), (0, 0, 1), (0, 1, 0), (0, 1, 1)]
    );
    assert_eq!(
        first_four(Progression::Rlcp),
        [(0, 0, 0), (0, 0, 1), (1, 0, 0), (1, 0, 1)]
    );
    assert_eq!(
        first_four(Progression::Rpcl),
        [(0, 0, 0), (1, 0, 0), (0, 0, 1), (1, 0, 1)]
    );
    assert_eq!(
        first_four(Progression::Pcrl),
        [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0)]
    );
}

/// The five orders are permutations of one set, not five different selections of
/// packets. This is the invariant that pins the positional sweeps down: they
/// enumerate precincts by walking the reference grid rather than by counting, so
/// a wrong step, a wrong lattice test, or a wrong `precno` shows up here as a
/// packet visited twice or not at all — even though the *counting* orders next to
/// them cannot go wrong that way.
#[test]
fn every_order_visits_the_same_packets() {
    // Sub-sampled components and precincts smaller than the resolution, so the
    // components' precinct lattices genuinely differ and the sweep has to
    // interleave them.
    let layout = Layout {
        size: (64, 48),
        sampling: vec![(1, 1), (2, 1), (2, 2)],
        levels: 2,
        precincts: vec![(3, 3), (4, 4), (4, 5)],
    };
    let expected: BTreeSet<_> = order_of(&layout, Progression::Lrcp, 3)
        .into_iter()
        .collect();
    assert!(expected.len() > 60, "the layout must be worth walking");

    for progression in ORDERS {
        let seen = order_of(&layout, progression, 3);
        assert_eq!(
            seen.len(),
            expected.len(),
            "{progression:?} visits {} packets, LRCP visits {}",
            seen.len(),
            expected.len(),
        );
        assert_eq!(
            seen.iter().copied().collect::<BTreeSet<_>>(),
            expected,
            "{progression:?} visits a different set of packets than LRCP",
        );
    }
}

/// With one precinct per resolution, PCRL and CPRL genuinely enumerate the same
/// sequence — their position loops both degenerate. That was true of every
/// codestream this crate could decode before #61, and it is why OpenJPEG's own
/// output for the two differed in exactly one byte: the COD progression code.
#[test]
fn pcrl_and_cprl_coincide_with_one_precinct() {
    let layout = Layout::plain((32, 32), 2).components(3);
    assert_eq!(
        order_of(&layout, Progression::Pcrl, 3),
        order_of(&layout, Progression::Cprl, 3),
    );
}

/// …and part company as soon as there is more than one precinct to position.
/// PCRL sweeps the canvas once and interleaves the components at each point;
/// CPRL finishes a component's whole sweep before starting the next. This is the
/// distinction no fixture in the corpus could draw until now.
#[test]
fn pcrl_and_cprl_diverge_once_precincts_exist() {
    let layout = Layout::plain((64, 64), 2).components(3).precincts(4, 4);
    let pcrl = order_of(&layout, Progression::Pcrl, 1);
    let cprl = order_of(&layout, Progression::Cprl, 1);

    assert_eq!(pcrl.len(), cprl.len());
    assert_ne!(pcrl, cprl, "the two orders must not coincide");
    // CPRL's first packets all belong to component 0; PCRL's do not.
    assert!(cprl[..cprl.len() / 3].iter().all(|&(_, _, c, _)| c == 0));
    assert!(pcrl[..4].iter().any(|&(_, _, c, _)| c != 0));
}

/// The orders are distinct only when more than one of the axes is. A single
/// component, a single layer and a single precinct collapse all five onto `r`,
/// which is why the conformance entry `p0_01` cannot test any of them.
#[test]
fn the_orders_coincide_when_only_resolution_varies() {
    let layout = Layout::plain((32, 32), 3);
    let first = order_of(&layout, ORDERS[0], 1);
    for progression in ORDERS {
        assert_eq!(order_of(&layout, progression, 1), first, "{progression:?}");
    }
    // Add a second layer and LRCP parts company with RLCP.
    let layered = Layout::plain((32, 32), 1);
    assert_ne!(
        order_of(&layered, Progression::Lrcp, 2),
        order_of(&layered, Progression::Rlcp, 2),
    );
}

// ---- Packet delimiters: SOP / EPH (issue #73) ----

const SOP: Delimiters = Delimiters {
    sop: true,
    eph: false,
};
const EPH: Delimiters = Delimiters {
    sop: false,
    eph: true,
};

/// A six-byte SOP marker segment: `FF91 0004 Nsop`.
fn sop_marker(nsop: u16) -> Vec<u8> {
    let mut v = vec![0xFF, 0x91, 0x00, 0x04];
    v.extend_from_slice(&nsop.to_be_bytes());
    v
}

/// One included block: a header with `zero_bit_planes = 0`, one pass, length 2.
fn one_block_header() -> Vec<u8> {
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // included
    w.bit(1); // zero-bitplane 0
    w.bit(0); // 1 pass
    w.bit(0); // Lblock stays 3
    w.bits(2, 3); // length 2
    w.finish()
}

/// SOP precedes the packet header and is consumed; its `Nsop` must match the
/// packet's index within the tile.
#[test]
fn sop_is_consumed_and_its_sequence_number_checked() {
    let mut data = sop_marker(0);
    data.extend_from_slice(&one_block_header());
    data.extend_from_slice(&[0xAB, 0xCD]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers_with(&data, &bands, 1, SOP).unwrap();
    assert_eq!(all_bytes(&subbands[0].blocks[0]), vec![0xAB, 0xCD]);
    assert_eq!(next, data.len());
}

/// SOP is optional even when `Scod` signals it (A.8.1). OpenJPEG warns and
/// carries on; rejecting its absence would false-reject a valid codestream.
#[test]
fn a_missing_sop_is_tolerated() {
    let mut data = one_block_header();
    data.extend_from_slice(&[1, 2]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers_with(&data, &bands, 1, SOP).unwrap();
    assert_eq!(all_bytes(&subbands[0].blocks[0]), vec![1, 2]);
    assert_eq!(next, data.len());
}

/// A wrong `Nsop` means the packet boundary is not where the codestream says.
/// That is what the marker exists to detect, so it is an error even though
/// OpenJPEG leaves it unchecked (`/* TODO : check the Nsop value */`).
#[test]
fn a_wrong_sop_sequence_number_is_codestream() {
    let mut data = sop_marker(7); // packet 0 claims to be packet 7
    data.extend_from_slice(&one_block_header());
    data.extend_from_slice(&[1, 2]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let e = parse_layers_with(&data, &bands, 1, SOP).unwrap_err();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("sequence number")),
        "got {e:?}"
    );
}

#[test]
fn a_malformed_sop_segment_is_codestream() {
    let bands = [single_block_band(BandKind::Ll, 8, 8)];

    // Lsop must be 4.
    let mut data = vec![0xFF, 0x91, 0x00, 0x06, 0x00, 0x00];
    data.extend_from_slice(&one_block_header());
    data.extend_from_slice(&[1, 2]);
    let e = parse_layers_with(&data, &bands, 1, SOP).unwrap_err();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("Lsop")),
        "got {e:?}"
    );

    // Truncated before Nsop.
    let data = vec![0xFF, 0x91, 0x00, 0x04, 0x00];
    let e = parse_layers_with(&data, &bands, 1, SOP).unwrap_err();
    assert!(matches!(e, Error::Codestream(_)), "got {e:?}");
}

/// `Nsop` is 16 bits and wraps. Packet 65536 carries `Nsop = 0` again.
#[test]
fn the_sop_sequence_number_wraps_at_65536() {
    let mut data = sop_marker(0);
    data.extend_from_slice(&one_block_header());
    data.extend_from_slice(&[1, 2]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let mut states: Vec<BandState<'_>> = bands.iter().map(BandState::new).collect();
    // Packet index 65536 expects Nsop 0, not 65536 (which does not fit).
    parse_packet(&data, 0, 0, 65536, SOP, 0, &bands, 0, &mut states).expect("wraps to zero");
}

/// EPH sits between the packet header and its body, and `Scod` makes it
/// mandatory rather than optional (A.8.2).
#[test]
fn eph_is_consumed_after_the_packet_header() {
    let mut data = one_block_header();
    data.extend_from_slice(&[0xFF, 0x92]);
    data.extend_from_slice(&[3, 4]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers_with(&data, &bands, 1, EPH).unwrap();
    assert_eq!(all_bytes(&subbands[0].blocks[0]), vec![3, 4]);
    assert_eq!(next, data.len());
}

#[test]
fn a_missing_eph_is_codestream() {
    let mut data = one_block_header();
    data.extend_from_slice(&[1, 2]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let e = parse_layers_with(&data, &bands, 1, EPH).unwrap_err();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("EPH")),
        "got {e:?}"
    );
}

/// An empty packet still carries its EPH: the marker follows the header, and an
/// empty packet has one.
#[test]
fn an_empty_packet_still_carries_its_eph() {
    let mut w = PackedHeader::new();
    w.bit(0); // empty packet
    let mut data = w.finish();
    data.extend_from_slice(&[0xFF, 0x92]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers_with(&data, &bands, 1, EPH).unwrap();
    assert_eq!(subbands[0].blocks[0].num_passes, 0);
    assert_eq!(next, data.len());
}

// ---- Code-block style: restart / termall (issue #67) ----

/// Under `restart` each coding pass is a terminated codeword segment of its own,
/// so one contribution carries a length field per pass, not one per contribution.
#[test]
fn restart_splits_a_contribution_into_one_segment_per_pass() {
    const RESTART: u8 = 0x04;
    let mut w = PackedHeader::new();
    w.bit(1); // present
    w.bit(1); // included
    w.bit(1); // zero-bitplane 0
    w.bits(0b1100, 4); // num_passes = 3
    w.bit(0); // Lblock stays 3
    // Three length fields, one per pass. Each is `Lblock + floor(log2 1)` = 3
    // bits wide, because each segment takes exactly one pass.
    w.bits(1, 3);
    w.bits(2, 3);
    w.bits(3, 3);
    let mut data = w.finish();
    data.extend_from_slice(&[0xA1]); // pass 0
    data.extend_from_slice(&[0xB1, 0xB2]); // pass 1
    data.extend_from_slice(&[0xC1, 0xC2, 0xC3]); // pass 2

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers_styled(
        &data,
        &bands,
        1,
        Delimiters {
            sop: false,
            eph: false,
        },
        RESTART,
    )
    .unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 3);
    assert_eq!(block.segments.len(), 3, "one segment per coding pass");
    for segment in &block.segments {
        assert_eq!(segment.passes, 1);
    }
    assert_eq!(block.segments[0].bytes(), vec![0xA1]);
    assert_eq!(block.segments[1].bytes(), vec![0xB1, 0xB2]);
    assert_eq!(block.segments[2].bytes(), vec![0xC1, 0xC2, 0xC3]);
    assert_eq!(next, data.len());
}

/// Without `restart` the same three passes share one segment and one length
/// field — the codeword is continuous.
#[test]
fn the_default_style_keeps_one_segment_for_every_pass() {
    let mut w = PackedHeader::new();
    w.bit(1);
    w.bit(1); // included
    w.bit(1); // zero-bitplane 0
    w.bits(0b1100, 4); // num_passes = 3
    w.bit(0); // Lblock stays 3
    // One length field, `Lblock + floor(log2 3)` = 4 bits wide.
    w.bits(6, 4);
    let mut data = w.finish();
    data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);

    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers(&data, &bands, 1).unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 3);
    assert_eq!(block.segments.len(), 1, "one continuous codeword");
    assert_eq!(block.segments[0].passes, 3);
    assert_eq!(next, data.len());
}

/// A segment never takes more passes than it may. Under `restart` a layer that
/// contributes several passes opens several segments inside one packet.
#[test]
fn restart_segments_fill_one_pass_at_a_time_across_layers() {
    const RESTART: u8 = 0x04;
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bit(1); // zero-bitplane 0
    l0.bit(0); // 1 pass
    l0.bit(0); // Lblock stays 3
    l0.bits(1, 3); // one length

    let mut l1 = PackedHeader::new();
    l1.bit(1);
    l1.bit(1); // contributes
    l1.bit(0); // 1 pass — a *new* segment, since the first is full
    l1.bit(0); // Lblock stays 3
    l1.bits(2, 3);

    let data = packets(&[(l0.finish(), vec![7]), (l1.finish(), vec![8, 9])]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (subbands, next) = parse_layers_styled(
        &data,
        &bands,
        2,
        Delimiters {
            sop: false,
            eph: false,
        },
        RESTART,
    )
    .unwrap();

    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 2);
    assert_eq!(
        block.segments.len(),
        2,
        "a full segment does not take more passes"
    );
    assert_eq!(block.segments[0].bytes(), vec![7]);
    assert_eq!(block.segments[1].bytes(), vec![8, 9]);
    assert_eq!(next, data.len());
}
