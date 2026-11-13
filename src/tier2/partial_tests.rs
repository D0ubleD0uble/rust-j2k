//! A packet cut short under `strict` off, at the code-block level: each
//! code-block holds whole layers, those the data gives it in full.

use std::borrow::Cow;

use super::{CodeBlock, CodedData, decode_packets_partial};
use crate::codestream::{MainHeader, Tile};

/// Every code-block of `coded`, in walk order, with the index of the
/// (component, resolution) it is in.
fn blocks<'a>(coded: &'a CodedData<'a>) -> Vec<(usize, &'a CodeBlock<'a>)> {
    coded
        .components
        .iter()
        .flat_map(|c| &c.resolutions)
        .enumerate()
        .flat_map(|(at, r)| {
            r.subbands
                .iter()
                .flat_map(move |b| b.blocks.iter().map(move |k| (at, k)))
        })
        .collect()
}

fn walk<'a>(header: &MainHeader, tile: &'a Tile<'a>) -> CodedData<'a> {
    let mut visits = u64::MAX;
    decode_packets_partial(header, tile, &mut visits, u64::MAX, 0)
        .unwrap_or_else(|_| panic!("the walk fails"))
        .0
}

/// `multilayer_lossless.j2k` (three layers, LRCP) cut at every byte of its
/// tile data. Each code-block is what a decode of the first `k` layers of
/// the whole file gives it, for some `k`: a cut packet adds the code-blocks
/// whose data is all there and leaves the rest at the layers before (B.11),
/// where OpenJPEG drops those layers too. Some cut leaves code-blocks of one
/// resolution, which is one precinct and so one packet a layer, at two layer
/// counts, the lower at least 1.
#[test]
fn a_cut_packet_keeps_each_code_blocks_whole_layers() {
    assert!(
        whole_layers("multilayer_lossless.j2k", 3),
        "no cut kept a block at an earlier layer beside a whole one"
    );
}

/// The same under the code-block styles that end segments within a block's
/// passes (bypass, restart) or not (the others), two layers each: a block
/// whose layer 2 continues its layer 1 segment and opens another keeps
/// neither when the second runs out.
#[test]
fn a_cut_packet_keeps_whole_layers_under_each_code_block_style() {
    for name in [
        "cblksty_bypass_lossless.j2k",
        "cblksty_restart_lossless.j2k",
        "cblksty_restart_pterm_segsym_lossless.j2k",
        "cblksty_pterm_lossless.j2k",
        "cblksty_reset_lossless.j2k",
    ] {
        whole_layers(name, 2);
    }
}

