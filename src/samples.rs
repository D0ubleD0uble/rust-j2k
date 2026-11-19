//! The sample limit: how many samples a decode may hold for its output, the
//! [`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit) option.
//!
//! Every component is reconstructed into its own buffer, so the cost is the
//! *sum* of the component areas at the decode's resolution reduction, not the
//! image area. A palette's expanded channels and sYCC or gray-to-RGB output are
//! held to the same limit. Each is checked from SIZ and the JP2 boxes alone,
//! before any tile is read or plane allocated, so a small file that declares a
//! huge image is refused at the cost of reading its header. Under a window or
//! a tile, the codestream's count is that of the tiles the decode reads
//! ([`check_read`]), and a palette's or sRGB output's count is that of the
//! returned area.
//!
//! The default limit ([`default_limit`]) is the larger of
//! [`MAX_IMAGE_SAMPLES`], 2^29 samples (2 GiB as `i32`), and
//! [`SAMPLES_PER_BYTE`], 8 samples per byte of input. Up to 64 MiB of input it
//! is 2^29, which admits a Sentinel-2 true-color image (10980 x 10980 x 3,
//! 361.7 M samples) and an RGB image of up to about 179 MP. Past that it
//! grows with the input, so a large file may ask for at most 32 bytes of
//! `i32` output per byte of input, and a small one is held to the floor.
//! The input is all the bytes the decode is given: a JP2 file's boxes
//! count. OpenJPEG has no such limit; it allocates what the header declares.
//!
//! While a palette expands, the index plane and the columns are held together,
//! so the peak there is about twice the output: a one-column palette at
//! 2^26 samples (8192 x 8192) peaks at about 515 MiB, on 5/3 and 9/7 alike.
//!
//! The decode's peak memory is above the output's size. Measured peak resident
//! memory at 2^26 samples (release build, Linux, glibc), for the empty-data
//! files that `scripts/gen-peak-memory-shapes.py` writes and, for one tile, for
//! an 8192 x 8192 image coded by `opj_compress` (lossless, and `-I -r 20`):
//!
//! | Shape                           | Empty data | Coded data |
//! |---------------------------------|------------|------------|
//! | 1 component, one tile, 9/7      | ~340 MiB   | ~347 MiB   |
//! | 1 component, one tile, 5/3      | ~260 MiB   | ~351 MiB   |
//! | 3 components, 9/7 with MCT      | ~316 MiB   |            |
//! | palette to 3 columns            | ~344 MiB   |            |
//! | sYCC 4:2:0 to sRGB              | ~395 MiB   |            |
//! | one tile 1024 x 65536, 9/7      | ~390 MiB   |            |
//! | one tile 64 x 2^20, 9/7         | ~409 MiB   |            |
//! | one tile 16 x 2^22, 9/7         | ~409 MiB   |            |
//! | one tile 8 x 2^23, 9/7          | ~429 MiB   |            |
//! | one tile 8 x 2^23, 5/3          | ~333 MiB   |            |
//!
//! For a single tile the peak comes at the finest wavelet level, which holds
//! that level's subbands while it builds the merged grid. Those buffers come
//! to 1.25 times the output, as fresh zeroed memory is mapped only as it is
//! written; the rest of the process adds about 0.1 times, so a square tile
//! peaks at about 1.35 times the output. The empty 5/3 file is lower only
//! because its subbands are never written.
//!
//! A tall, narrow tile peaks higher: about 1.5 to 1.7 times the output for the
//! 9/7 tiles 8 to 1024 samples wide above. Narrower than 16 samples, the
//! wavelet's column pass takes one column at a time through a buffer as tall
//! as the tile, and that buffer is a larger share of the tile the narrower it
//! is. At 2^24 samples (64 MiB of output), 3 wavelet levels and 64 x 64
//! code-blocks, a 9/7 tile 2 samples wide peaks at about 120 MiB (1.9 times)
//! and one 1 sample wide at about 165 MiB (2.6 times). More levels add
//! code-blocks, and their fixed overhead is a larger share of a small image:
//! at 5 levels the same tiles peak at about 2.1 and 2.7 times, and at 8 levels
//! the 1-wide tile at about 3.2 times. The 8- to 1024-wide figures in the
//! table are at 5 levels. The coded-data figures include
//! the caller's input bytes, so they grow with the file: a 38 MB lossless
//! 8192 x 8192 file peaks at about 380 MiB. The ratios carry over to the
//! 2^29 floor, about 2.7 GiB for one component in one tile: a 23170 x 23170
//! one-tile 5/3 image (lossless, 286 MiB of input) peaks at 2.9 GiB, about 1.3
//! times its 2 GiB of output plus the input. A tiled image peaks lower: the
//! 1024 x 1024-tiled Sentinel-2 band (460 MiB of samples) peaks at about 470 MiB, as
//! `opj_decompress` does (see `docs/resource-bounds.md`, "The sample limit").

