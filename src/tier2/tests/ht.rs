//! Tier-2 for HT code-blocks (ISO/IEC 15444-15 Annex B): placeholder passes,
//! the cleanup and refinement segments, their length fields and limits, and
//! the layer cap.
//!
//! Two oracles. Hand-built packet headers, one field at a time, and packet
//! headers of `ds0_ht_02_b11` and `ds0_ht_05_b11` decoded by hand below. No
//! reference decoder grades this step: OpenJPEG 2.5.0 loses sync on
//! placeholder passes, and OpenJPH reads one quality layer only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

const HT: u8 = 0x40;

const NO_DELIMITERS: Delimiters = Delimiters {
    sop: false,
    eph: false,
};

/// Parse `layers` packets of one HT precinct over `bands`, keeping the first
/// `kept` layers as a layer cap does (see [`Budget::layers`]).
fn parse_ht<'a>(
    data: &'a [u8],
    bands: &[BandGeom],
    layers: u32,
    kept: u32,
    delimiters: Delimiters,
) -> Result<(Vec<Subband<'a>>, usize)> {
    let mut states: Vec<BandState<'a>> = bands.iter().map(BandState::new).collect();
    let mut streams = PacketStreams {
        header: data,
        header_pos: 0,
        data,
        body_pos: 0,
        packed: false,
        tile: 0,
    };
    let mut scratch = Scratch::default();
    for layer in 0..layers {
        parse_packet(
            &mut streams,
            &Meter::new(u64::MAX),
            layer,
            layer,
            delimiters,
            HT,
            bands,
            0,
            &mut states,
            &mut scratch,
            layer < kept,
            None,
        )?;
    }
    Ok((build_subbands(bands, states), streams.body_pos))
}

/// The one code-block of an 8 x 8 band, after `layers` packets of `data`, the
/// first `kept` of them kept.
fn one_block(data: &[u8], layers: u32, kept: u32) -> Result<CodeBlock<'_>> {
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let (mut subbands, next) = parse_ht(data, &bands, layers, kept, NO_DELIMITERS)?;
    assert_eq!(next, data.len(), "the packets tile the data");
    Ok(subbands.remove(0).blocks.remove(0))
}

/// Write a pass count (Table B.4), the inverse of [`read_num_passes`].
fn num_passes(w: &mut PackedHeader, n: u32) {
    match n {
        1 => w.bit(0),
        2 => w.bits(0b10, 2),
        3..=5 => w.bits(0b1100 | (n - 3), 4),
        6..=36 => w.bits(0x1E0 | (n - 6), 9),
        _ => w.bits(0xFF80 | (n - 37), 16),
    }
}

/// A packet header whose one block is first included here with zero
/// bit-planes `2`, and contributes `passes` passes with Lblock 3 and the
/// length fields `lengths`, each `(value, bits)`.
fn first(passes: u32, lengths: &[(u32, u32)]) -> Vec<u8> {
    let mut w = PackedHeader::new();
    w.bit(1); // nonempty
    w.bit(1); // inclusion tree: included in this layer
    w.bits(0b001, 3); // zero bit-planes 2
    num_passes(&mut w, passes);
    w.bit(0); // Lblock stays 3
    for &(value, bits) in lengths {
        w.bits(value, bits);
    }
    w.finish()
}

/// A later packet header of the same block: it contributes `passes` passes
/// with the length fields `lengths`.
fn later(passes: u32, lengths: &[(u32, u32)]) -> Vec<u8> {
    let mut w = PackedHeader::new();
    w.bit(1); // nonempty
    w.bit(1); // included: contributes
    num_passes(&mut w, passes);
    w.bit(0); // Lblock stays 3
    for &(value, bits) in lengths {
        w.bits(value, bits);
    }
    w.finish()
}

/// Each segment's pass count and bytes.
fn segments(block: &CodeBlock<'_>) -> Vec<(u32, Vec<u8>)> {
    block
        .segments
        .iter()
        .map(|s| (s.passes, s.bytes()))
        .collect()
}

// ---- One contribution: 0, 1 and 2 placeholder triples ----

