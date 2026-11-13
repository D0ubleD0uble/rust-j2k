//! The decode's tier-1 work bound ([`super::decode_work`]) against the
//! committed files: what each one's code-blocks cost the block decoder, as a
//! fraction of its bound.

use std::path::{Path, PathBuf};

use super::{decode_work, numbps, passes_run, work};
use crate::tier2::{BandKind, CodeBlock, CodedData, ComponentCoded, Resolution, Subband};

/// How far below its bound every committed file's block decode must stay:
/// at most this fraction of it. The most encoder output was seen to spend
/// is 42.5% of the bound (`opj_compress -n 1` on a flat 16-bit image, the
/// committed `opj_flat2048_16bit_n1`), so half leaves it room, and a file
/// past half spends more than any encoder output measured.
const MARGIN: f64 = 1.0 / 2.0;

/// Every `.j2k` and `.jp2` under `dir`, recursively.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read fixtures").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "j2k" || e == "jp2") {
            out.push(path);
        }
    }
}

/// The sample-passes a full-resolution decode of every tile spends in the
/// block decoder, the bytes of tile data the bound is scaled by, and the
/// bytes of code-block data; `None` when the file has no codestream or a
/// tile's packets do not parse.
fn spend(bytes: &[u8]) -> Option<(u64, u64, u64)> {
    let codestream = if bytes.starts_with(&[0xFF, 0x4F]) {
        bytes
    } else {
        crate::jp2::parse(bytes, &crate::WorkLimit::Default)
            .ok()?
            .codestream
    };
    let parsed = crate::codestream::parse_header(codestream).ok()?;
    let cs = crate::codestream::parse_tiles(codestream, parsed).ok()?;
    let (mut spent, mut block_bytes) = (0u64, 0u64);
    for (index, tile) in cs.tiles.iter().enumerate() {
        let header = cs.tile_header(index).ok()?;
        let mut visits = u64::MAX;
        let coded = crate::tier2::decode_packets(&header, tile, &mut visits, u64::MAX, 0).ok()?;
        spent += work(&header, &coded, 0);
        for component in &coded.components {
            for resolution in &component.resolutions {
                for subband in &resolution.subbands {
                    for block in &subband.blocks {
                        for segment in &block.segments {
                            block_bytes +=
                                segment.chunks.iter().map(|c| c.len() as u64).sum::<u64>();
                        }
                    }
                }
            }
        }
    }
    Some((spent, crate::tile_bytes(&cs), block_bytes))
}

/// The bound is 2^26 sample-passes, or 2^15 per byte of tile data past
/// 2^11 bytes.
#[test]
fn the_tier1_work_bound_grows_with_the_tile_data() {
    assert_eq!(decode_work(0), 1 << 26);
    assert_eq!(decode_work(1 << 11), 1 << 26);
    assert_eq!(decode_work((1 << 11) + 1), (1 << 26) + (1 << 15));
    assert_eq!(decode_work(1 << 24), 1 << 39);
    assert_eq!(decode_work(u64::MAX), u64::MAX);
}

/// Every committed file whose packets parse, the conformance corpus and the
/// real-world files included, spends a small fraction of the tier-1 bound,
/// so the bound refuses none of them. Run with `--nocapture` for the table.
#[test]
fn every_committed_file_decodes_far_below_the_tier1_work_bound() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths = Vec::new();
    files(&root, &mut paths);
    paths.sort();
    let mut rows = Vec::new();
    for path in &paths {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        // These are made to go past the bound, but for the encoder output.
        if name.starts_with("tier1_work_bound/t1bomb") {
            continue;
        }
        let bytes = std::fs::read(path).unwrap();
        if let Some((spent, data, block_bytes)) = spend(&bytes) {
            let bound = decode_work(data);
            rows.push((spent as f64 / bound as f64, spent, data, block_bytes, name));
        }
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    eprintln!(
        "fraction of bound\tsample-passes\tdata bytes\tper byte\tblock bytes\tper block byte\tfile"
    );
    for (fraction, spent, data, block_bytes, name) in &rows {
        let per_byte = *spent as f64 / (*data).max(1) as f64;
        let per_block_byte = *spent as f64 / (*block_bytes).max(1) as f64;
        eprintln!(
            "{fraction:.6}\t{spent}\t{data}\t{per_byte:.1}\t{block_bytes}\t{per_block_byte:.1}\t{name}"
        );
    }
    eprintln!("{} files measured of {}", rows.len(), paths.len());
    assert!(rows.len() > 100, "{} files measured", rows.len());
    for name in [
        "tier1_work_bound/opj_flat2048_16bit_n1.j2k",
        "tier1_work_bound/opj_specks2048_16bit.j2k",
    ] {
        assert!(rows.iter().any(|row| row.4 == name), "{name} not measured");
    }
    for (fraction, _, _, _, name) in &rows {
        assert!(*fraction <= MARGIN, "{name} spends {fraction} of its bound");
    }
}