use crate::{Error, WorkLimit};

/// The floor of the default sample limit: 2^29 samples, 2 GiB as `i32`.
pub(crate) const MAX_IMAGE_SAMPLES: u64 = 1 << 29;

/// The samples the default limit allows per byte of input, once that comes
/// to more than [`MAX_IMAGE_SAMPLES`]: past 64 MiB of input. The large valid
/// files measured hold far fewer: a Sentinel-2 true-color image about 2.9
/// samples a byte, and a lossless 23170 x 23170 gradient about 1.8. A file
/// over the floor that holds more samples a byte than this needs a raised
/// limit.
pub(crate) const SAMPLES_PER_BYTE: u64 = 8;

/// The most samples one buffer can hold on this target. Every sample buffer
/// holds 4-byte samples (`i32` or `f32`), and an allocation may not pass
/// `isize::MAX` bytes, so no limit, not even [`WorkLimit::Unlimited`], goes
/// past this: past it the buffer's size could not be computed, let alone
/// allocated. It is 2^29 - 1 on a 32-bit target, and 2^61 - 1 on a 64-bit one.
pub(crate) const ADDRESSABLE_SAMPLES: u64 = isize::MAX as u64 / 4;

/// The default sample limit for `bytes` bytes of input: the larger of
/// [`MAX_IMAGE_SAMPLES`] and [`SAMPLES_PER_BYTE`] samples a byte.
pub(crate) fn default_limit(bytes: u64) -> u64 {
    MAX_IMAGE_SAMPLES.max(bytes.saturating_mul(SAMPLES_PER_BYTE))
}

/// The sample limit of one decode: the samples it may hold, and the default
/// limit for its input, which a `Raised` factor multiplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cap {
    /// The samples the decode may hold.
    pub(crate) allowed: u64,
    /// The default limit for the decode's input ([`default_limit`]).
    pub(crate) default: u64,
    /// The `n` of a [`WorkLimit::AtMost`] limit: a ceiling the caller set,
    /// which a refusal names, with no retry. `None` for the other limits.
    pub(crate) at_most: Option<u64>,
}

impl Cap {
    /// This cap with no limit on the count, for a plan that must count
    /// nothing.
    pub(crate) fn lifted(self) -> Cap {
        Cap {
            allowed: u64::MAX,
            ..self
        }
    }
}

/// The samples a decode of `bytes` bytes of input may hold under `limit`:
/// `limit` applied to [`default_limit`], or the `n` of `AtMost(n)` whatever
/// the input, and never past [`ADDRESSABLE_SAMPLES`].
pub(crate) fn cap(limit: &WorkLimit, bytes: usize) -> Cap {
    let default = default_limit(bytes as u64);
    let at_most = match limit {
        WorkLimit::AtMost(n) => Some(*n),
        _ => None,
    };
    Cap {
        allowed: limit.apply(default).min(ADDRESSABLE_SAMPLES),
        default,
        at_most,
    }
}

/// The [`Error::Limit`] for `what` (a description of what is over the limit,
/// such as "image area 10980x10980"), which needs `total` samples where `cap`
/// allows fewer. It names the option, and the smallest
/// [`WorkLimit::Raised`] factor of the cap's default that admits `total` (or,
/// past any factor, only [`WorkLimit::Unlimited`]), or says that the target
/// cannot address that many. Under [`WorkLimit::AtMost`] it names that
/// ceiling and no retry: the caller set it, so no option the decoder could
/// name is one the caller would take.
pub(crate) fn over(what: &str, total: u64, cap: Cap) -> Error {
    if total > ADDRESSABLE_SAMPLES {
        return Error::limit_fixed(
            crate::LimitKind::AddressSpace,
            format!(
                "{what} needs {total} samples, more than this target can address \
             ({ADDRESSABLE_SAMPLES})"
            ),
        );
    }
    if let Some(n) = cap.at_most {
        return Error::limit_unhinted(
            crate::LimitKind::Samples,
            format!(
                "{what} needs {total} samples, over the ceiling of `WorkLimit::AtMost({n})` \
                 (see `DecodeOptions::sample_limit`)"
            ),
        );
    }
    let (retry, advice) = crate::limit_hint::samples(total, cap.default);
    Error::limit_retry(
        crate::LimitKind::Samples,
        retry,
        format!(
            "{what} needs {total} samples, over the sample limit of {} (see \
             `DecodeOptions::sample_limit`); {advice}",
            cap.allowed
        ),
    )
}