/// No placeholder passes: a cleanup, a SigProp and a MagRef pass. The cleanup
/// segment ends at pass 0 (B.2), so there are two length fields, of
/// `3 + floor(log2 1)` and `3 + floor(log2 2)` bits. The Part 1 rule would
/// read one field of 4 bits.
#[test]
fn a_cleanup_and_two_refinement_passes_are_two_segments() {
    let data = packets(&[(first(3, &[(2, 3), (3, 4)]), vec![1, 2, 3, 4, 5])]);
    let block = one_block(&data, 1, 1).unwrap();
    assert_eq!(block.num_passes, 3);
    assert_eq!(block.zero_bit_planes, 2);
    assert_eq!(block.placeholders, 0);
    assert_eq!(segments(&block), [(1, vec![1, 2]), (2, vec![3, 4, 5])]);
    assert!(!block.cut);
}

/// Three placeholder passes and the cleanup pass: one segment of 4 passes, a
/// field of `3 + floor(log2 4)` bits, and one more skipped bit-plane (B.3).
#[test]
fn one_placeholder_triple_skips_one_bit_plane() {
    let data = packets(&[(first(4, &[(2, 5)]), vec![1, 2])]);
    let block = one_block(&data, 1, 1).unwrap();
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 3);
    assert_eq!(block.placeholders, 3);
    assert_eq!(segments(&block), [(1, vec![1, 2])]);
}

/// Six placeholder passes, the cleanup pass and a SigProp pass: a field of
/// `3 + floor(log2 7)` bits for the cleanup segment and one of 3 bits for the
/// refinement segment, and two more skipped bit-planes.
#[test]
fn two_placeholder_triples_and_a_sigprop_pass() {
    let data = packets(&[(first(8, &[(2, 5), (1, 3)]), vec![1, 2, 3])]);
    let block = one_block(&data, 1, 1).unwrap();
    assert_eq!(block.num_passes, 2);
    assert_eq!(block.zero_bit_planes, 4);
    assert_eq!(block.placeholders, 6);
    assert_eq!(segments(&block), [(1, vec![1, 2]), (1, vec![3])]);
}

// ---- Later layers ----

/// The refinement passes may come in later layers, one each: the second
/// joins the first's segment, which holds two passes (B.2), and a pass after
/// them opens a cleanup segment of one pass.
#[test]
fn refinement_passes_in_later_layers_fill_one_segment() {
    let data = packets(&[
        (first(1, &[(2, 3)]), vec![1, 2]),
        (later(1, &[(1, 3)]), vec![3]),
        (later(1, &[(2, 3)]), vec![4, 5]),
        (later(1, &[(2, 3)]), vec![6, 7]),
    ]);
    let block = one_block(&data, 4, 4).unwrap();
    assert_eq!(block.num_passes, 4);
    assert_eq!(
        segments(&block),
        [(1, vec![1, 2]), (2, vec![3, 4, 5]), (1, vec![6, 7])]
    );
    assert_eq!(block.segments[1].chunks.len(), 2, "one chunk per layer");

    // Both refinement passes in one later layer: one field of 4 bits.
    let data = packets(&[
        (first(1, &[(2, 3)]), vec![1, 2]),
        (later(2, &[(3, 4)]), vec![3, 4, 5]),
    ]);
    let block = one_block(&data, 2, 2).unwrap();
    assert_eq!(segments(&block), [(1, vec![1, 2]), (2, vec![3, 4, 5])]);
}

/// A first contribution of placeholder passes only (B.3 NOTE 4). Three passes
/// could be a cleanup and two refinement passes, whose cleanup field is 3
/// bits, but those bits are zero and a first cleanup segment is at least 2
/// bytes, so they are the start of one 4-bit field for all three passes. The
/// cleanup pass comes in the next layer, after the three: one bit-plane
/// skipped.
#[test]
fn a_first_contribution_of_placeholders_only() {
    let mut l0 = PackedHeader::new();
    l0.bit(1);
    l0.bit(1); // included
    l0.bits(0b001, 3); // zero bit-planes 2
    num_passes(&mut l0, 3);
    l0.bit(0); // Lblock 3
    l0.bits(0, 3 + 1); // floor(log2 3) = 1: one 4-bit field, zero
    l0.bits(0b11, 2); // padding, which a misread takes for a refinement field
    let l0 = l0.finish();
    let data = packets(&[(l0, vec![]), (later(1, &[(2, 3)]), vec![1, 2])]);

    let block = one_block(&data, 2, 2).unwrap();
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 3);
    assert_eq!(block.placeholders, 3);
    assert_eq!(segments(&block), [(1, vec![1, 2])]);
}