/// Cut the tile data of fixture `name`, of one tile and `layers` layers, at
/// every byte, and check each code-block is what a decode of the first `k`
/// layers of the whole file gives it, for some `k`. Whether some cut left
/// code-blocks of one resolution at two layer counts, the lower at least 1.
fn whole_layers(name: &str, layers: u16) -> bool {
    let bytes = std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let parsed = crate::codestream::parse_header(&bytes).unwrap();
    let cs = crate::codestream::parse_tiles(&bytes, parsed).unwrap();
    assert_eq!(cs.tiles.len(), 1, "{name}");
    let header = cs.tile_header(0).unwrap().into_owned();
    let tile = &cs.tiles[0];
    assert_eq!(header.cod.layers, layers, "{name}");

    // The first k layers of the whole file, for k from 0 to 3: the same data
    // under a COD that declares k layers, its packets past them left over.
    let limited: Vec<CodedData<'_>> = (0..=layers)
        .map(|k| {
            let mut header = header.clone();
            header.cod.layers = k.max(1);
            if k == 0 {
                let empty = Tile {
                    data: Cow::Borrowed(&[]),
                    part_ends: vec![0],
                    ..tile.clone()
                };
                // A tile with no data: every block empty, borrowing nothing.
                let coded = walk(&header, &empty);
                return strip(coded);
            }
            walk(&header, tile)
        })
        .collect();
    let limited: Vec<Vec<(usize, &CodeBlock<'_>)>> = limited.iter().map(blocks).collect();

    let mut split = false;
    for cut in 0..tile.data.len() {
        let short = Tile {
            data: Cow::Borrowed(&tile.data[..cut]),
            part_ends: vec![cut],
            ..tile.clone()
        };
        let coded = walk(&header, &short);
        let got = blocks(&coded);
        // For each block, its resolution and the layer counts it matches.
        let mut matched: Vec<(usize, Vec<u16>)> = Vec::new();
        for (b, (at, block)) in got.iter().enumerate() {
            let ks: Vec<u16> = (0..=layers)
                .filter(|&k| limited[usize::from(k)][b].1 == *block)
                .collect();
            assert!(
                !ks.is_empty(),
                "{name} cut {cut}: block {b} holds part of a layer"
            );
            matched.push((*at, ks));
        }
        // A block that holds layers 1 to k but not layer k + 1, beside one of
        // the same packet that holds layer k + 1.
        for (at, ks) in &matched {
            for &k in ks {
                let earlier = k >= 1 && !ks.contains(&0) && !ks.contains(&(k + 1));
                split |= earlier
                    && matched.iter().any(|(other, os)| {
                        other == at && os.contains(&(k + 1)) && !os.contains(&k)
                    });
            }
        }
    }
    split
}

/// `coded` with its blocks' segments made `'static`: the empty decode holds
/// no bytes.
fn strip(coded: CodedData<'_>) -> CodedData<'static> {
    let mut out = CodedData::default();
    for component in coded.components {
        let mut c = super::ComponentCoded::default();
        for resolution in component.resolutions {
            let subbands = resolution
                .subbands
                .into_iter()
                .map(|band| super::Subband {
                    blocks: band
                        .blocks
                        .into_iter()
                        .map(|block| {
                            assert!(block.segments.is_empty());
                            CodeBlock {
                                segments: Vec::new(),
                                ..block
                            }
                        })
                        .collect(),
                    ..band
                })
                .collect();
            c.resolutions.push(super::Resolution { subbands });
        }
        out.components.push(c);
    }
    out
}

/// A packet whose inline header runs past the end of the data adds nothing,
/// even to a code-block whose entry was read whole before the cut. One 8 x 4
/// tile of two 4 x 4 code-blocks, no decomposition, one layer; the header
/// gives the first block one pass and no bytes, the second one pass and one
/// byte: nonempty 1; included 1 1, zero bit-planes 1 1, one pass 0, Lblock
/// 0, length 000; included 1, zero bit-planes 1, one pass 0, Lblock 0,
/// length 001. That is 17 bits, `F8 30 80`; the data is cut after `F8 30`.
#[test]
fn a_cut_inline_header_adds_nothing() {
    fn segment(code: u16, body: &[u8]) -> Vec<u8> {
        let mut out = code.to_be_bytes().to_vec();
        out.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
        out.extend_from_slice(body);
        out
    }
    let mut siz = vec![0, 0];
    for value in [8u32, 4, 0, 0, 8, 4, 0, 0] {
        siz.extend_from_slice(&value.to_be_bytes());
    }
    siz.extend_from_slice(&[0, 1, 7, 1, 1]);
    let data = [0xF8, 0x30, 0x80, 0x00];
    let mut bytes = vec![0xFF, 0x4F];
    bytes.extend(segment(0xFF51, &siz));
    bytes.extend(segment(0xFF52, &[0, 0, 0, 1, 0, 0, 0, 0, 0, 1]));
    bytes.extend(segment(0xFF5C, &[0x20, 9 << 3]));
    let mut sot = vec![0, 0];
    sot.extend_from_slice(&(12 + 2 + data.len() as u32).to_be_bytes());
    sot.extend_from_slice(&[0, 1]);
    bytes.extend(segment(0xFF90, &sot));
    bytes.extend_from_slice(&[0xFF, 0x93]);
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&[0xFF, 0xD9]);

    let parsed = crate::codestream::parse_header(&bytes).unwrap();
    let cs = crate::codestream::parse_tiles(&bytes, parsed).unwrap();
    let header = cs.tile_header(0).unwrap().into_owned();
    let tile = &cs.tiles[0];
    let at = |cut: usize| Tile {
        data: Cow::Borrowed(&tile.data[..cut]),
        part_ends: vec![cut],
        ..tile.clone()
    };
    let empty = strip(walk(&header, &at(0)));
    let empty = blocks(&empty);
    // The whole header with its body cut: the first block's entry and data
    // are whole, so it keeps its pass.
    let body_cut = at(3);
    let body_cut = walk(&header, &body_cut);
    let body_cut = blocks(&body_cut);
    assert_eq!(body_cut.len(), 2);
    assert_eq!(body_cut[0].1.num_passes, 1);
    assert_eq!(body_cut[1], empty[1]);
    // The header cut: nothing.
    let header_cut = at(2);
    let header_cut = walk(&header, &header_cut);
    assert_eq!(blocks(&header_cut), empty);
}
