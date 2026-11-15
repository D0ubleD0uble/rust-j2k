//! Structured fuzz entry points (see `fuzz/`).
//!
//! The `decode` fuzz target must assemble a byte-perfect main header before a
//! mutation reaches tier-2 or tier-1, so those state machines see little of
//! the fuzzer's throughput. The helpers here pin the well-formed scaffolding
//! and hand the fuzzer only the untrusted bytes one stage consumes. The
//! contract under test is the crate-wide one — typed errors, never a panic,
//! bounded work — so results are discarded.
//!
//! `#[doc(hidden)]` and semver-exempt: a test seam for the detached fuzz
//! workspace, not public API. A smoke test below keeps it building in CI.

use crate::tier1::ebcot::mq::MqDecoder;
use crate::tier1::ebcot::passes::{
    BlockParams, BlockState, Orientation, decode_block, init_contexts,
};
use crate::tier2::{CodeBlock, CodedSegment};

/// Drive the raw MQ decoder over `data`: context transitions, BYTEIN
/// stuffing, marker handling, and past-end synthesis, with bounded work.
pub fn mq_stream(data: &[u8]) {
    let mut mq = MqDecoder::new(data);
    // The real per-block context array, standard initial states included.
    let mut contexts = init_contexts();
    // Enough decisions to consume every coded byte and run well past the end
    // into marker synthesis, capped so a large input cannot hang an exec.
    let decisions = (data.len() * 8).clamp(256, 1 << 20);
    for i in 0..decisions {
        let _ = mq.decode(&mut contexts[i % contexts.len()]);
    }
}

/// Decode one code-block whose shape, coding parameters, and coded bytes are
/// all steered by `data`, the way tier-2 would hand them to tier-1. Each
/// parameter reads its own byte so no region of the grid is unreachable.
pub fn tier1_block(data: &[u8]) {
    let [a, b, c, d, e, f, g, coded @ ..] = data else {
        return;
    };
    use crate::codestream::markers::code_block_style::{PTERM, RESET, SEGSYM, TERMALL, VCAUSAL};

    let width = u32::from(a & 0x3F) + 1; // 1..=64, the subset's block ceiling
    let height = u32::from(b & 0x3F) + 1;
    let orient = match a >> 6 {
        0 => Orientation::Ll,
        1 => Orientation::Hl,
        2 => Orientation::Lh,
        _ => Orientation::Hh,
    };
    let params = BlockParams {
        // Every decoded style flag whose contract this hook can honor. `LAZY`
        // stays out: `decode_block` requires the lazy split's homogeneous
        // segments (raw runs never carry a cleanup pass), and this hook feeds
        // one unsplit segment — the `tile_body` target's bypass shape covers
        // it through the real tier-2 split instead. The HTJ2K bits never
        // reach this decoder: an HT block goes to the HT coder, and the
        // codestream's block-coder guard refuses MIXED.
        style: c & (RESET | TERMALL | VCAUSAL | PTERM | SEGSYM),
        roi_shift: d & 0x3F,
        // A spare bit of the maxshift byte: whether a layer cap cut the
        // (one) segment short, which lifts its `predictable termination` check.
        last_segment_cut: d & 0x40 != 0,
    };
    let num_passes = u32::from(*e) % 110; // tier-2's 109-pass segment cap
    let numbps = u32::from(*f) % 31; // MAX_BIT_PLANES
    // One past the largest agreeing value, so the zero_bit_planes > numbps
    // saturation path is exercised alongside in-range decodes.
    let zero_bit_planes = u32::from(*g) % 32;

    let mut state = BlockState::new(width, height);
    let segments = [(coded.to_vec(), num_passes)];
    let _ = decode_block(
        &segments,
        &mut state,
        orient,
        numbps,
        num_passes,
        zero_bit_planes,
        params,
    );
}