/// " at resolution reduction `reduction`", or nothing at full resolution:
/// the tail of a message from [`over`].
pub(crate) fn at(reduction: u8) -> String {
    if reduction == 0 {
        String::new()
    } else {
        format!(" at resolution reduction {reduction}")
    }
}

/// Hold the codestream's components, at `reduction`, to `cap` samples.
///
/// The image area is checked first: it is the cheap answer for the common
/// case. Then the components' own extents are summed, as each has its own
/// plane; `Csiz` reaches 16384, so many components of a modest image can
/// still ask for far more than its area.
pub(crate) fn check_codestream(
    siz: &crate::codestream::markers::Siz,
    reduction: u8,
    cap: Cap,
) -> crate::Result<()> {
    check_extent(siz, reduction, cap, false)
}

/// Hold a decode of `selection` to `cap` samples: [`check_codestream`] when
/// it returns the whole image. Under a window or a tile, the
/// count is every component of the tiles the decode reads, at `reduction`,
/// the area [`Selection::read_rect`](crate::window::Selection::read_rect)
/// gives.
///
/// That is more than the decode returns, and it has to be. The returned
/// planes lie inside those tiles. Each tile read is reconstructed whole, in
/// buffers sized by the tile, for every component the decode returns and
/// the rest of a color transform's three. And the packet walk builds every
/// component's code-block geometry in each tile read, a component subset
/// or not; the code-blocks of the kept resolutions are charged to no work
/// bound, as they are no more than the samples counted here, and no more
/// than 2^20 a tile (`docs/resource-bounds.md`).
pub(crate) fn check_read(
    siz: &crate::codestream::markers::Siz,
    selection: &crate::window::Selection,
    reduction: u8,
    cap: Cap,
) -> crate::Result<()> {
    if selection.area.image_rect() == siz.image_rect() {
        return check_codestream(siz, reduction, cap);
    }
    check_extent(
        &siz.windowed(selection.read_rect(siz)),
        reduction,
        cap,
        true,
    )
}