/// One code-block of `width` x `height` with `passes` passes and no zero
/// bit-planes.
fn block(width: usize, height: usize, passes: u32) -> CodeBlock<'static> {
    CodeBlock {
        x: 0,
        y: 0,
        width,
        height,
        num_passes: passes,
        zero_bit_planes: 0,
        segments: Vec::new(),
    }
}

/// A band of `kind` holding `blocks`.
fn band(kind: BandKind, blocks: Vec<CodeBlock<'static>>) -> Subband<'static> {
    Subband {
        kind,
        origin: (0, 0),
        width: 64,
        height: 64,
        block_cols: blocks.len(),
        block_rows: 1,
        blocks,
    }
}

/// A block runs its passes, but no more than `3 x top - 2` its coded
/// bit-planes hold, and none when it has no coded plane or more than the
/// decode takes.
#[test]
fn a_block_runs_no_more_passes_than_its_bit_planes_hold() {
    assert_eq!(passes_run(10, &block(4, 4, 0), 0), 0);
    assert_eq!(passes_run(10, &block(4, 4, 7), 0), 7);
    assert_eq!(passes_run(10, &block(4, 4, 28), 0), 28);
    assert_eq!(passes_run(10, &block(4, 4, 29), 0), 28);
    assert_eq!(passes_run(30, &block(4, 4, 164), 0), 88);
    // A maxshift raises the top plane.
    assert_eq!(passes_run(10, &block(4, 4, 164), 2), 34);
    // No coded plane, and more than the decode takes.
    let mut none = block(4, 4, 5);
    none.zero_bit_planes = 10;
    assert_eq!(passes_run(10, &none, 0), 0);
    assert_eq!(passes_run(31, &block(4, 4, 5), 0), 0);
}

/// The work is passes times samples over every code-block decoded; a
/// resolution reduction leaves out the bands it drops.
#[test]
fn the_work_counts_the_code_blocks_decoded() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/multilayer_lossless.j2k"
    ))
    .unwrap();
    let header = crate::codestream::parse_header(&bytes).unwrap().header;
    let coded = CodedData {
        components: vec![ComponentCoded {
            resolutions: vec![
                Resolution {
                    subbands: vec![band(BandKind::Ll, vec![block(8, 8, 1)])],
                },
                Resolution {
                    subbands: vec![
                        band(BandKind::Hl, vec![block(16, 16, 2), block(16, 4, 0)]),
                        band(BandKind::Lh, vec![block(16, 16, 3)]),
                        band(BandKind::Hh, vec![block(16, 16, 4)]),
                    ],
                },
            ],
        }],
    };
    assert_eq!(work(&header, &coded, 0), 64 + 256 * (2 + 3 + 4));
    assert_eq!(work(&header, &coded, 1), 64);
    // A reduction past the pyramid keeps the coarsest resolution, as the
    // decode does (it refuses that reduction before this point).
    assert_eq!(work(&header, &coded, 5), 64);
}

/// A component's maxshift raises its blocks' top plane, so the work counts
/// the passes that shift lets them run.
#[test]
fn the_work_counts_the_passes_a_maxshift_adds() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/multilayer_lossless.j2k"
    ))
    .unwrap();
    let header = crate::codestream::parse_header(&bytes).unwrap().header;
    let coded = CodedData {
        components: vec![ComponentCoded {
            resolutions: vec![Resolution {
                subbands: vec![band(BandKind::Ll, vec![block(8, 8, 164)])],
            }],
        }],
    };
    let planes = numbps(&header, 0, 0).unwrap();
    let mut shifted = header.clone();
    shifted.components[0].roi_shift = 5;
    assert_eq!(work(&header, &coded, 0), 64 * u64::from(3 * planes - 2));
    assert_eq!(
        work(&shifted, &coded, 0),
        64 * u64::from(3 * (planes + 5) - 2)
    );
}