/// Decode one HT code-block whose shape, coding parameters and segments all
/// come from `data`, straight into the HT coder, the way tier-1 hands it a
/// block. The first 13 bytes steer; the rest are coded bytes.
///
/// - Bytes 0 to 2 and 12: the shape. Its width is 1 to 1024 and its height 1 to
///   `min(1024, 4096 / width)`, so 1x1, 1x1024, 1024x4, 4x1024 and odd sizes
///   all occur, up to the 4096 samples a block holds. With flag bit 3 both
///   sides are 1 to 64 instead.
/// - Byte 3: flags. Bit 0 is the causal flag, bit 1 makes the input dense in
///   0xFF, bit 2 stretches the segments by repeating the coded bytes, bit 3
///   is the small shape above, bit 4 gives a `CAP` bound, bit 5 a maxshift,
///   bit 6 splits each segment into chunks as layers do, and bit 7 rewrites
///   the cleanup segment's last two bytes into a valid `Scup`.
/// - Bytes 4 and 5: `Mb` and `P`, each 0 to 39, past the 30-plane limit.
/// - Byte 6: the coding passes (0 to 4, one more than a set holds) and, from
///   bit 3, the maxshift.
/// - Bytes 7 to 10: the cleanup length `Lcup` and the refinement length, each
///   0 to 5000 when the segments are stretched, else cut to the bytes there
///   are.
/// - Byte 11: the `CAP` bound, the chunk split and the `Scup` to write.
/// - Byte 12: the high byte of the height (byte 2 is the low one).
pub fn ht_block(data: &[u8]) {
    let _ = run_ht_block(data);
}

/// [`ht_block`], giving the block's shape and the samples it decoded, or `None` when `data` is too
/// short to steer a block.
fn run_ht_block(data: &[u8]) -> Option<((usize, usize), crate::Result<usize>)> {
    let [
        w0,
        w1,
        h,
        flags,
        mb,
        zbp,
        pr,
        l0,
        l1,
        r0,
        r1,
        extra,
        h1,
        coded @ ..,
    ] = data
    else {
        return None;
    };
    let width = 1 + usize::from(u16::from_le_bytes([*w0, *w1]) % 1024);
    let height = 1 + usize::from(u16::from_le_bytes([*h, *h1])) % (4096 / width).min(1024);
    let (width, height) = if flags & 0x08 != 0 {
        (1 + usize::from(*w0 & 0x3F), 1 + usize::from(*h & 0x3F))
    } else {
        (width, height)
    };
    let numbps = u32::from(*mb) % 40;
    let zero_bit_planes = u32::from(*zbp) % 40;
    let num_passes = u32::from(pr & 7) % 5;
    let roi_shift = if flags & 0x20 != 0 {
        (pr >> 3) & 0x1F
    } else {
        0
    };
    let bound_bits = (flags & 0x10 != 0).then(|| u32::from(*extra) % 80);

    // The two segments' bytes: `coded` in order, or `coded` repeated to the
    // lengths asked for (to 5000 bytes each).
    let stretch = flags & 0x04 != 0;
    let dense = |b: u8| {
        if flags & 0x02 != 0 && b >= 0x40 {
            0xFF
        } else {
            b
        }
    };
    let cleanup_len = usize::from(u16::from_le_bytes([*l0, *l1]) % 5001);
    let refine_len = usize::from(u16::from_le_bytes([*r0, *r1]) % 5001);
    let (mut cleanup, refinement): (Vec<u8>, Vec<u8>) = if stretch && !coded.is_empty() {
        let fill = |from: usize, len: usize| -> Vec<u8> {
            (0..len)
                .map(|i| dense(coded[(from + i) % coded.len()]))
                .collect()
        };
        (fill(0, cleanup_len), fill(cleanup_len, refine_len))
    } else {
        let (a, b) = coded.split_at(cleanup_len.min(coded.len()));
        (
            a.iter().map(|&x| dense(x)).collect(),
            b.iter().take(refine_len).map(|&x| dense(x)).collect(),
        )
    };
    if flags & 0x80 != 0 && cleanup.len() >= 2 {
        let n = cleanup.len();
        let scup = 2 + usize::from(*extra) * 16 % (n.min(4079) - 1);
        cleanup[n - 1] = (scup >> 4) as u8;
        cleanup[n - 2] = (cleanup[n - 2] & 0xF0) | (scup & 0xF) as u8;
    }

    // A segment's bytes in one chunk, or two, as two layers deliver them.
    fn chunks(bytes: &[u8], split: Option<u8>) -> Vec<&[u8]> {
        match split {
            Some(at) if bytes.len() > 1 => {
                let at = 1 + usize::from(at) % (bytes.len() - 1);
                vec![&bytes[..at], &bytes[at..]]
            }
            _ => vec![bytes],
        }
    }
    let split = (flags & 0x40 != 0).then_some(*extra);
    let mut segments = vec![CodedSegment {
        passes: 1,
        chunks: chunks(&cleanup, split),
    }];
    if num_passes > 1 || !refinement.is_empty() {
        segments.push(CodedSegment {
            passes: num_passes.saturating_sub(1),
            chunks: chunks(&refinement, split),
        });
    }
    let block = CodeBlock {
        x: 0,
        y: 0,
        width,
        height,
        num_passes,
        zero_bit_planes,
        segments,
        cut: false,
        placeholders: 0,
        missing_from: None,
    };
    let decoded =
        crate::tier1::decode_ht_block(&block, numbps, roi_shift, flags & 1 != 0, bound_bits);
    if let Ok(coefficients) = &decoded {
        assert_eq!(coefficients.len(), width * height);
    }
    Some(((width, height), decoded.map(|c| c.len())))
}

