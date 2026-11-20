//! Keeps a fuzz run under libFuzzer's RSS limit and its timeout.
//!
//! Two budgets: [`over`] skips an input that declares too many samples,
//! which bounds memory, and [`options`] caps the work its decodes may do,
//! which bounds time. Each target divides each budget by a weight of its
//! own, pinned by `tests/fuzz_seed_outcomes.rs`.

use rust_j2k::{DecodeOptions, WorkLimit};

/// The most samples a fuzz input may declare to a target that holds one
/// image at a time: 2^28 (1 GiB as `i32`). A decode allocates its output
/// before it decodes a tile, and the work cap ([`options`]) refuses a tile
/// before reconstructing it, so a decode at this budget allocates its output
/// and little more. The work cap lets a target's decode reconstruct at most
/// 2^25 samples (`window_crop`'s), an eighth of this, and that peaks at
/// about 1.35 times its samples for a square tile and up to about 3.2 times
/// for a tile one sample wide. A target that
/// holds several images at once divides the budget by its `weight` (see
/// [`over`]). The decoder's default sample limit is at least 2^29, so a
/// sample limit refusal of the image area is never decoded.
pub const MAX_FUZZ_SAMPLES: u64 = 1 << 28;

/// The most work, in sample-passes, the decodes of one execution may spend
/// between them: 2^28. A tile's work under the fuzz cap is its code-blocks'
/// sample-passes and `rust_j2k::fuzz::CAP_WORK_PER_SAMPLE` for each sample
/// it reconstructs (`rust_j2k::fuzz::with_code_block_work_cap`). Under the
/// fuzz build (ASan) a sample-pass of the slowest block data takes about
/// 27 ns, so this is about 7 s, under the 30 s `-timeout` the fuzz job
/// passes with room for a loaded machine.
pub const MAX_FUZZ_WORK: u64 = 1 << 28;

/// The default options with the code-block work bound set to [`MAX_FUZZ_WORK`]
/// divided by `weight`: the most code-block bounds' worth of work one
/// execution can spend, counting a decode at `WorkLimit::Raised(n)` as `n`
/// and one at `Unlimited` as `rust_j2k::fuzz::CODE_BLOCK_CAP_FACTOR`, the
/// most the cap lets it spend. Every decode a budgeted target runs starts
/// from these. An input past the cap is refused with `Error::Limit` before
/// the work is done, which is still a path worth fuzzing.
#[allow(dead_code)]
pub fn options(weight: u64) -> DecodeOptions {
    rust_j2k::fuzz::with_code_block_work_cap(DecodeOptions::default(), cap(weight))
}

/// The code-block work cap [`options`] sets for a target of `weight`.
#[allow(dead_code)]
pub fn cap(weight: u64) -> u64 {
    MAX_FUZZ_WORK / weight
}

/// Whether `data` declares more output samples than the budget allows a
/// target of `weight` (the most images it holds at once, a power of two;
/// the work cap bounds how many decodes it runs). The budget is
/// [`MAX_FUZZ_SAMPLES`] divided by `weight`, at full resolution, which bounds
/// every window, tile and reduction of it.
///
/// The count is the larger of the header's and the `SIZ` segment's. The
/// header lists output channels, which a palette makes fewer than the
/// components the decoder allocates, and a JP2 whose `ihdr` box disagrees with
/// `SIZ` does not read at all, yet a lenient decode goes on to allocate from
/// `SIZ`. An input with no readable `SIZ` fails cheaply and is not over.
#[allow(dead_code)]
pub fn over(data: &[u8], weight: u64) -> bool {
    declared(data) > u128::from(samples(weight))
}

/// The most samples [`over`] allows a target of `weight`.
#[allow(dead_code)]
pub fn samples(weight: u64) -> u64 {
    MAX_FUZZ_SAMPLES / weight
}

/// The samples `data` declares, counted as [`over`] counts them, before the budget.
pub fn declared(data: &[u8]) -> u128 {
    let options = DecodeOptions::default().with_sample_limit(WorkLimit::Unlimited);
    let header = rust_j2k::read_header_with(data, &options)
        .map(|header| {
            header
                .components
                .iter()
                .map(|c| u128::from(c.width) * u128::from(c.height))
                .sum::<u128>()
        })
        .unwrap_or(0);
    header.max(siz_samples(data))
}

/// The most samples any `SOC` + `SIZ` pair in `data` declares, read straight
/// from the bytes. A pair counts only if its `Lsiz` is `38 + 3 * Csiz` and
/// `Csiz <= 16384`, the decoder's own rules, so a run of repeated markers
/// costs a few byte reads per position. The scan takes every such pair, so one
/// inside a box that is not the codestream only makes the count larger.
fn siz_samples(data: &[u8]) -> u128 {
    const PAIR: [u8; 4] = [0xFF, 0x4F, 0xFF, 0x51];
    let be32 = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
    };
    data.windows(PAIR.len())
        .enumerate()
        .filter(|(_, w)| *w == PAIR)
        .filter_map(|(soc, _)| {
            // Offsets from SOC: SIZ marker 2, Lsiz 4, Rsiz 6, Xsiz 8, Ysiz 12,
            // XOsiz 16, YOsiz 20, tile fields 24..40, Csiz 40, components 42.
            let (xsiz, ysiz, xo, yo) = (
                be32(soc + 8)?,
                be32(soc + 12)?,
                be32(soc + 16)?,
                be32(soc + 20)?,
            );
            let lsiz = data.get(soc + 4..soc + 6)?;
            let csiz = data.get(soc + 40..soc + 42)?;
            let lsiz = usize::from(u16::from_be_bytes([lsiz[0], lsiz[1]]));
            let csiz = usize::from(u16::from_be_bytes([csiz[0], csiz[1]]));
            if csiz > 16384 || lsiz != 38 + 3 * csiz {
                return None;
            }
            let mut total = 0u128;
            for c in 0..csiz {
                let Some(&[_, xr, yr]) = data.get(soc + 42 + 3 * c..soc + 45 + 3 * c) else {
                    break;
                };
                let (xr, yr) = (u64::from(xr), u64::from(yr));
                if xr == 0 || yr == 0 {
                    continue;
                }
                let w = xsiz.div_ceil(xr).saturating_sub(xo.div_ceil(xr));
                let h = ysiz.div_ceil(yr).saturating_sub(yo.div_ceil(yr));
                total += u128::from(w) * u128::from(h);
            }
            Some(total)
        })
        .max()
        .unwrap_or(0)
}
