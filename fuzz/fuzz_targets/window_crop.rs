#![no_main]
//! Property target over [`rust_j2k::DecodeOptions::region`] and
//! [`rust_j2k::DecodeOptions::components`]: a window, a tile and a
//! component subset decode to the crop or selection of the whole decode at
//! the same resolution reduction.
//!
//! Eight control bytes pick the reduction (0 to 2), one of four modes (a
//! window, a tile, a component subset, or a subset through a window), and
//! the window, tile or subset; the rest is the file. So libFuzzer can keep a
//! file and try other areas of it by mutating the control bytes. An input
//! that starts with a codestream's `SOC` (`FF 4F`) or the JP2 signature box
//! is instead the whole file, with a hash of it as the control bytes, so
//! plain fixtures are valid seeds.
//!
//! The window and tile are worked out on the reference grid from the first
//! `SOC SIZ` in the file. That `SIZ` may not be the codestream's (a JP2 file
//! can hold those bytes in another box), so an input is skipped unless its
//! image area gives the whole decode's size and component sizes.
//!
//! When the whole decode gives an image, the part must too, unless its
//! options are refused as [`rust_j2k::Error::InvalidOptions`] (an area with
//! no sample of a component at the reduction, a component past the image's).
//! Its size, and each of its components, must equal the whole decode's, cut
//! to the area. A window may start one point before the image area's origin
//! (it is cut to the image area), so an offset image gets one that overlaps
//! its edge.

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/inconsistent.rs"]
mod inconsistent;
use inconsistent::checked;

#[path = "../common/budget.rs"]
mod budget;

use rust_j2k::{decode_with, Error, Image, Region, Window};

/// A rectangle on the reference grid, `x0..x1` by `y0..y1`.
type Rect = (u32, u32, u32, u32);

/// The SIZ fields of the first `SOC SIZ` in `data`:
/// `[Xsiz, Ysiz, XOsiz, YOsiz, XTsiz, YTsiz, XTOsiz, YTOsiz]`.
fn siz(data: &[u8]) -> Option<[u32; 8]> {
    let at = data
        .windows(4)
        .position(|w| w == [0xFF, 0x4F, 0xFF, 0x51])?;
    let s = data.get(at..at + 40)?;
    let word = |i: usize| u32::from_be_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]]);
    Some([
        word(8),
        word(12),
        word(16),
        word(20),
        word(24),
        word(28),
        word(32),
        word(36),
    ])
}

/// Reference-grid coordinate `v` on a component sub-sampled by `d`, at
/// reduction `r` (B-14 and B-15 with no tile split).
fn at(v: u32, d: u32, r: u8) -> u32 {
    v.div_ceil(d).div_ceil(1 << r)
}

/// `whole`'s component `c` inside the reference-grid rectangle `area`, cut
/// to the image area `image`, at reduction `r`: its width and samples.
fn crop(whole: &Image, c: usize, image: Rect, area: Rect, r: u8) -> (u32, Vec<i32>) {
    let p = &whole.components[c];
    let (dx, dy) = (u32::from(p.x_sampling), u32::from(p.y_sampling));
    let (x0, y0) = (area.0.max(image.0), area.1.max(image.1));
    let (x1, y1) = (area.2.min(image.2), area.3.min(image.3));
    let (ox, oy) = (at(image.0, dx, r), at(image.1, dy, r));
    let mut out = Vec::new();
    for y in at(y0, dy, r)..at(y1, dy, r) {
        for x in at(x0, dx, r)..at(x1, dx, r) {
            out.push(p.samples[((y - oy) * p.width + (x - ox)) as usize]);
        }
    }
    (at(x1, dx, r).saturating_sub(at(x0, dx, r)), out)
}