/// Open `data` as an ICC profile and read it as a monochrome and as an RGB
/// input profile, without building the transform to sRGB. The transform is
/// far costlier than the reading, so the `icc_srgb` target builds it for
/// only some inputs.
pub fn icc_profile(data: &[u8]) {
    crate::jp2::SrgbPlan::read_profile(data);
}

#[cfg(test)]
mod tests {
    /// Smoke-run every hook so the fuzz surface builds and stays panic-free in
    /// CI, where `--cfg fuzzing` never is set.
    #[test]
    fn fuzz_hooks_run_on_sample_inputs() {
        let samples: [&[u8]; 5] = [
            &[],
            &[0xFF; 40],
            &[0x00; 40],
            &[0x55, 0x1E, 0x22, 0x0A, 0x03, 0x80, 0xFF, 0x91],
            &[0xC0, 0x24, 0x6D, 0x1E, 0x21, 0xFF, 0xD9, 0x00],
        ];
        for sample in samples {
            super::mq_stream(sample);
            super::tier1_block(sample);
            super::ht_block(sample);
            super::icc_profile(sample);
        }
    }

    /// The hook reaches the tall shapes the review of #360 and #367 asked for:
    /// a height past 256 reads its second byte.
    #[test]
    fn ht_block_reaches_tall_shapes() {
        let shape = |w: u16, h: u16| {
            let mut data = vec![0u8; 13];
            data[..2].copy_from_slice(&(w - 1).to_le_bytes());
            data[2] = (h - 1) as u8;
            data[12] = ((h - 1) >> 8) as u8;
            super::run_ht_block(&data).unwrap().0
        };
        assert_eq!(shape(1, 1024), (1, 1024));
        assert_eq!(shape(4, 1024), (4, 1024));
        assert_eq!(shape(2, 1024), (2, 1024));
        assert_eq!(shape(8, 512), (8, 512));
        assert_eq!(shape(1024, 4), (1024, 4));
        assert_eq!(shape(1, 1), (1, 1));
    }

    /// The HT conformance codestreams whose blocks seed the `ht_block` target.
    const HT_SEED_FILES: [&str; 6] = [
        "ds0_ht_02_b11",
        "ds0_ht_04_b11",
        "ds0_ht_05_b11",
        "ds1_ht_01_b11",
        "ds1_ht_04_b9",
        "ds0_ht_12_b11",
    ];

