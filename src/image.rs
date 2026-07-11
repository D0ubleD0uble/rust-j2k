//! Decoder output: the image area on the reference grid, and one integer
//! component per SIZ component, each in raster order on its own sample grid,
//! plus the metadata a caller needs to interpret the samples (bit depth,
//! signedness, sub-sampling).

use crate::codestream::MainHeader;
use crate::codestream::markers::{Rect, Siz, Transform};
use crate::dwt::Samples;
use crate::{Error, Result};

/// A decoded image: the image area on the reference grid, plus its components.
///
/// [`width`](Self::width) and [`height`](Self::height) describe the *reference
/// grid* image area (`Xsiz - XOsiz` by `Ysiz - YOsiz`), which is the coordinate
/// space the components are registered against. A sub-sampled component covers
/// that same area with fewer samples, so its own
/// [`Component::width`]/[`Component::height`] can be smaller. With unit
/// sub-sampling — the common case — they are equal.
///
/// Components are in SIZ order and are independent: no inter-component (color)
/// transform is applied here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Reference-grid image area width, in reference-grid points.
    pub width: u32,
    /// Reference-grid image area height, in reference-grid points.
    pub height: u32,
    /// One entry per SIZ component, in SIZ order. Never empty.
    pub components: Vec<Component>,
}

/// One decoded component: its own sample grid, its declared depth and sign, and
/// the sub-sampling that maps it onto the image's reference grid.
///
/// `samples` is row-major, `width * height` entries, each already
/// DC-level-shifted and clamped to the declared depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// Component width in samples.
    pub width: u32,
    /// Component height in samples.
    pub height: u32,
    /// Bits per sample as declared in SIZ `Ssiz` (1..=32).
    pub bit_depth: u8,
    /// Whether samples are signed (SIZ component sign bit).
    pub signed: bool,
    /// Horizontal sub-sampling factor (SIZ `XRsiz`); `1` means unit sampling.
    pub x_sampling: u8,
    /// Vertical sub-sampling factor (SIZ `YRsiz`); `1` means unit sampling.
    pub y_sampling: u8,
    /// `width * height` samples, row-major.
    pub samples: Vec<i32>,
}

impl Image {
    /// The component at `index` in SIZ order, or `None` if out of range.
    pub fn component(&self, index: usize) -> Option<&Component> {
        self.components.get(index)
    }
}

impl Component {
    /// Sample at `(x, y)` on this component's own grid, or `None` if out of bounds.
    pub fn sample(&self, x: u32, y: u32) -> Option<i32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.samples
            .get((y as usize) * (self.width as usize) + x as usize)
            .copied()
    }
}

/// Final stage: apply the inverse DC level shift, clamp to the component depth,
/// and package the reconstructed samples with their geometry.
///
/// The component geometry follows the SIZ reference-grid equations (ISO/IEC
/// 15444-1 §B.2): for sub-sampling factors `XRsiz`/`YRsiz`,
///
/// ```text
/// width  = ceil(Xsiz / XRsiz) - ceil(XOsiz / XRsiz)
/// height = ceil(Ysiz / YRsiz) - ceil(YOsiz / YRsiz)
/// ```
///
/// which reduces to the image area `Xsiz - XOsiz` by `Ysiz - YOsiz` under unit
/// sub-sampling. The image area itself is carried on the [`Image`].
///
/// The inverse DC level shift (§G.1.2) adds `2^(depth-1)` back to *unsigned*
/// components (the encoder subtracted it before the forward transform); signed
/// components are left as-is. Samples are then clamped to the declared depth
/// and sign before being packed row-major into a [`Component`].
///
/// `samples` carries one reconstructed sample vector per SIZ component, in SIZ
/// order. Each is level-shifted and clamped on its own component's depth and
/// sign, then packed with that component's geometry.
/// The per-component sample planes a tiled decode reconstructs into: one plane
/// per SIZ component, sized to that component's whole grid, which each tile
/// writes its own rectangle of.
///
/// Tiles are decoded independently — their own coordinate frame, their own
/// header, their own wavelet pyramid — and the canvas is where they meet again.
/// A tile-component's reconstructed raster is placed at the tile's offset on the
/// component grid; the tiles partition that grid exactly, so every sample is
/// written once. Nothing is blended and no tile overlaps another: the wavelet
/// transform is applied per tile, which is why a tiled encode can show seams,
/// and reproducing them is correct.
pub(crate) struct Canvas {
    planes: Vec<Samples>,
    /// Each component's whole grid at the decode's reduction — the rectangle a
    /// tile's placement is measured against.
    rects: Vec<Rect>,
}

