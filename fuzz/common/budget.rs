//! Keeps a fuzz run under libFuzzer's RSS limit and timeout.

use rust_j2k::{DecodeOptions, WorkLimit};

/// The most samples a fuzz input may declare to a target that decodes it
/// once and keeps one image: 2^26 (256 MiB as `i32`). A decode peaks at about
/// 1.35 times its output for a square tile and up to about 3.2 times for a
/// tile one sample wide, so that is under 1 GiB and well under the 4 GiB
/// `-rss_limit_mb` the fuzz job passes. The time is the tighter limit. Under
/// the fuzz build (ASan) a thin image of 2^26 samples takes about 9 s a
/// decode, and the job's `-timeout=30` counts the whole execution, so a
/// target that decodes one input several times, or holds several images,
/// divides the budget by its `weight` (see [`over`]). The decoder's default
/// limit is at least 2^29, so a sample limit refusal of the image area is
/// never decoded.
const MAX_FUZZ_SAMPLES: u64 = 1 << 26;

/// Whether `data` declares more output samples than the budget allows a
/// target of `weight` (the most decodes of the input one execution runs, or
/// images it holds at once, whichever is more; a power of two). The budget is
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
    declared(data) > u128::from(MAX_FUZZ_SAMPLES / weight)
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
