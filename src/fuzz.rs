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
use crate::tier2::{CodeBlock, Segments};

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
        // reach this decoder: an HT block goes to the HT coder, and a Part 1
        // block of a MIXED tile-component gets the style without them.
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
///   bit 3, the maxshift: 0 to 30, or with 31 the whole of byte 12, 0 to 255.
///   With the maxshift flag, byte 5 is the zero bit-planes up to 255.
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
    // Under a maxshift the zero bit-planes reach the 255 a byte holds, past
    // `Mb + s`'s usual range, as the packet header allows up to 293.
    let zero_bit_planes = if flags & 0x20 != 0 {
        u32::from(*zbp)
    } else {
        u32::from(*zbp) % 40
    };
    let num_passes = u32::from(pr & 7) % 5;
    let roi_shift = if flags & 0x20 != 0 {
        match (pr >> 3) & 0x1F {
            31 => *h1,
            s => s,
        }
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
    let mut segments = Segments::default();
    segments.push(1, &chunks(&cleanup, split));
    if num_passes > 1 || !refinement.is_empty() {
        segments.push(num_passes.saturating_sub(1), &chunks(&refinement, split));
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
        coder: crate::tier2::Coder::Ht,
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

/// How far above its cap [`with_code_block_work_cap`] lets a code-block bound
/// go: the most a `Raised` factor or `Unlimited` sets is this times the cap.
/// The `retry_ladder` target decodes at factors up to this, so its ladder
/// sees the factors it asks for.
pub const CODE_BLOCK_CAP_FACTOR: u64 = 4;

/// The sample-passes [`with_code_block_work_cap`] charges for each sample a
/// tile reconstructs. Under the fuzz build (ASan) a sample of a thin image
/// with empty packets takes about as long to reconstruct as this many
/// sample-passes of the slowest block data.
pub const CAP_WORK_PER_SAMPLE: u64 = 4;

/// `options` with the code-block work bound set to `cap` sample-passes,
/// so a fuzz target's time does not grow with the input's bytes. The bound
/// grows with the tiles' data (see
/// [`DecodeOptions::code_block_work_limit`](crate::DecodeOptions::code_block_work_limit)),
/// so a file of a few tens of kilobytes can ask for minutes of block
/// decoding under the fuzz build, all within the default bound.
///
/// Under the cap, the work of a tile is its blocks' sample-passes and
/// [`CAP_WORK_PER_SAMPLE`] for each sample of each component it
/// reconstructs, at the reduction, so that the cap also bounds the time an
/// image with little coded data takes.
///
/// Under the cap, the bound `WorkLimit::Default` sets is `cap`, whatever
/// the tiles' data, and `Raised(n)` sets `n` times that, so a
/// refusal's `retry_with` names the factor its retry needs, as without the
/// cap. No limit sets more than [`CODE_BLOCK_CAP_FACTOR`] times `cap`:
/// `Unlimited` sets that bound too. Each decode is held to its bound by
/// itself; an incremental decoder holds all its refines to it together. A
/// file past the bound is refused with an
/// [`Error::Limit`](crate::Error::Limit) of
/// [`LimitKind::CodeBlockWork`](crate::LimitKind::CodeBlockWork) before any
/// of its blocks are decoded.
#[must_use]
pub fn with_code_block_work_cap(options: crate::DecodeOptions, cap: u64) -> crate::DecodeOptions {
    crate::DecodeOptions {
        code_block_work_cap: Some(cap),
        ..options
    }
}

/// Check that the walk of the tile-parts that keeps no tile record, which
/// `read_header_with` runs where it refuses a tile, gives the error of the
/// decode's parse of the tile-parts, with `strict` on and off, for the
/// codestream `data` holds (raw, or in a JP2 file's `jp2c` box), and on up
/// to 15 of its prefixes that end in the tile-parts. Panics where the two
/// differ. Input whose main header does not parse is not checked.
pub fn tile_parts_walks_agree(data: &[u8]) {
    use crate::codestream;
    let codestream = if !data.starts_with(&[0xFF, 0x4F]) && crate::jp2::looks_like_jp2(data) {
        match crate::jp2::parse(data, &crate::WorkLimit::default()) {
            Ok(file) => file.codestream,
            Err(_) => return,
        }
    } else {
        data
    };
    let Ok(parsed) = codestream::parse_header(codestream) else {
        return;
    };
    let lean_is_full = |bytes: &[u8], parsed: &codestream::ParsedHeader| {
        for strict in [true, false] {
            let lean = codestream::tile_parts_fault(bytes, parsed, strict);
            let full = codestream::parse_tiles_with(bytes, parsed.clone(), strict).err();
            assert_eq!(
                format!("{lean:?}"),
                format!("{full:?}"),
                "the walks differ on {} bytes with `strict` {strict}",
                bytes.len()
            );
        }
    };
    lean_is_full(codestream, &parsed);
    // The walk kept from call to call on prefixes that grow, as
    // `IncrementalDecoder::header` keeps it, gives on each what a walk from
    // the start gives, and so does a call with `strict` on after them. It
    // gives the main header it read from the prefix. The main header of a
    // prefix that holds the first `SOT` marker code reads as the whole
    // input's does, so only a prefix that ends before it is skipped.
    let main_header_len = parsed.main_header_len();
    let mut kept = None;
    let agree = |prefix: &[u8], strict: bool, kept: &mut Option<codestream::KeptWalk>| {
        let parsed = match codestream::parse_header(prefix) {
            Ok(parsed) => parsed,
            Err(_) if prefix.len() < main_header_len + 2 => return None,
            Err(error) => panic!(
                "the main header of {} bytes does not read: {error:?}",
                prefix.len()
            ),
        };
        let resumed = codestream::tile_parts_fault_kept(prefix, &parsed, strict, kept);
        let afresh = codestream::tile_parts_fault(prefix, &parsed, strict);
        assert_eq!(
            format!("{resumed:?}"),
            format!("{afresh:?}"),
            "the kept walk differs on {} bytes with `strict` {strict}",
            prefix.len()
        );
        Some(parsed)
    };
    // Each prefix is also held to the decode's parse, so the walks' handling
    // of input that ends early (a cut tile-part header, a `Psot` past the
    // end, a missing `EOC`) is checked on cut input made from whole seeds.
    let len = codestream.len();
    for cut in (1..=16).map(|k| main_header_len + (len - main_header_len) * k / 16) {
        let prefix = &codestream[..cut];
        let parsed = agree(prefix, false, &mut kept);
        if let (Some(parsed), true) = (parsed, cut < len) {
            lean_is_full(prefix, &parsed);
        }
    }
    agree(codestream, true, &mut kept);
    agree(codestream, false, &mut kept);
}

/// An [`IncrementalDecoder`](crate::IncrementalDecoder) as if its earlier
/// refines had spent `blocks` of the code-block work, so the
/// `incremental_decode` target reaches the cut that bound makes without
/// inputs large enough to spend it. Otherwise it is
/// [`IncrementalDecoder::new`](crate::IncrementalDecoder::new).
pub fn incremental_with_code_block_work_spent(
    options: crate::DecodeOptions,
    blocks: u64,
) -> crate::Result<crate::IncrementalDecoder> {
    crate::IncrementalDecoder::with_code_block_work_spent(options, blocks)
}

/// The code-block work `decoder`'s refines have spent, and the code-block
/// bound of the last decode a refine tried: `None` when that decode did not
/// get as far as the bound, or the options set none.
pub fn incremental_code_block_work(decoder: &crate::IncrementalDecoder) -> (u64, Option<u64>) {
    decoder.code_block_work()
}

/// How many packet bounds the walks of tiles not kept may spend over all
/// the refines of an incremental decoder before their bound refuses one.
pub const REPEAT_BOUNDS: u64 = crate::tier2::resume::REPEAT_BOUNDS;

/// What an [`IncrementalDecoder`](crate::IncrementalDecoder) has charged
/// for the walks that read a tile from its start again: the walks of tiles
/// not kept, the walks started or built again, and the first walk of a
/// tile whose walk was let go when the options changed. The first walk of
/// a tile kept from its start is not charged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatWork {
    /// The packet visits those walks have spent over all the refines.
    pub visits: u64,
    /// When their bound refused the last refine that tried, the packet
    /// bound of that refine's decode: the refusal comes once `visits` is
    /// past [`REPEAT_BOUNDS`] of it. Such a refine waits with
    /// [`Waiting::RepeatLimit`](crate::Waiting::RepeatLimit) where a
    /// one-shot decode of the same bytes may give an image or an error.
    pub refused: Option<u64>,
    /// How many tiles have a walk too large to keep.
    pub not_kept: usize,
}

/// What `decoder` has charged for the walks that read a tile again (see
/// [`RepeatWork`]).
pub fn incremental_repeat_work(decoder: &crate::IncrementalDecoder) -> RepeatWork {
    let (visits, refused, not_kept) = decoder.repeat_work();
    RepeatWork {
        visits,
        refused,
        not_kept,
    }
}

#[cfg(test)]
mod tests {
    /// The work a whole decode of `bytes` under `options` spends on its
    /// code-blocks, and the bound it had, read through an incremental
    /// decoder fed every byte.
    fn block_work(bytes: &[u8], options: &crate::DecodeOptions) -> (u64, Option<u64>) {
        let mut decoder = crate::IncrementalDecoder::new(options.clone()).unwrap();
        decoder.feed(bytes);
        let _ = decoder.refine();
        super::incremental_code_block_work(&decoder)
    }

    /// The cap charges the samples reconstructed, sets the bound `Default`
    /// sets, scales `Raised` from it so a refusal's retry names the factor
    /// that decodes, and holds `Unlimited` to
    /// [`super::CODE_BLOCK_CAP_FACTOR`] times the cap. Without it the
    /// options decode and print as before.
    #[test]
    fn the_code_block_cap_sets_the_bound_and_keeps_its_factors() {
        use super::{CODE_BLOCK_CAP_FACTOR as K, with_code_block_work_cap as cap};
        use crate::{DecodeOptions, Error, LimitKind, Raise, WorkLimit, decode_with};
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/layers/lrcp.j2k");
        let bytes = std::fs::read(path).unwrap();
        // The fuzz budgets are sized from these: a change must be measured.
        assert_eq!((K, super::CAP_WORK_PER_SAMPLE), (4, 4));
        let plain = DecodeOptions::default();
        let image = decode_with(&bytes, &plain).unwrap();
        let (blocks, bound) = block_work(&bytes, &plain);
        let default = bound.unwrap();
        // Under the cap each sample reconstructed is charged too.
        let samples: u64 = image
            .components
            .iter()
            .map(|c| c.samples.len() as u64)
            .sum();
        let (work, bound) = block_work(&bytes, &cap(plain.clone(), default));
        assert_eq!(bound, Some(default));
        assert_eq!(work, blocks + 4 * samples);
        assert!(blocks > 0 && work < default, "{work} of {default}");
        // `Unlimited` has no bound without the cap.
        let unlimited = plain
            .clone()
            .with_code_block_work_limit(WorkLimit::Unlimited);
        assert_eq!(block_work(&bytes, &unlimited).1, None);

        // `Default`: the cap.
        assert_eq!(
            block_work(&bytes, &cap(plain.clone(), work)),
            (work, Some(work))
        );
        assert_eq!(
            decode_with(&bytes, &cap(plain.clone(), work)).unwrap(),
            image
        );
        // Also past the default bound.
        let high = cap(plain.clone(), default + 1);
        assert_eq!(block_work(&bytes, &high).1, Some(default + 1));
        let below = cap(plain.clone(), work - 1);
        let Err(Error::Limit(limit)) = decode_with(&bytes, &below) else {
            panic!("a cap under the work refuses");
        };
        assert_eq!(limit.kind(), LimitKind::CodeBlockWork);
        assert_eq!(limit.retry_with(), Some(Raise::Raised(2)));
        let retry = below.raised_for(&limit).unwrap();
        assert_eq!(block_work(&bytes, &retry).1, Some(2 * (work - 1)));
        assert_eq!(decode_with(&bytes, &retry).unwrap(), image);

        // `Raised(K)` is K times the cap; `Unlimited` and higher factors stop
        // there.
        let quarter = work.div_ceil(K);
        for limit in [
            WorkLimit::Raised(K as u32),
            WorkLimit::Raised(1000),
            WorkLimit::Unlimited,
        ] {
            let options = cap(
                plain.clone().with_code_block_work_limit(limit.clone()),
                quarter,
            );
            assert_eq!(
                block_work(&bytes, &options).1,
                Some(K * quarter),
                "{limit:?}"
            );
            assert_eq!(decode_with(&bytes, &options).unwrap(), image, "{limit:?}");
            let short = cap(options, quarter - 1);
            assert!(
                matches!(decode_with(&bytes, &short), Err(Error::Limit(m)) if m.kind() == LimitKind::CodeBlockWork),
                "{limit:?}"
            );
        }

        // The cap prints only when it is set.
        assert!(!format!("{plain:?}").contains("cap"));
        assert!(format!("{:?}", cap(plain, 7)).ends_with(", code_block_work_cap: 7 }"));
    }

    /// A decoder started with work spent is cut by what is left of the
    /// bound: a refine whose work just fits decodes, and one more unit of
    /// work spent refuses it. The hooks report the work and the bound.
    #[test]
    fn incremental_hooks_cut_the_code_block_bound() {
        use crate::{DecodeOptions, decode_with};
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/layers/lrcp.j2k");
        let bytes = std::fs::read(path).unwrap();
        let start = |spent: u64| {
            let mut decoder =
                super::incremental_with_code_block_work_spent(DecodeOptions::default(), spent)
                    .unwrap();
            decoder.feed(&bytes);
            let refined = decoder.refine().unwrap().image();
            (refined, super::incremental_code_block_work(&decoder))
        };
        let (image, (work, bound)) = start(0);
        let bound = bound.unwrap();
        assert!(work > 0 && work < bound, "{work} of {bound}");
        let lenient = DecodeOptions::default().with_strict(false);
        assert_eq!(image.unwrap(), decode_with(&bytes, &lenient).unwrap());

        let (fits, (spent, _)) = start(bound - work);
        assert!(fits.is_some());
        assert_eq!(spent, bound);
        let (cut, (spent, seen)) = start(bound - work + 1);
        assert_eq!(cut, None);
        assert_eq!(seen, Some(bound));
        assert_eq!(spent, bound - work + 1, "a refused tile spends nothing");
    }

    /// A tile whose walk is started again at each tile-part, fed 64 bytes
    /// at a time: a refine the bound on those walks refuses reports that
    /// bound, with the visits past `REPEAT_BOUNDS` of it. A decoder fed all
    /// the bytes at once keeps the walk and charges nothing for it.
    #[test]
    fn incremental_repeat_hook_reports_the_refusal() {
        use crate::{DecodeOptions, IncrementalDecoder, Refined, Waiting};
        let bytes = crate::synth::restarted_poc_tile(4, 13, 400);
        let options = DecodeOptions::default().with_strict(false);
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        let mut refusals = 0;
        for end in (64..bytes.len()).step_by(64).chain([bytes.len()]) {
            decoder.feed(&bytes[decoder.bytes_fed()..end]);
            let refined = decoder.refine().unwrap();
            let work = super::incremental_repeat_work(&decoder);
            // The hook speaks of the last try, so a refine that did not try
            // (`MoreBytes`) is left out; on a try, the refine reports the
            // repeat bound exactly when the hook does.
            if !matches!(refined, Refined::Waiting(Waiting::MoreBytes { .. })) {
                assert_eq!(
                    matches!(refined, Refined::Waiting(Waiting::RepeatLimit { .. })),
                    work.refused.is_some(),
                    "{end}: {work:?}"
                );
            }
            if let Some(bound) = work.refused {
                assert!(work.visits > super::REPEAT_BOUNDS * bound, "{work:?}");
                refusals += 1;
            }
        }
        assert!(refusals > 0);
        let mut once = IncrementalDecoder::new(options).unwrap();
        once.feed(&bytes);
        assert!(once.refine().unwrap().image().is_some());
        let work = super::incremental_repeat_work(&once);
        assert_eq!(
            work,
            super::RepeatWork {
                visits: 0,
                refused: None,
                not_kept: 0
            }
        );
    }

    /// The `incremental_decode` target's check on a refine the bound on
    /// the walks read again refuses, over an input that reaches that bound
    /// fed as the target feeds it: a tile whose walk starts again at each
    /// tile-part, in pieces a hash of the file picks, one refine after
    /// each. A decoder fed each due prefix at once keeps the walk, so it
    /// charges nothing. Were the first walk of a kept tile charged, the
    /// fifth due refine would be refused, and the fresh decoder's charge
    /// would fail the check. The input is 16 KB, past
    /// the target's `-max_len=4096`, so the fuzzer does not reach it. Set
    /// `RUST_J2K_INCREMENTAL_SEED_DIR` to write it out as a seed (see
    /// `fuzz/README.md`).
    #[test]
    fn incremental_target_repeat_check_holds_on_a_restarted_walk() {
        use crate::{DecodeOptions, IncrementalDecoder};
        fn splitmix(x: u64) -> u64 {
            let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }
        let data = crate::synth::restarted_poc_tile(4, 8, 299);
        if let Some(dir) = std::env::var_os("RUST_J2K_INCREMENTAL_SEED_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("restarted_poc_tile_4_8_299.j2k"), &data).unwrap();
        }
        // The target's options for an input that starts with `SOC`: all
        // control bytes zero, `strict` off for the refines.
        let options = DecodeOptions::default().with_strict(false);
        let mut decoder = IncrementalDecoder::new(options.clone()).unwrap();
        // The target's schedule: FNV-1a of the file, then a splitmix step
        // for each piece and for each due refine with a code-block bound.
        let mut hash = data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        let (mut fed, mut tried) = (0, 0);
        let mut due = Vec::new();
        while fed < data.len() {
            hash = splitmix(hash);
            let piece = 1 + (hash % (data.len() as u64 / 4 + 1)) as usize;
            let end = (fed + piece).min(data.len());
            decoder.feed(&data[fed..end]);
            fed = end;
            decoder.refine().unwrap();
            if fed < tried * 2 {
                continue;
            }
            tried = fed;
            let work = super::incremental_repeat_work(&decoder);
            if let Some(bound) = work.refused {
                assert!(work.visits > super::REPEAT_BOUNDS * bound, "{work:?}");
            }
            if super::incremental_code_block_work(&decoder).1.is_some() {
                hash = splitmix(hash);
            }
            due.push(fed);
        }
        // The due refines past the main header that the target makes, the
        // last at the bytes where the charge of a kept walk refuses it.
        assert_eq!(due[due.len() - 5..], [168, 1568, 3167, 7677, 16007]);
        // The target asks a fresh decoder where the refine is refused; here
        // it is asked at the last due refine, which the overcharge refuses.
        let last = due[due.len() - 1];
        let mut fresh = IncrementalDecoder::new(options).unwrap();
        fresh.feed(&data[..last]);
        assert!(fresh.refine().unwrap().image().is_some());
        let once = super::incremental_repeat_work(&fresh);
        assert_eq!(once.refused, None);
        assert_eq!(once.not_kept, 0);
        assert_eq!(once.visits, 0, "{last} bytes: a kept walk was charged");
        // Each refine after the first walks the tile again from its start,
        // at about 0.8 of the packet bound each.
        let work = super::incremental_repeat_work(&decoder);
        assert_eq!(work.refused, None);
        assert!(work.visits > 3 * (1 << 24), "{work:?}");
    }

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

    /// Slice the code-blocks of an HT codestream into `ht_block` inputs, each
    /// with whether it must decode (the tall copies of a block carry bytes made
    /// for another shape and need not): the
    /// block's shape, passes and bytes, in the layout the hook reads. A seed's
    /// `Mb` is `P + 6`, which leaves planes to code.
    fn ht_seeds(name: &str) -> Vec<(Vec<u8>, bool)> {
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
                let Some(cleanup) = block.segments.first() else {
                    continue;
                };
                let cleanup = cleanup.bytes();
                let refinement = block.segments.get(1).map(|s| s.bytes()).unwrap_or_default();
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
                seed.extend(cleanup.iter());
                seed.extend(refinement.iter());
                // The same bytes in the tall shapes the corpus lacks.
                if seeds.len() % 64 == 0 {
                    for (w, h) in [(1u16, 1024u16), (4, 1024), (2, 1024), (8, 512)] {
                        let mut tall = seed.clone();
                        tall[..2].copy_from_slice(&(w - 1).to_le_bytes());
                        tall[2] = (h - 1) as u8;
                        tall[12] = ((h - 1) >> 8) as u8;
                        seeds.push((tall, false));
                    }
                }
                seeds.push((seed, true));
            }
        }
        seeds
    }

    /// A one-pass block of `width` x `height` over the cleanup segment
    /// `F2 00` (`Scup = 2`), which decodes at any shape.
    fn tall_seed(width: u16, height: u16) -> Vec<u8> {
        let mut seed = (width - 1).to_le_bytes().to_vec();
        seed.extend([(height - 1) as u8, 0, 8, 0, 1, 2, 0, 0, 0, 0]);
        seed.push(((height - 1) >> 8) as u8);
        seed.extend([0xF2, 0x00]);
        seed
    }

    /// The tall shapes decode: one seed for each.
    #[test]
    fn ht_block_tall_seeds_decode() {
        for (w, h) in [(1u16, 1024u16), (2, 1024), (3, 1024), (4, 1024), (8, 512)] {
            let (shape, decoded) = super::run_ht_block(&tall_seed(w, h)).unwrap();
            assert_eq!(shape, (usize::from(w), usize::from(h)));
            assert_eq!(decoded.unwrap(), usize::from(w) * usize::from(h), "{w}x{h}");
        }
    }

    /// The seeds sliced from the HT corpus decode through the hook, so the
    /// target starts from blocks it accepts. Set `RUST_J2K_HT_SEED_DIR` to
    /// write them out as the target's seed corpus (see `fuzz/README.md`).
    #[test]
    fn ht_block_seeds_from_the_corpus_decode() {
        let dir = std::env::var_os("RUST_J2K_HT_SEED_DIR").map(std::path::PathBuf::from);
        let (mut total, mut refined) = (0usize, 0usize);
        for name in HT_SEED_FILES {
            for (i, (seed, must_decode)) in ht_seeds(name).iter().enumerate() {
                let width = 1 + usize::from(u16::from_le_bytes([seed[0], seed[1]]));
                let height = 1 + usize::from(u16::from_le_bytes([seed[2], seed[12]]));
                let (shape, decoded) = super::run_ht_block(seed).expect("a full header");
                assert_eq!(shape, (width, height));
                if *must_decode {
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
        if let Some(dir) = &dir {
            for (w, h) in [(1, 1024), (2, 1024), (3, 1024), (4, 1024), (8, 512)] {
                std::fs::write(dir.join(format!("tall_{w}x{h}")), tall_seed(w, h)).unwrap();
            }
        }
        if total > 0 {
            assert!(refined > 0, "no seed runs a refinement pass");
        }
    }
}