impl Canvas {
    /// Allocate one zeroed plane per component, at the reduced size
    /// [`assemble`] will expect.
    ///
    /// A plane's arithmetic follows its component's wavelet — 5/3 reconstructs
    /// exact integers, 9/7 reals — which is why `parse` rejects a codestream
    /// whose tiles disagree about a component's wavelet: it would ask one plane
    /// to hold both.
    pub(crate) fn new(header: &MainHeader, reduction: u8) -> Result<Self> {
        let siz = &header.siz;
        let (mut planes, mut rects) = (Vec::new(), Vec::new());
        for index in 0..siz.components.len() {
            let (width, height) = siz
                .component_extent_at(index, reduction)
                .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {index}")))?;
            let count = (width as usize) * (height as usize);
            planes.push(match header.components[index].coding.transform {
                Transform::Reversible53 => Samples::Reversible(vec![0; count]),
                Transform::Irreversible97 => Samples::Irreversible(vec![0.0; count]),
            });
            rects.push(
                siz.component_rect(index)
                    .ok_or_else(|| {
                        Error::Inconsistent(format!("SIZ declares no component {index}"))
                    })?
                    .reduced(reduction),
            );
        }
        Ok(Canvas { planes, rects })
    }

    /// Write one tile-component's reconstructed raster into its component's
    /// plane. `rect` is the tile-component's rectangle on the component grid, at
    /// the same reduction the canvas was built at.
    pub(crate) fn place(&mut self, index: usize, rect: Rect, samples: Samples) -> Result<()> {
        let (plane, canvas) = self
            .planes
            .get_mut(index)
            .zip(self.rects.get(index))
            .ok_or_else(|| Error::Inconsistent(format!("no canvas plane for component {index}")))?;
        if samples.len() as u64 != rect.area() {
            return Err(Error::Inconsistent(format!(
                "component {index} of a tile reconstructed {} samples but its {}x{} rectangle \
                 holds {}",
                samples.len(),
                rect.width(),
                rect.height(),
                rect.area(),
            )));
        }
        // The tile grid partitions the canvas, so this only fails on a header
        // whose SIZ tiling and whose decoded geometry disagree.
        if rect.x0 < canvas.x0 || rect.y0 < canvas.y0 || rect.x1 > canvas.x1 || rect.y1 > canvas.y1
        {
            return Err(Error::Inconsistent(format!(
                "a tile's component {index} rectangle {rect:?} falls outside the canvas {canvas:?}",
            )));
        }
        let (left, top) = (
            (rect.x0 - canvas.x0) as usize,
            (rect.y0 - canvas.y0) as usize,
        );
        let stride = canvas.width() as usize;
        match (plane, samples) {
            (Samples::Reversible(dst), Samples::Reversible(src)) => {
                blit(dst, stride, &src, rect.width() as usize, left, top);
            }
            (Samples::Irreversible(dst), Samples::Irreversible(src)) => {
                blit(dst, stride, &src, rect.width() as usize, left, top);
            }
            _ => {
                return Err(Error::Inconsistent(format!(
                    "a tile reconstructed component {index} on the other wavelet's arithmetic"
                )));
            }
        }
        Ok(())
    }

    /// The finished planes, in SIZ component order, ready for [`assemble`].
    pub(crate) fn into_planes(self) -> Vec<Samples> {
        self.planes
    }
}

/// Copy a `src_width`-wide raster into `dst` (a `stride`-wide raster) with its
/// top-left at `(left, top)`. The caller has already checked that it fits.
fn blit<T: Copy>(
    dst: &mut [T],
    stride: usize,
    src: &[T],
    src_width: usize,
    left: usize,
    top: usize,
) {
    if src_width == 0 {
        return;
    }
    for (row, line) in src.chunks_exact(src_width).enumerate() {
        let start = (top + row) * stride + left;
        dst[start..start + src_width].copy_from_slice(line);
    }
}