    /// Slice the code-blocks of an HT codestream into `ht_block` inputs: the
    /// block's shape, passes and bytes, in the layout the hook reads. A seed's
    /// `Mb` is `P + 6`, which leaves planes to code.
    fn ht_seeds(name: &str) -> Vec<Vec<u8>> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/conformance/ht/codestreams")
            .join(format!("{name}.j2k"));
        // The conformance directory is not in the published package.
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        let parsed = crate::codestream::parse_header(&bytes).unwrap();
        let cs = crate::codestream::parse_tiles(&bytes, parsed).unwrap();
        let mut seeds = Vec::new();
        for (index, tile) in cs.tiles.iter().enumerate() {
            let header = cs.tile_header(index).unwrap();
            let mut visits = u64::MAX;
            let coded =
                crate::tier2::decode_packets(&header, tile, &mut visits, u64::MAX, 0).unwrap();
            let blocks = coded
                .components
                .iter()
                .flat_map(|c| &c.resolutions)
                .flat_map(|r| &r.subbands)
                .flat_map(|s| &s.blocks);
            for block in blocks {
                let [cleanup, rest @ ..] = &block.segments[..] else {
                    continue;
                };
                let cleanup = cleanup.bytes();
                let refinement = rest.first().map(|s| s.bytes()).unwrap_or_default();
                let mut seed = ((block.width - 1) as u16).to_le_bytes().to_vec();
                seed.push((block.height - 1) as u8);
                seed.push(0);
                seed.push((block.zero_bit_planes + 6) as u8);
                seed.push(block.zero_bit_planes as u8);
                seed.push(block.num_passes as u8);
                seed.extend((cleanup.len() as u16).to_le_bytes());
                seed.extend((refinement.len() as u16).to_le_bytes());
                seed.push(0);
                seed.push(((block.height - 1) >> 8) as u8);
                seed.extend(&cleanup);
                seed.extend(&refinement);
                // The same bytes in the tall shapes the corpus lacks.
                if seeds.len() % 64 == 0 {
                    for (w, h) in [(1u16, 1024u16), (4, 1024), (2, 1024), (8, 512)] {
                        let mut tall = seed.clone();
                        tall[..2].copy_from_slice(&(w - 1).to_le_bytes());
                        tall[2] = (h - 1) as u8;
                        tall[12] = ((h - 1) >> 8) as u8;
                        seeds.push(tall);
                    }
                }
                seeds.push(seed);
            }
        }
        seeds
    }

    /// The seeds sliced from the HT corpus decode through the hook, so the
    /// target starts from blocks it accepts. Set `RUST_J2K_HT_SEED_DIR` to
    /// write them out as the target's seed corpus (see `fuzz/README.md`).
    #[test]
    fn ht_block_seeds_from_the_corpus_decode() {
        let dir = std::env::var_os("RUST_J2K_HT_SEED_DIR").map(std::path::PathBuf::from);
        let (mut total, mut refined) = (0usize, 0usize);
        for name in HT_SEED_FILES {
            for (i, seed) in ht_seeds(name).iter().enumerate() {
                let width = 1 + usize::from(u16::from_le_bytes([seed[0], seed[1]]));
                let height = 1 + usize::from(u16::from_le_bytes([seed[2], seed[12]]));
                let (shape, decoded) = super::run_ht_block(seed).expect("a full header");
                assert_eq!(shape, (width, height));
                // The tall copies carry bytes made for another shape.
                if height <= 256 && width * height <= 4096 && !(height == 512) {
                    assert_eq!(decoded.expect("a corpus block decodes"), width * height);
                }
                total += 1;
                refined += usize::from(seed[6] > 1);
                if let Some(dir) = &dir {
                    std::fs::create_dir_all(dir).unwrap();
                    std::fs::write(dir.join(format!("{name}_{i}")), seed).unwrap();
                }
            }
        }
        if total > 0 {
            assert!(refined > 0, "no seed runs a refinement pass");
        }
    }
}
