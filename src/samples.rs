//! The sample limit: how many samples a decode may hold for its output, the
//! [`DecodeOptions::sample_limit`](crate::DecodeOptions::sample_limit) option.
//!
//! Every component is reconstructed into its own buffer, so the cost is the
//! *sum* of the component areas at the decode's resolution reduction, not the
//! image area. A palette's expanded channels and sYCC or gray-to-RGB output are
//! held to the same limit. Each is checked from SIZ and the JP2 boxes alone,
//! before any tile is read or plane allocated, so a small file that declares a
//! huge image is refused at the cost of reading its header.
//!
//! The default limit, [`MAX_IMAGE_SAMPLES`], is 2^28 samples: 1 GiB as `i32`.
//! It admits a Sentinel-2 10 m band (10980 x 10980, 120.6 M samples) and an
//! RGB image of up to about 89 MP. OpenJPEG has no such limit; it allocates
//! what the header declares.
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
//!
//! For a single tile the peak comes at the finest wavelet level, which holds
//! that level's subbands while it builds the merged grid. Those buffers come
//! to 1.25 times the output, as fresh zeroed memory is mapped only as it is
//! written; the rest of the process adds about 0.1 times, so a single tile
//! peaks at about 1.35 times the output. The empty 5/3 file is lower only
//! because its subbands are never written. The coded-data figures include
//! the caller's input bytes, so they grow with the file: a 38 MB lossless
//! 8192 x 8192 file peaks at about 380 MiB. The ratios carry over to the
//! 2^28 default, about 1.4 GiB for one component in one tile, though that size
//! was not measured. A tiled image peaks lower: the 1024 x 1024-tiled
//! Sentinel-2 band (460 MiB of samples) peaks at about 470 MiB, as
//! `opj_decompress` does (see `docs/correctness.md`, "The sample limit").

use crate::{Error, WorkLimit};

/// The default sample limit: 2^28 samples, 1 GiB as `i32`.
pub(crate) const MAX_IMAGE_SAMPLES: u64 = 1 << 28;

/// The most samples one buffer can hold on this target. Every sample buffer
/// holds 4-byte samples (`i32` or `f32`), and an allocation may not pass
/// `isize::MAX` bytes, so no limit, not even [`WorkLimit::Unlimited`], goes
/// past this: past it the buffer's size could not be computed, let alone
/// allocated. It is 2^29 - 1 on a 32-bit target, and 2^61 - 1 on a 64-bit one.
pub(crate) const ADDRESSABLE_SAMPLES: u64 = isize::MAX as u64 / 4;

/// The samples a decode may hold under `limit`.
pub(crate) fn cap(limit: &WorkLimit) -> u64 {
    limit.apply(MAX_IMAGE_SAMPLES).min(ADDRESSABLE_SAMPLES)
}

/// The [`Error::Limit`] for `what` (a description of what is over the limit,
/// such as "image area 10980×10980"), which needs `total` samples where `cap`
/// are allowed. It names the option, and the smallest
/// [`WorkLimit::Raised`] factor that admits `total` (or, past any factor, only
/// [`WorkLimit::Unlimited`]), or says that the target cannot address that many.
pub(crate) fn over(what: &str, total: u64, cap: u64) -> Error {
    if total > ADDRESSABLE_SAMPLES {
        return Error::Limit(format!(
            "{what} needs {total} samples, more than this target can address \
             ({ADDRESSABLE_SAMPLES})"
        ));
    }
    let over = format!(
        "{what} needs {total} samples, over the sample limit of {cap} (see \
         `DecodeOptions::sample_limit`)"
    );
    // Past `u32::MAX` times the default no `Raised` factor admits it.
    match u32::try_from(total.div_ceil(MAX_IMAGE_SAMPLES)) {
        Ok(factor) => Error::Limit(format!(
            "{over}; it needs at least `WorkLimit::Raised({factor})`, or use \
             `WorkLimit::Unlimited` for trusted input"
        )),
        Err(_) => Error::Limit(format!(
            "{over}; only `WorkLimit::Unlimited` admits it, for trusted input"
        )),
    }
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
    cap: u64,
) -> crate::Result<()> {
    // The image area, `(Xsiz - XOsiz)·(Ysiz - YOsiz)` reduced, not `Xsiz·Ysiz`,
    // so a large origin cannot inflate the count of an image that reconstructs
    // to a small canvas.
    let (width, height) = siz.image_extent_at(reduction);
    let area = u64::from(width) * u64::from(height);
    if area > cap {
        let what = format!("image area {width}×{height}{}", at(reduction));
        return Err(over(&what, area, cap));
    }
    let mut total: u64 = 0;
    for index in 0..siz.components.len() {
        let (width, height) = siz
            .component_extent_at(index, reduction)
            .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {index}")))?;
        total = total.saturating_add(u64::from(width) * u64::from(height));
    }
    if total > cap {
        let what = format!("{} components{}", siz.components.len(), at(reduction));
        return Err(over(&what, total, cap));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(total: u64) -> String {
        match over("image area", total, MAX_IMAGE_SAMPLES) {
            Error::Limit(m) => m,
            other => panic!("{other:?}"),
        }
    }

    /// The hint names the smallest factor that admits the image, up to the
    /// largest factor there is.
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
    /// address, the hint names only `Unlimited`. A 2^60-sample image, such as
    /// GDAL's `dimensions_above_31bit.jp2` at reduction 2, is one.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn past_every_factor_the_hint_names_only_unlimited() {
        let largest = u64::from(u32::MAX) * MAX_IMAGE_SAMPLES;
        for total in [largest + 1, 1 << 60] {
            let m = message(total);
            assert!(!m.contains("Raised"), "{m}");
            assert!(m.contains("only `WorkLimit::Unlimited` admits it"), "{m}");
        }
    }
}