pub(crate) fn assemble(header: &MainHeader, samples: Vec<Samples>, reduction: u8) -> Result<Image> {
    let siz = &header.siz;
    if samples.len() != siz.components.len() {
        return Err(Error::Inconsistent(format!(
            "decoded {} components but SIZ declares {}",
            samples.len(),
            siz.components.len(),
        )));
    }
    if samples.is_empty() {
        return Err(Error::Inconsistent("SIZ declares no components".into()));
    }

    let components = samples
        .into_iter()
        .enumerate()
        .map(|(index, component_samples)| {
            assemble_component(siz, index, component_samples, reduction)
        })
        .collect::<Result<Vec<_>>>()?;

    // At a reduction the image area shrinks with its components, by the same
    // halve-and-round-up per dropped level.
    let (image_width, image_height) = siz.image_extent_at(reduction);
    Ok(Image {
        width: image_width,
        height: image_height,
        components,
    })
}

/// Round to the nearest integer, level-shift, clamp, and package one
/// component's reconstructed samples.
///
/// This is the *only* place the irreversible path rounds. It happens after the
/// inverse colour transform, matching `opj_tcd_dc_level_shift_decode`, which
/// calls `opj_lrintf` and then adds the DC level shift and clamps:
///
/// ```c
/// OPJ_INT64 l_value_int = (OPJ_INT64)opj_lrintf(l_value);
/// *l_dest = opj_int64_clamp(l_value_int + l_tccp->m_dc_level_shift, l_min, l_max);
/// ```
///
/// `opj_lrintf` is `lrintf` on GCC and Clang — the build that produced the
/// committed oracles — and `lrintf` rounds under the default floating-point
/// mode, which is ties-to-even. Its MSVC arms (`cvtss2si`, `fistp`) round
/// ties-to-even too. Rust's `f32::round` rounds ties *away from zero*, so `2.5`
/// would become `3` where the oracle gives `2`; `round_ties_even` agrees.
///
/// OpenJPEG tried the away-from-zero form and reverted it, leaving the evidence
/// in `opj_includes.h`:
///
/// ```c
/// /* commented out line breaks many tests */
/// /* return (long)((f>0.0f) ? (f + 0.5f):(f -0.5f)); */
/// ```
fn assemble_component(
    siz: &Siz,
    index: usize,
    samples: Samples,
    reduction: u8,
) -> Result<Component> {
    let comp = siz
        .components
        .get(index)
        .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {index}")))?;

    let depth = comp.bit_depth;
    // The standard allows 1..=38 (Table A-11) and `decode_siz` enforces it;
    // this defensive restatement keeps the shift arithmetic below well-defined
    // for callers that assemble a header by hand. Legal depths whose range
    // does not fit the i32 sample container (33..=38, and unsigned 32) fall
    // through to the container check, which rejects them as Unsupported.
    if !(1..=38).contains(&depth) {
        return Err(Error::Marker(format!(
            "component {index} bit depth {depth} outside the standard's range 1..=38"
        )));
    }
    if comp.x_sampling == 0 || comp.y_sampling == 0 {
        return Err(Error::Marker(format!(
            "component {index} sub-sampling factor is zero"
        )));
    }

    // Safe now that the zero-factor case above is excluded.
    let (width, height) = siz
        .component_extent_at(index, reduction)
        .ok_or_else(|| Error::Inconsistent(format!("SIZ declares no component {index}")))?;

    let expected = (width as usize) * (height as usize);
    if samples.len() != expected {
        return Err(Error::Inconsistent(format!(
            "component {index} decoded {} samples but its geometry is {width}x{height} = {expected}",
            samples.len()
        )));
    }

    // The irreversible path arrives unrounded. `as i32` saturates, so a value
    // outside the container cannot wrap before the clamp below sees it.
    let samples: Vec<i32> = match samples {
        Samples::Reversible(values) => values,
        Samples::Irreversible(values) => values
            .into_iter()
            .map(|v| v.round_ties_even() as i32)
            .collect(),
    };

    // Level-shift offset and the clamp bounds, all in i64 so the `1 << 31`
    // signed-32-bit case and the unsigned `2^depth - 1` upper bound cannot
    // overflow during the computation.
    let (shift, lo, hi): (i64, i64, i64) = if comp.signed {
        (0, -(1i64 << (depth - 1)), (1i64 << (depth - 1)) - 1)
    } else {
        (1i64 << (depth - 1), 0, (1i64 << depth) - 1)
    };

    // The output container is `i32`; reject depths whose clamp range cannot be
    // represented (e.g. unsigned 32-bit, whose max is `2^32 - 1`). Checked once
    // so the per-sample cast below is always exact.
    if hi > i32::MAX as i64 || lo < i32::MIN as i64 {
        return Err(Error::Unsupported(format!(
            "component {index} depth {depth} ({}) exceeds the i32 sample container",
            if comp.signed { "signed" } else { "unsigned" }
        )));
    }

    let shifted: Vec<i32> = samples
        .into_iter()
        .map(|v| (v as i64 + shift).clamp(lo, hi) as i32)
        .collect();

    Ok(Component {
        width,
        height,
        bit_depth: depth,
        signed: comp.signed,
        x_sampling: comp.x_sampling,
        y_sampling: comp.y_sampling,
        samples: shifted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codestream::markers::{
        Cod, Progression, Qcd, QuantStyle, Siz, SizComponent, Transform,
    };

    /// A minimal single-component header: an image of `x_size` by `y_size` at the
    /// origin with unit sub-sampling. Only the SIZ fields `assemble` reads
    /// matter; COD/QCD are filler. Tests tweak offsets/sampling on the result.
    fn header(x_size: u32, y_size: u32, bit_depth: u8, signed: bool) -> MainHeader {
        MainHeader::new(
            Siz {
                x_size,
                y_size,
                x_offset: 0,
                y_offset: 0,
                tile_width: x_size,
                tile_height: y_size,
                tile_x_offset: 0,
                tile_y_offset: 0,
                components: vec![SizComponent {
                    bit_depth,
                    signed,
                    x_sampling: 1,
                    y_sampling: 1,
                }],
            },
            Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: 0,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                use_sop: false,
                use_eph: false,
                multiple_component_transform: false,
                transform: Transform::Reversible53,
                precinct_sizes: vec![],
            },
            Qcd {
                style: QuantStyle::None,
                guard_bits: 1,
                steps: vec![],
            },
        )
    }

    /// Assemble a single-component image: most of these tests predate the
    /// component axis and care only about level shift, clamping, and geometry.
    fn assemble1(header: &MainHeader, samples: Vec<i32>) -> Result<Image> {
        assemble(header, vec![Samples::Reversible(samples)], 0)
    }

    /// The single component `assemble` produces, for tests that only care about
    /// the samples and not the enclosing image area.
    fn only(img: &Image) -> &Component {
        assert_eq!(img.components.len(), 1, "assemble emits one component");
        img.component(0).unwrap()
    }

    #[test]
    fn unsigned_adds_level_shift_and_clamps() {
        // 8-bit unsigned: shift = 128, clamp to [0, 255].
        let h = header(2, 2, 8, false);
        let img = assemble1(&h, vec![0, -128, 127, 200]).unwrap();
        // 0 -> 128 (mid), -128 -> 0 (low edge), 127 -> 255 (high edge),
        // 200 -> 328 clamps to 255.
        assert_eq!(only(&img).samples, vec![128, 0, 255, 255]);
        // And the low edge under-shoot also clamps.
        let img = assemble1(&header(1, 1, 8, false), vec![-200]).unwrap();
        assert_eq!(only(&img).samples, vec![0]);
    }

    #[test]
    fn signed_is_unshifted_and_clamps() {
        // 8-bit signed: no shift, clamp to [-128, 127].
        let h = header(2, 2, 8, true);
        let img = assemble1(&h, vec![0, 50, 200, -200]).unwrap();
        assert_eq!(only(&img).samples, vec![0, 50, 127, -128]);
    }

    #[test]
    fn wider_depths_round_trip_edges() {
        // 12-bit unsigned: shift = 2048, clamp to [0, 4095].
        let img = assemble1(&header(1, 4, 12, false), vec![-2048, 0, 2047, 9999]).unwrap();
        assert_eq!(only(&img).samples, vec![0, 2048, 4095, 4095]);
        // 16-bit signed: clamp to [-32768, 32767], no shift.
        let img = assemble1(&header(1, 3, 16, true), vec![-40000, 12345, 40000]).unwrap();
        assert_eq!(only(&img).samples, vec![-32768, 12345, 32767]);
    }

    #[test]
    fn geometry_uses_siz_image_area() {
        let mut h = header(6, 5, 8, false);
        h.siz.x_offset = 2;
        h.siz.y_offset = 1;
        // width = 6 - 2 = 4, height = 5 - 1 = 4.
        let img = assemble1(&h, vec![0; 16]).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        let c = only(&img);
        assert_eq!((c.width, c.height), (4, 4));
        assert_eq!(c.bit_depth, 8);
        assert!(!c.signed);
        assert_eq!(c.sample(3, 3), Some(128));
        assert_eq!(c.sample(4, 0), None);
    }

    #[test]
    fn geometry_honours_sub_sampling() {
        let mut h = header(8, 8, 8, false);
        h.siz.components[0].x_sampling = 2;
        h.siz.components[0].y_sampling = 2;
        let img = assemble1(&h, vec![0; 16]).unwrap();
        // The image area stays on the reference grid ...
        assert_eq!((img.width, img.height), (8, 8));
        // ... while the component carries ceil(8/2) - ceil(0/2) = 4 per axis.
        let c = only(&img);
        assert_eq!((c.width, c.height), (4, 4));
        assert_eq!((c.x_sampling, c.y_sampling), (2, 2));
    }

    /// The irreversible path rounds once, here, and rounds ties to even —
    /// matching `opj_lrintf` under the default floating-point mode. Rust's
    /// `f32::round` rounds ties away from zero and would disagree at every
    /// half-integer.
    #[test]
    fn irreversible_samples_round_ties_to_even() {
        // 8-bit signed: no level shift, clamp to [-128, 127].
        let h = header(5, 1, 8, true);
        let img = assemble(
            &h,
            vec![Samples::Irreversible(vec![0.5, 1.5, 2.5, -0.5, -2.5])],
            0,
        )
        .unwrap();
        assert_eq!(only(&img).samples, vec![0, 2, 2, 0, -2]);

        // Away-from-zero would have given [1, 2, 3, -1, -3].
        let away: Vec<i32> = [0.5f32, 1.5, 2.5, -0.5, -2.5]
            .iter()
            .map(|v| v.round() as i32)
            .collect();
        assert_ne!(away, only(&img).samples);
    }

    /// A reconstructed value outside the `i32` container saturates rather than
    /// wrapping, before the depth clamp sees it.
    #[test]
    fn irreversible_samples_saturate_into_the_container() {
        let h = header(2, 1, 16, true);
        let img = assemble(&h, vec![Samples::Irreversible(vec![1e30, -1e30])], 0).unwrap();
        assert_eq!(only(&img).samples, vec![32767, -32768]);
    }

    #[test]
    fn sample_count_mismatch_is_inconsistent() {
        let h = header(4, 4, 8, false); // expects 16 samples
        let err = assemble1(&h, vec![0; 15]).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)), "got {err:?}");
    }

    #[test]
    fn depth_beyond_i32_container_is_unsupported() {
        // Unsigned 32-bit's upper bound (2^32 - 1) cannot fit in i32, and the
        // standard-legal 33..=38 depths (Table A-11) exceed it either way —
        // legal-but-undecodable, so Unsupported rather than Marker.
        for (depth, signed) in [(32, false), (33, false), (33, true), (38, true)] {
            let h = header(1, 1, depth, signed);
            let err = assemble1(&h, vec![0]).unwrap_err();
            assert!(matches!(err, Error::Unsupported(_)), "{depth} got {err:?}");
        }
        // Signed 32-bit fits exactly and is accepted.
        let img = assemble1(&header(1, 1, 32, true), vec![i32::MIN]).unwrap();
        assert_eq!(only(&img).samples, vec![i32::MIN]);
    }

    #[test]
    fn bad_depth_is_rejected() {
        // 0 and 39+ are illegal encodings (Table A-11), not missing features.
        for depth in [0, 39] {
            let h = header(1, 1, depth, false);
            assert!(matches!(assemble1(&h, vec![0]), Err(Error::Marker(_))));
        }
    }
}