fuzz_target!(|input: &[u8]| {
    const JP2_SIGNATURE: [u8; 8] = [0, 0, 0, 12, b'j', b'P', b' ', b' '];
    let (control, data) = if input.starts_with(&[0xFF, 0x4F]) || input.starts_with(&JP2_SIGNATURE) {
        (fnv1a(input), input)
    } else {
        let Some((control, data)) = input.split_first_chunk::<8>() else {
            return;
        };
        (u64::from_le_bytes(*control), data)
    };
    let Some([xs, ys, xo, yo, xt, yt, xto, yto]) = siz(data) else {
        return;
    };
    if xs <= xo || ys <= yo || xt == 0 || yt == 0 || budget::over(data, 2) {
        return;
    }
    let image = (xo, yo, xs, ys);
    let r = ((control & 3) % 3) as u8;
    // Two decodes, the whole and the part, both held.
    let base = budget::options(2).with_resolution_reduction(r);
    let bits = control >> 8;
    let small = |shift: u32, m: u32| ((bits >> shift) as u32) % m;
    // 0: window, 1: tile, 2: component subset, 3: subset through a window.
    let mode = (control >> 2) & 3;
    let subset: Option<Vec<u16>> = match mode {
        2 | 3 => Some(match small(48, 4) {
            0 => vec![0],
            1 => vec![1],
            2 => vec![2, 0],
            _ => vec![1, 0],
        }),
        _ => None,
    };
    // A subset names codestream components, as raw channels do.
    let whole_options = if subset.is_some() {
        base.clone().with_raw_channels(true)
    } else {
        base.clone()
    };
    let Ok(whole) = checked(decode_with(data, &whole_options)) else {
        return;
    };
    let fits = whole.width == at(xs, 1, r) - at(xo, 1, r)
        && whole.height == at(ys, 1, r) - at(yo, 1, r)
        && whole.components.iter().all(|c| {
            let (dx, dy) = (u32::from(c.x_sampling), u32::from(c.y_sampling));
            c.width == at(xs, dx, r) - at(xo, dx, r) && c.height == at(ys, dy, r) - at(yo, dy, r)
        });
    // A SIZ the decoder accepts has its tile origin at or before the image's
    // (B-7), which the subtractions below need; the first `SOC SIZ` may not
    // be the one it read, so check.
    if !fits || xto > xo || yto > yo {
        return;
    }
    let mut options = base.with_components(subset.clone());
    let area = match mode {
        0 | 3 => {
            let (w, h) = (xs - xo, ys - yo);
            let x0 = xo
                .saturating_add(small(0, w.saturating_add(2)))
                .saturating_sub(1);
            let y0 = yo
                .saturating_add(small(12, h.saturating_add(2)))
                .saturating_sub(1);
            let x1 = x0.saturating_add(1 + small(24, w.min(96)));
            let y1 = y0.saturating_add(1 + small(36, h.min(96)));
            options = options.with_region(Region::Window(Window::new(x0, y0, x1, y1)));
            (x0, y0, x1, y1)
        }
        1 => {
            let across = (xs - xto).div_ceil(xt);
            let down = (ys - yto).div_ceil(yt);
            let t = small(0, across.saturating_mul(down).clamp(1, 16));
            let (p, q) = (t % across, t / across);
            options = options.with_region(Region::Tile(t));
            (
                xto.saturating_add(p.saturating_mul(xt)).max(xo),
                yto.saturating_add(q.saturating_mul(yt)).max(yo),
                xto.saturating_add((p + 1).saturating_mul(xt)).min(xs),
                yto.saturating_add((q + 1).saturating_mul(yt)).min(ys),
            )
        }
        _ => (0, 0, u32::MAX, u32::MAX),
    };
    let part = match checked(decode_with(data, &options)) {
        Ok(part) => part,
        Err(Error::InvalidOptions(_)) => return,
        Err(e) => panic!("the whole image decodes, but {options:?} gives {e:?}"),
    };
    let picks: Vec<usize> = match &subset {
        Some(list) => list.iter().map(|&c| usize::from(c)).collect(),
        None => (0..whole.components.len()).collect(),
    };
    assert_eq!(part.components.len(), picks.len(), "{options:?}");
    // The image's own size is the area cut to the image, on the reduced grid.
    let (cx0, cy0) = (area.0.max(image.0), area.1.max(image.1));
    let (cx1, cy1) = (area.2.min(image.2), area.3.min(image.3));
    assert_eq!(
        (part.width, part.height),
        (
            at(cx1, 1, r).saturating_sub(at(cx0, 1, r)),
            at(cy1, 1, r).saturating_sub(at(cy0, 1, r))
        ),
        "{options:?}: image size"
    );
    for (i, &c) in picks.iter().enumerate() {
        let (width, expected) = crop(&whole, c, image, area, r);
        let got = &part.components[i];
        let w = &whole.components[c];
        assert_eq!(
            (got.width, got.samples.len()),
            (width, expected.len()),
            "{options:?}: component {i} (codestream {c}) size"
        );
        assert!(
            got.samples == expected,
            "{options:?}: component {i} (codestream {c}) differs from the crop"
        );
        assert_eq!(
            (got.bit_depth, got.signed, got.x_sampling, got.y_sampling),
            (w.bit_depth, w.signed, w.x_sampling, w.y_sampling),
            "{options:?}: component {i} (codestream {c}) format"
        );
    }
});

/// FNV-1a over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
