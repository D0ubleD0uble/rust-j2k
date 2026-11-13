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
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/multilayer_lossless.j2k"
    ))
    .unwrap();
    let parsed = crate::codestream::parse_header(&bytes).unwrap();
    let cs = crate::codestream::parse_tiles(&bytes, parsed).unwrap();
    let header = cs.tile_header(0).unwrap().into_owned();
    let tile = &cs.tiles[0];
    let layers = header.cod.layers;
    assert_eq!(layers, 3);

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
            assert!(!ks.is_empty(), "cut {cut}: block {b} holds part of a layer");
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
    assert!(
        split,
        "no cut kept a block at an earlier layer beside a whole one"
    );
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