/// A contribution that cannot hold the cleanup pass: after four placeholder
/// passes, a fifth would put it at pass 3, before them. Its field is read at
/// the placeholder width at once, and the next layer's two passes end at
/// pass 6, the cleanup pass: two bit-planes skipped.
#[test]
fn placeholders_past_the_cleanup_pass_a_count_gives() {
    let data = packets(&[
        (first(4, &[(0, 5)]), vec![]),
        (later(1, &[(0, 3)]), vec![]),
        (later(2, &[(2, 4)]), vec![1, 2]),
    ]);
    let block = one_block(&data, 3, 3).unwrap();
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 4);
    assert_eq!(block.placeholders, 6);
    assert_eq!(segments(&block), [(1, vec![1, 2])]);
}

/// A block whose layers hold placeholder passes only decodes to zeros: no
/// pass, no segment, and no skipped bit-plane.
#[test]
fn a_block_of_placeholders_only_has_no_pass() {
    let data = packets(&[(first(3, &[(0, 4)]), vec![])]);
    let block = one_block(&data, 1, 1).unwrap();
    assert_eq!(block.num_passes, 0);
    assert_eq!(block.zero_bit_planes, 2);
    assert_eq!(block.placeholders, 3);
    assert!(block.segments.is_empty());
}

// ---- Faults ----

/// A first cleanup segment of 1 byte is refused: B.3 requires at least 2,
/// which is what tells a cleanup field from a placeholder one.
#[test]
fn a_one_byte_first_cleanup_segment_is_codestream() {
    let data = packets(&[(first(1, &[(1, 3)]), vec![1])]);
    let e = one_block(&data, 1, 1).unwrap_err();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("first cleanup segment is 1 byte")),
        "{e:?}"
    );
}

/// Placeholder passes carry no bytes (B.1): a field that starts with the
/// cleanup reading's zero bits and then has a set bit is refused.
#[test]
fn placeholder_passes_with_bytes_are_codestream() {
    let data = packets(&[(first(3, &[(0, 3), (1, 1)]), vec![1])]);
    let e = one_block(&data, 1, 1).unwrap_err();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("placeholder passes carry bytes")),
        "{e:?}"
    );
}

/// A header whose one block has Lblock `3 + grow` and contributes `passes`
/// passes with `lengths`.
fn grown(grow: u32, passes: u32, lengths: &[(u32, u32)]) -> Vec<u8> {
    let mut w = PackedHeader::new();
    w.bit(1);
    w.bit(1);
    w.bits(0b001, 3);
    num_passes(&mut w, passes);
    for _ in 0..grow {
        w.bit(1);
    }
    w.bit(0);
    for &(value, bits) in lengths {
        w.bits(value, bits);
    }
    w.finish()
}

/// A cleanup segment is under 65535 bytes (7.1.1). The limit is checked on
/// the field, before the body is read.
#[test]
fn an_ht_cleanup_segment_past_its_limit_is_codestream() {
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let at = |length| parse_ht(&grown(13, 1, &[(length, 16)]), &bands, 1, 1, NO_DELIMITERS).err();
    let e = at(65535).unwrap();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("HT cleanup segment is 65535 bytes")),
        "{e:?}"
    );
    // At the limit the field reads, and the body is what runs short.
    assert!(
        matches!(at(65534), Some(Error::Codestream(m)) if m.contains("overruns")),
        "the limit is 65534"
    );
}

/// A refinement segment is under 2047 bytes (7.1.2), summed over the layers
/// that carry its passes.
#[test]
fn an_ht_refinement_segment_past_its_limit_is_codestream() {
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    // Lblock 11: a cleanup of 2 bytes and a SigProp pass of 2000 bytes, then
    // a MagRef pass of `magref` bytes.
    let at = |magref: u32| {
        let mut l1 = PackedHeader::new();
        l1.bit(1);
        l1.bit(1);
        num_passes(&mut l1, 1);
        l1.bit(0);
        l1.bits(magref, 11);
        let data = packets(&[
            (grown(8, 2, &[(2, 11), (2000, 11)]), vec![0; 2002]),
            (l1.finish(), vec![0; magref as usize]),
        ]);
        parse_ht(&data, &bands, 2, 2, NO_DELIMITERS).map(|_| ())
    };
    assert!(at(46).is_ok());
    let e = at(47).unwrap_err();
    assert!(
        matches!(&e, Error::Codestream(m) if m.contains("HT refinement segment is 2047 bytes")),
        "{e:?}"
    );
}

