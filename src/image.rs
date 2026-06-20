//! Decoder output: one integer component in raster order, plus the metadata a
//! caller needs to interpret the samples (bit depth + signedness).

use crate::codestream::MainHeader;
use crate::{Error, Result};

/// A decoded single-component image. `samples` is row-major, `width * height`
/// entries, each already DC-level-shifted and clamped to the declared depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Bits per sample as declared in SIZ (1..=32 for the GRIB2 subset).
    pub bit_depth: u8,
    /// Whether samples are signed (SIZ component sign bit).
    pub signed: bool,
    /// `width * height` samples, row-major.
    pub samples: Vec<i32>,
}

impl Image {
    /// Sample at `(x, y)`, or `None` if out of bounds.
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
/// which reduces to `Xsiz - XOsiz` by `Ysiz - YOsiz` for the unit-sampling
/// single-component GRIB2 case.
///
/// The inverse DC level shift (§G.1.2) adds `2^(depth-1)` back to *unsigned*
/// components (the encoder subtracted it before the forward transform); signed
/// components are left as-is. Samples are then clamped to the declared depth
/// and sign before being packed row-major into the [`Image`].
pub(crate) fn assemble(header: &MainHeader, samples: Vec<i32>) -> Result<Image> {
    let siz = &header.siz;
    let comp = siz
        .components
        .first()
        .ok_or_else(|| Error::Inconsistent("SIZ declares no components".into()))?;

    let depth = comp.bit_depth;
    if !(1..=32).contains(&depth) {
        return Err(Error::Marker(format!(
            "component bit depth {depth} outside the supported range 1..=32"
        )));
    }
    if comp.x_sampling == 0 || comp.y_sampling == 0 {
        return Err(Error::Marker(
            "component sub-sampling factor is zero".into(),
        ));
    }

    let xr = comp.x_sampling as u32;
    let yr = comp.y_sampling as u32;
    let width = siz
        .x_size
        .div_ceil(xr)
        .saturating_sub(siz.x_offset.div_ceil(xr));
    let height = siz
        .y_size
        .div_ceil(yr)
        .saturating_sub(siz.y_offset.div_ceil(yr));

    let expected = (width as usize) * (height as usize);
    if samples.len() != expected {
        return Err(Error::Inconsistent(format!(
            "decoded {} samples but SIZ geometry is {width}x{height} = {expected}",
            samples.len()
        )));
    }

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
            "component depth {depth} ({}) exceeds the i32 sample container",
            if comp.signed { "signed" } else { "unsigned" }
        )));
    }

    let shifted: Vec<i32> = samples
        .into_iter()
        .map(|v| (v as i64 + shift).clamp(lo, hi) as i32)
        .collect();

    Ok(Image {
        width,
        height,
        bit_depth: depth,
        signed: comp.signed,
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
        MainHeader {
            siz: Siz {
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
            cod: Cod {
                progression: Progression::Lrcp,
                layers: 1,
                decomposition_levels: 0,
                code_block_width: 4,
                code_block_height: 4,
                code_block_style: 0,
                transform: Transform::Reversible53,
                precinct_sizes: vec![],
            },
            qcd: Qcd {
                style: QuantStyle::None,
                guard_bits: 1,
                steps: vec![],
            },
        }
    }

    #[test]
    fn unsigned_adds_level_shift_and_clamps() {
        // 8-bit unsigned: shift = 128, clamp to [0, 255].
        let h = header(2, 2, 8, false);
        let img = assemble(&h, vec![0, -128, 127, 200]).unwrap();
        // 0 -> 128 (mid), -128 -> 0 (low edge), 127 -> 255 (high edge),
        // 200 -> 328 clamps to 255.
        assert_eq!(img.samples, vec![128, 0, 255, 255]);
        // And the low edge under-shoot also clamps.
        let img = assemble(&header(1, 1, 8, false), vec![-200]).unwrap();
        assert_eq!(img.samples, vec![0]);
    }

    #[test]
    fn signed_is_unshifted_and_clamps() {
        // 8-bit signed: no shift, clamp to [-128, 127].
        let h = header(2, 2, 8, true);
        let img = assemble(&h, vec![0, 50, 200, -200]).unwrap();
        assert_eq!(img.samples, vec![0, 50, 127, -128]);
    }

    #[test]
    fn wider_depths_round_trip_edges() {
        // 12-bit unsigned: shift = 2048, clamp to [0, 4095].
        let img = assemble(&header(1, 4, 12, false), vec![-2048, 0, 2047, 9999]).unwrap();
        assert_eq!(img.samples, vec![0, 2048, 4095, 4095]);
        // 16-bit signed: clamp to [-32768, 32767], no shift.
        let img = assemble(&header(1, 3, 16, true), vec![-40000, 12345, 40000]).unwrap();
        assert_eq!(img.samples, vec![-32768, 12345, 32767]);
    }

    #[test]
    fn geometry_uses_siz_image_area() {
        let mut h = header(6, 5, 8, false);
        h.siz.x_offset = 2;
        h.siz.y_offset = 1;
        // width = 6 - 2 = 4, height = 5 - 1 = 4.
        let img = assemble(&h, vec![0; 16]).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.bit_depth, 8);
        assert!(!img.signed);
        assert_eq!(img.sample(3, 3), Some(128));
        assert_eq!(img.sample(4, 0), None);
    }

    #[test]
    fn geometry_honours_sub_sampling() {
        let mut h = header(8, 8, 8, false);
        h.siz.components[0].x_sampling = 2;
        h.siz.components[0].y_sampling = 2;
        // ceil(8/2) - ceil(0/2) = 4 in each axis.
        let img = assemble(&h, vec![0; 16]).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
    }

    #[test]
    fn sample_count_mismatch_is_inconsistent() {
        let h = header(4, 4, 8, false); // expects 16 samples
        let err = assemble(&h, vec![0; 15]).unwrap_err();
        assert!(matches!(err, Error::Inconsistent(_)), "got {err:?}");
    }

    #[test]
    fn depth_beyond_i32_container_is_unsupported() {
        // Unsigned 32-bit's upper bound (2^32 - 1) cannot fit in i32.
        let h = header(1, 1, 32, false);
        let err = assemble(&h, vec![0]).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "got {err:?}");
        // Signed 32-bit fits exactly and is accepted.
        let img = assemble(&header(1, 1, 32, true), vec![i32::MIN]).unwrap();
        assert_eq!(img.samples, vec![i32::MIN]);
    }

    #[test]
    fn bad_depth_is_rejected() {
        let h = header(1, 1, 0, false);
        assert!(matches!(assemble(&h, vec![0]), Err(Error::Marker(_))));
    }
}
