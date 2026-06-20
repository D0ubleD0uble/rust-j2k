//! Decoder output: one integer component in raster order, plus the metadata a
//! caller needs to interpret the samples (bit depth + signedness).

use crate::Result;
use crate::codestream::MainHeader;

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
pub(crate) fn assemble(header: &MainHeader, samples: Vec<i32>) -> Result<Image> {
    todo!("DC level shift + clamp, then build Image from SIZ geometry")
}