// ---- The layer cap ----

/// A cap that keeps a first contribution of placeholder passes only, and
/// drops the layer with the cleanup pass, keeps no pass: the cleanup pass is
/// present only when the kept passes reach it, whatever the pass count says.
#[test]
fn a_cap_that_keeps_only_placeholders_keeps_no_cleanup_pass() {
    let data = packets(&[
        (first(3, &[(0, 4)]), vec![]),
        (later(1, &[(2, 3)]), vec![1, 2]),
    ]);
    let block = one_block(&data, 2, 1).unwrap();
    assert_eq!(block.num_passes, 0);
    assert!(block.segments.is_empty());
    assert!(!block.cut, "no segment, so none cut");
    assert_eq!(block.placeholders, 3);
    assert_eq!(
        block.zero_bit_planes, 2,
        "no cleanup pass, no skipped plane"
    );

    let block = one_block(&data, 2, 2).unwrap();
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.zero_bit_planes, 3);
}

/// A cap that keeps the SigProp pass and drops the MagRef pass after it hands
/// Tier-1 both segments, the refinement one cut after its one kept pass.
#[test]
fn a_cap_that_keeps_sigprop_and_drops_magref() {
    let data = packets(&[
        (first(2, &[(2, 3), (3, 3)]), vec![1, 2, 3, 4, 5]),
        (later(1, &[(2, 3)]), vec![6, 7]),
    ]);
    let block = one_block(&data, 2, 1).unwrap();
    assert_eq!(block.num_passes, 2);
    assert_eq!(segments(&block), [(1, vec![1, 2]), (1, vec![3, 4, 5])]);
    assert!(block.cut, "the refinement segment is cut");

    let block = one_block(&data, 2, 2).unwrap();
    assert_eq!(block.num_passes, 3);
    assert_eq!(
        segments(&block),
        [(1, vec![1, 2]), (2, vec![3, 4, 5, 6, 7])]
    );
    assert!(!block.cut);
}

// ---- A cut packet, with `strict` off ----

/// A packet whose data runs out puts its blocks back as they were, every
/// field of their segments too, so that a read that resumes there follows the
/// HT segment rules from the right place. Layer 0 gives three placeholder
/// passes; layer 1, cut one byte short, would give the cleanup pass (which
/// ends the first segment and sets its pass limit and length) and a
/// refinement segment.
#[test]
fn a_cut_packet_puts_back_the_ht_segment_state() {
    let data = packets(&[
        (first(3, &[(0, 4)]), vec![]),
        (later(3, &[(2, 3), (3, 4)]), vec![1, 2, 3, 4, 5]),
    ]);
    let bands = [single_block_band(BandKind::Ll, 8, 8)];
    let mut states: Vec<BandState<'_>> = bands.iter().map(BandState::new).collect();
    let cut = &data[..data.len() - 1];
    let mut streams = PacketStreams {
        header: cut,
        header_pos: 0,
        data: cut,
        body_pos: 0,
        packed: false,
        tile: 0,
    };
    let mut scratch = Scratch::default();
    let mut log = Vec::new();
    let snapshot = |states: &[BandState<'_>]| {
        let block = &states[0].blocks[0];
        let segments: Vec<_> = block
            .segments
            .iter()
            .map(|s| {
                (
                    s.kind,
                    s.passes,
                    s.kept,
                    s.max_passes,
                    s.length,
                    s.chunks.len(),
                )
            })
            .collect();
        (block.num_passes, block.lblock, segments)
    };
    let mut before = None;
    for (layer, expected) in [(0, Packet::Read), (1, Packet::Ended)] {
        if layer == 1 {
            before = Some(snapshot(&states));
        }
        let packet = parse_packet(
            &mut streams,
            &Meter::new(u64::MAX),
            layer,
            layer,
            NO_DELIMITERS,
            HT,
            &bands,
            0,
            &mut states,
            &mut scratch,
            true,
            Some(&mut log),
        )
        .unwrap();
        assert_eq!(packet, expected, "layer {layer}");
    }
    let before = before.unwrap();
    assert_eq!(
        before.2,
        [(
            SegmentKind::HtCleanup { placeholders: None },
            3,
            3,
            u32::MAX,
            0,
            0
        )]
    );
    assert_eq!(snapshot(&states), before);
}