/// [`check_codestream`] of the image area `siz` declares, which is the area
/// of the tiles a decode reads when `tiles` is set (the message says which).
fn check_extent(
    siz: &crate::codestream::markers::Siz,
    reduction: u8,
    cap: Cap,
    tiles: bool,
) -> crate::Result<()> {
    // The image area, `(Xsiz - XOsiz)·(Ysiz - YOsiz)` reduced, not `Xsiz·Ysiz`,
    // so a large origin cannot inflate the count of an image that reconstructs
    // to a small canvas.
    let (width, height) = siz.image_extent_at(reduction);
    let samples = u64::from(width) * u64::from(height);
    if samples > cap.allowed {
        let what = if tiles {
            format!(
                "the area of the tiles read ({width}x{height}{})",
                at(reduction)
            )
        } else {
            format!("image area {width}x{height}{}", at(reduction))
        };
        return Err(over(&what, samples, cap));
    }
    let mut total: u64 = 0;
    for index in 0..siz.components.len() {
        let (width, height) = siz
            .component_extent_at(index, reduction)
            .ok_or_else(|| Error::inconsistent(format!("`SIZ` declares no component {index}")))?;
        total = total.saturating_add(u64::from(width) * u64::from(height));
    }
    if total > cap.allowed {
        let what = format!(
            "a decode of {}{} components{}",
            if tiles { "the tiles read in " } else { "" },
            siz.components.len(),
            at(reduction)
        );
        return Err(over(&what, total, cap));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default cap of an input under 64 MiB: the floor.
    const DEFAULT: Cap = Cap {
        allowed: MAX_IMAGE_SAMPLES,
        default: MAX_IMAGE_SAMPLES,
        at_most: None,
    };

    fn message(total: u64) -> String {
        match over("image area", total, DEFAULT) {
            Error::Limit(m) => m.message().to_owned(),
            other => panic!("{other:?}"),
        }
    }

    /// The hint names the smallest factor that admits the image, up to the
    /// largest factor there is.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_hint_names_the_smallest_factor() {
        assert!(message(MAX_IMAGE_SAMPLES + 1).contains("`WorkLimit::Raised(2)`"));
        let largest = u64::from(u32::MAX) * MAX_IMAGE_SAMPLES;
        if largest <= ADDRESSABLE_SAMPLES {
            let m = message(largest);
            assert!(
                m.contains(&format!("`WorkLimit::Raised({})`", u32::MAX)),
                "{m}"
            );
        }
    }

    /// Past what any `Raised` factor admits, but within what the target can
    /// address, the hint names only `Unlimited`. The largest image the target
    /// can address (2^61 - 1 samples on a 64-bit one) is one.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn past_every_factor_the_hint_names_only_unlimited() {
        let largest = u64::from(u32::MAX) * MAX_IMAGE_SAMPLES;
        for total in [largest + 1, ADDRESSABLE_SAMPLES] {
            let m = message(total);
            assert!(!m.contains("Raised"), "{m}");
            assert!(m.contains("only `WorkLimit::Unlimited` admits it"), "{m}");
            let Error::Limit(limit) = over("image area", total, DEFAULT) else {
                unreachable!()
            };
            assert_eq!(limit.kind(), crate::LimitKind::Samples);
            assert_eq!(limit.retry_with(), Some(WorkLimit::Unlimited));
        }
    }

    /// The hint is data as well as words: the kind and the factor. A multiple
    /// of 2^29 is past a 32-bit target's address cap.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_hint_is_typed() {
        let Error::Limit(limit) = over("image area", MAX_IMAGE_SAMPLES * 3, DEFAULT) else {
            unreachable!()
        };
        assert_eq!(limit.kind(), crate::LimitKind::Samples);
        assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(3)));
    }

    /// Past what the target can address, no option admits the image, so the
    /// refusal gives nothing to retry with.
    #[test]
    fn past_the_address_space_there_is_no_retry() {
        let error = over("image area", ADDRESSABLE_SAMPLES + 1, DEFAULT);
        assert!(
            crate::error::is_fixed_limit(&error, crate::LimitKind::AddressSpace),
            "{error:?}"
        );
    }

    /// The default is the floor up to 64 MiB of input, and 8 samples a byte
    /// past it.
    #[test]
    fn the_default_scales_with_the_input_past_the_floor() {
        assert_eq!(cap(&WorkLimit::Default, 0), DEFAULT);
        assert_eq!(cap(&WorkLimit::Default, 100), DEFAULT);
        assert_eq!(cap(&WorkLimit::Default, 1 << 26), DEFAULT);
        let past = (1 << 26) + 1;
        let scaled = cap(&WorkLimit::Default, past);
        assert_eq!(scaled.default, 8 * past as u64);
        assert_eq!(scaled.allowed, 8 * past as u64);
    }

    /// `Raised(n)` is n times the scaled default, so it is never below
    /// `Default` and grows with n; `Unlimited` lifts it to the address cap.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn raised_multiplies_the_scaled_default() {
        let bytes = 1 << 27;
        let default = cap(&WorkLimit::Default, bytes);
        assert_eq!(default.allowed, 1 << 30);
        for n in 1..=4 {
            let raised = cap(&WorkLimit::Raised(n), bytes);
            assert_eq!(raised.default, default.default);
            assert_eq!(raised.allowed, u64::from(n) << 30);
        }
        assert_eq!(
            cap(&WorkLimit::Unlimited, bytes).allowed,
            ADDRESSABLE_SAMPLES
        );
    }

    /// However many bytes there are, the cap stays within what the target
    /// can address: on a 32-bit target 2^29 - 1, below the floor itself.
    #[test]
    fn the_scaled_default_never_passes_the_address_space() {
        for limit in [
            WorkLimit::Default,
            WorkLimit::Raised(u32::MAX),
            WorkLimit::Unlimited,
        ] {
            assert_eq!(
                cap(&limit, usize::MAX).allowed,
                ADDRESSABLE_SAMPLES,
                "{limit:?}"
            );
        }
    }

    /// Under a scaled default the hint is still the smallest factor that
    /// admits the count: the factor named admits it, and one less does not.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_hint_is_exact_under_a_scaled_default() {
        let bytes = 100 << 20;
        let default = cap(&WorkLimit::Default, bytes);
        for total in [
            default.allowed + 1,
            3 * default.allowed,
            5 * default.allowed - 1,
        ] {
            let Error::Limit(limit) = over("image area", total, default) else {
                unreachable!()
            };
            let Some(WorkLimit::Raised(n)) = limit.retry_with() else {
                panic!("{limit:?}")
            };
            assert!(n >= 2, "{limit}");
            assert!(
                cap(&WorkLimit::Raised(n), bytes).allowed >= total,
                "{limit}"
            );
            assert!(
                cap(&WorkLimit::Raised(n - 1), bytes).allowed < total,
                "{limit}"
            );
            let limit_words = format!("over the sample limit of {} ", default.allowed);
            assert!(limit.message().contains(&limit_words), "{limit}");
        }
    }

    /// `AtMost(n)` allows `n` whatever the input's length: not a factor of the
    /// default, nor capped by it, and padding past 64 MiB does not raise it.
    /// Only the address cap is lower.
    #[test]
    fn at_most_is_absolute() {
        for bytes in [0, 1 << 26, (1 << 26) + 1, usize::MAX] {
            for n in [1, 1000, MAX_IMAGE_SAMPLES - 1, MAX_IMAGE_SAMPLES + 1] {
                let n = n.min(ADDRESSABLE_SAMPLES);
                let cap = cap(&WorkLimit::AtMost(n), bytes);
                assert_eq!(cap.allowed, n, "AtMost({n}), {bytes} bytes");
                assert_eq!(cap.at_most, Some(n), "AtMost({n}), {bytes} bytes");
                assert_eq!(cap.default, default_limit(bytes as u64));
            }
            assert_eq!(
                cap(&WorkLimit::AtMost(u64::MAX), bytes).allowed,
                ADDRESSABLE_SAMPLES
            );
        }
        for limit in [
            WorkLimit::Default,
            WorkLimit::Raised(2),
            WorkLimit::Unlimited,
        ] {
            assert_eq!(cap(&limit, 0).at_most, None, "{limit:?}");
        }
    }

    /// A refusal under `AtMost(n)` names the ceiling and the count, and no
    /// factor: it has nothing to retry with.
    #[test]
    fn a_refusal_under_at_most_names_the_ceiling_and_no_retry() {
        let n = 1 << 20;
        for bytes in [0, (1 << 26) + 1] {
            let Error::Limit(limit) = over(
                "image area 2048x1024",
                1 << 21,
                cap(&WorkLimit::AtMost(n), bytes),
            ) else {
                unreachable!()
            };
            assert_eq!(limit.kind(), crate::LimitKind::Samples);
            assert_eq!(limit.retry_with(), None, "{limit}");
            let m = limit.message();
            assert!(
                m.starts_with(
                    "image area 2048x1024 needs 2097152 samples, over the ceiling of \
                     `WorkLimit::AtMost(1048576)` (see `DecodeOptions::sample_limit`)"
                ),
                "{m}"
            );
            assert!(!m.contains("Raised"), "{m}");
            assert!(!m.contains("Unlimited"), "{m}");
        }
        // Past the address space the refusal is still that one.
        let error = over(
            "image area",
            ADDRESSABLE_SAMPLES + 1,
            cap(&WorkLimit::AtMost(n), 0),
        );
        assert!(
            crate::error::is_fixed_limit(&error, crate::LimitKind::AddressSpace),
            "{error:?}"
        );
    }

    /// A refusal under a raised limit names a factor of the default, not of
    /// the raised limit, so a retry with it makes progress.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_refusal_under_a_raised_limit_names_a_larger_factor() {
        let raised = cap(&WorkLimit::Raised(2), 0);
        let total = 3 * MAX_IMAGE_SAMPLES + 1;
        let Error::Limit(limit) = over("image area", total, raised) else {
            unreachable!()
        };
        assert_eq!(limit.retry_with(), Some(WorkLimit::Raised(4)), "{limit}");
        assert!(cap(&WorkLimit::Raised(4), 0).allowed >= total);
    }
}