// ---- Packet headers of the conformance codestreams, decoded by hand ----

fn ht_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/conformance/ht")
}

fn read_codestream(name: &str) -> Vec<u8> {
    std::fs::read(ht_dir().join("codestreams").join(name)).unwrap()
}

/// The data after the `SOD` of a one-tile, one-tile-part codestream.
fn tile_data(bytes: &[u8]) -> &[u8] {
    let sod = bytes
        .windows(2)
        .position(|w| w == [0xFF, 0x93])
        .expect("SOD");
    &bytes[sod + 2..bytes.len() - 2]
}

/// `ds0_ht_02_b11` (127 x 126, one component, LRCP, 6 layers, `SOP` and
/// `EPH`; `COC` gives 32 x 32 code-blocks). Resolution 0 is one 16 x 16 `LL`
/// band of one block, and its packets are numbers 0, 4, 8, 12, 16 and 20.
/// Their headers, read by hand (`Lb` is Lblock):
///
/// ```text
/// L0  c3 00     1 nonempty, 1 included, 00001 zero bit-planes 4, 10 two
///               passes, 0 Lb 3. P0 = floor(1/3) = 0, so a cleanup field
///               would be 3 bits: 000. Zero, so placeholders: the one field
///               is 3 + floor(log2 2) = 4 bits, 1 more: 0. Pad 00.
/// L1  e0 00     1, 1 contributes, 10 two passes, 0. Four passes so far:
///               P0 = 1, cleanup at pass 3, 2 passes in: 4 bits 0000.
///               Placeholders, field complete. Pad.
/// L2  f4 00     1, 1, 1101 four passes, 0. Eight: P0 = 2, cleanup at pass
///               6, 3 passes in: 4 bits 0000, then 1 more of the 5: 0.
/// L3  fc 5a 40  1, 1, 11 11 00010 eight passes, 110 Lb 5. Sixteen: P0 = 5,
///               cleanup at pass 15, 8 passes in: 5 + 3 = 8 bits
///               10010000 = 144. Nonzero: the cleanup pass. Pad 000000.
/// L4  80        1, 0 no contribution.
/// L5  80        the same.
/// ```
///
/// So the block skips 4 + 5 = 9 bit-planes, and its one cleanup pass is the
/// 144 bytes after packet 12's header.
#[test]
fn ds0_ht_02_b11_resolution_0_by_hand() {
    let bytes = read_codestream("ds0_ht_02_b11.j2k");
    // The packets, each `SOP` header `EPH` body, and the headers and bodies
    // of resolution 0.
    let mut packets = Vec::new();
    let mut rest = tile_data(&bytes);
    while !rest.is_empty() {
        assert_eq!(rest[..2], [0xFF, 0x91], "SOP");
        let eph = rest.windows(2).position(|w| w == [0xFF, 0x92]).unwrap();
        let next = rest[eph..]
            .windows(2)
            .position(|w| w == [0xFF, 0x91])
            .map_or(rest.len(), |p| eph + p);
        packets.push((&rest[6..eph], &rest[eph + 2..next]));
        rest = &rest[next..];
    }
    assert_eq!(packets.len(), 24, "6 layers of 4 resolutions");
    let res0: Vec<_> = packets.iter().step_by(4).collect();
    let headers: Vec<&[u8]> = res0.iter().map(|p| p.0).collect();
    let expected: [&[u8]; 6] = [
        &[0xC3, 0x00],
        &[0xE0, 0x00],
        &[0xF4, 0x00],
        &[0xFC, 0x5A, 0x40],
        &[0x80],
        &[0x80],
    ];
    assert_eq!(headers, expected);
    let bodies: Vec<usize> = res0.iter().map(|p| p.1.len()).collect();
    assert_eq!(bodies, [0, 0, 0, 144, 0, 0]);

    // Resolution 0's packets alone, with `EPH` and without `SOP`.
    let mut data = Vec::new();
    for (header, body) in &res0 {
        data.extend_from_slice(header);
        data.extend_from_slice(&[0xFF, 0x92]);
        data.extend_from_slice(body);
    }
    let eph = Delimiters {
        sop: false,
        eph: true,
    };
    let bands = [single_block_band(BandKind::Ll, 16, 16)];
    let at = |kept| {
        let (subbands, next) = parse_ht(&data, &bands, 6, kept, eph).unwrap();
        assert_eq!(next, data.len());
        subbands[0].blocks[0].clone()
    };
    let block = at(6);
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.placeholders, 15);
    assert_eq!(block.zero_bit_planes, 9);
    assert_eq!(segments(&block), [(1, res0[3].1.to_vec())]);
    // Three layers of placeholders: no pass yet, 8 placeholders.
    let block = at(3);
    assert_eq!((block.num_passes, block.placeholders), (0, 8));
    assert!(block.segments.is_empty());
}

/// `ds0_ht_05_b11` (1024 x 1024, four components, PCRL, 7 layers, no
/// delimiters; 32 x 32 code-blocks, 6 levels). The tile's first packets are
/// the 7 layers of component 0's resolution 0, one 16 x 16 `LL` band of one
/// block. By hand:
///
/// ```text
/// L0  c3 00     1, 1, 00001 zero bit-planes 4, 10 two passes, 0 Lb 3;
///               000 then 0: two placeholders (as ds0_ht_02_b11's L0).
/// L1  f0 00     1, 1, 1100 three passes, 0. Five: P0 = 1, cleanup at pass
///               3, 2 passes in: 0000, all 4 bits of the field.
/// L2  fc 40 00  1, 1, 11 11 00010 eight passes, 0. Thirteen: P0 = 4,
///               cleanup at pass 12, 8 passes in: 6 bits 000000.
/// L3  fc 20 00  1, 1, 11 11 00001 seven passes, 0. Twenty: P0 = 6, cleanup
///               at pass 18, 6 passes in: 5 bits 00000, all of the field.
/// L4  f0 00     three passes. 23: P0 = 7, 2 passes in: 0000.
/// L5  fc 00 00  1, 1, 11 11 00000 six passes, 0. 29: P0 = 9, cleanup at
///               pass 27, 5 passes in: 5 bits 00000.
/// L6  fb dd 70  1, 1, 1110 five passes, 11110 Lb 7. 34: P0 = 11, cleanup at
///               pass 33, 5 passes in: 7 + 2 = 9 bits 111010111 = 471.
/// ```
///
/// So the block skips 4 + 11 = 15 bit-planes, and its cleanup pass is the 471
/// bytes after L6's header, which ends 18 bytes into the tile.
#[test]
fn ds0_ht_05_b11_component_0_resolution_0_by_hand() {
    let bytes = read_codestream("ds0_ht_05_b11.j2k");
    let data = tile_data(&bytes);
    assert_eq!(
        data[..18],
        [
            0xC3, 0x00, 0xF0, 0x00, 0xFC, 0x40, 0x00, 0xFC, 0x20, 0x00, 0xF0, 0x00, 0xFC, 0x00,
            0x00, 0xFB, 0xDD, 0x70
        ]
    );
    let bands = [single_block_band(BandKind::Ll, 16, 16)];
    let (subbands, next) = parse_ht(data, &bands, 7, 7, NO_DELIMITERS).unwrap();
    assert_eq!(next, 18 + 471, "the next packet starts after the body");
    let block = &subbands[0].blocks[0];
    assert_eq!(block.num_passes, 1);
    assert_eq!(block.placeholders, 33);
    assert_eq!(block.zero_bit_planes, 15);
    assert_eq!(segments(block), [(1, data[18..18 + 471].to_vec())]);
}

// ---- The pass counts of the conformance corpus ----

/// The pass counts of one codestream's code-blocks over every layer: how many
/// blocks contribute any pass, how many of them have placeholder passes, how
/// many have each count of passes past those, and how many have refinement
/// passes but no refinement bytes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Walked {
    blocks: u64,
    placeholder: u64,
    histogram: BTreeMap<String, u64>,
    zero_length_refinement: u64,
}

/// Walk every tile of `codestream` and count its code-blocks' passes. `None`
/// when the walk is refused, as a MIXED code-block style is in the main
/// header.
fn walk_passes(codestream: &[u8]) -> Option<Walked> {
    let parsed = crate::codestream::parse_header(codestream).ok()?;
    let cs = crate::codestream::parse_tiles(codestream, parsed).ok()?;
    let mut walked = Walked::default();
    for (index, tile) in cs.tiles.iter().enumerate() {
        let header = cs.tile_header(index).unwrap();
        let mut visits = u64::MAX;
        let coded = decode_packets(&header, tile, &mut visits, u64::MAX, 0)
            .unwrap_or_else(|e| panic!("tile {index}: {e:?}"));
        let blocks = coded
            .components
            .iter()
            .flat_map(|c| &c.resolutions)
            .flat_map(|r| &r.subbands)
            .flat_map(|b| &b.blocks);
        for block in blocks {
            if block.num_passes + block.placeholders == 0 {
                continue;
            }
            walked.blocks += 1;
            walked.placeholder += u64::from(block.placeholders > 0);
            *walked
                .histogram
                .entry(block.num_passes.to_string())
                .or_default() += 1;
            let refinement = block.segments.get(1).map_or(0, |s| s.bytes().len());
            walked.zero_length_refinement += u64::from(block.num_passes > 1 && refinement == 0);
        }
    }
    Some(walked)
}

fn walked_of(entry: &serde_json::Value) -> Option<Walked> {
    let walked = entry["passes"]["walked"].as_object()?;
    Some(Walked {
        blocks: walked["blocks"].as_u64().unwrap(),
        placeholder: walked["placeholder"].as_u64().unwrap(),
        histogram: walked["histogram"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
            .collect(),
        zero_length_refinement: walked["zero_length_refinement"].as_u64().unwrap(),
    })
}

/// The codestream of a manifest entry: the file, or its `codestream_range`.
fn codestream_of<'a>(bytes: &'a [u8], entry: &serde_json::Value) -> &'a [u8] {
    match entry["codestream_range"].as_array() {
        Some(range) => {
            &bytes[range[0].as_u64().unwrap() as usize..range[1].as_u64().unwrap() as usize]
        }
        None => bytes,
    }
}

/// Every HT codestream of the conformance and OpenJPH corpora walks, and its
/// pass counts are the ones its manifest records under `passes.walked`, from
/// which the `IN_CLASS` lists of `tests/conformance_ht.rs` are checked. Where
/// OpenJPEG counted the passes too (`passes.measured`, on the files it decodes
/// whole), the two counts agree. Run with `--nocapture` for the counts.
#[test]
fn the_corpus_pass_counts_are_the_manifests() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let sets: [(&Path, &str, &[&str]); 2] = [
        (&ht_dir(), "conformance/ht", &["codestreams", "jph"]),
        (&root.join("openjph"), "openjph", &["entries"]),
    ];
    let mut checked = 0;
    let mut mismatched = Vec::new();
    for (dir, label, lists) in sets {
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        for list in lists {
            for entry in manifest[*list].as_array().unwrap() {
                let name = entry["name"].as_str().unwrap();
                let file = entry["codestream"]
                    .as_str()
                    .or(entry["file"].as_str())
                    .unwrap();
                let bytes = std::fs::read(dir.join(file)).unwrap();
                let walked = walk_passes(codestream_of(&bytes, entry));
                eprintln!("{label} {name}: {walked:?}");
                if walked != walked_of(entry) {
                    mismatched.push(format!("{label} {name}"));
                }
                if let (Some(walked), Some(measured)) =
                    (&walked, entry["passes"]["measured"].as_object())
                {
                    assert_eq!(walked.blocks, measured["blocks"].as_u64().unwrap());
                    let histogram: BTreeMap<String, u64> = measured["histogram"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
                        .collect();
                    assert_eq!(walked.histogram, histogram, "{label} {name}");
                    assert_eq!(
                        walked.zero_length_refinement,
                        measured["zero_length_refinement"].as_u64().unwrap(),
                        "{label} {name}"
                    );
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 41 + 20 + 16);
    assert!(
        mismatched.is_empty(),
        "walked counts differ: {mismatched:?}"
    );
}
