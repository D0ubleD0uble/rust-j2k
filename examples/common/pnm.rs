//! A binary PGM and PPM writer for the examples.
//!
//! The examples include this file with `#[path]`, and so does
//! `tests/examples_pnm.rs`, which checks the bytes against the fixtures'
//! `.expected.json` snapshots. It is not an example itself: Cargo only builds
//! `examples/*.rs` and `examples/*/main.rs`.

use rust_j2k::Image;

/// Encode `image` as a binary PGM (one channel, `P5`) or PPM (three
/// channels, `P6`).
///
/// The maxval is `2^depth - 1`. A sample is one byte when the maxval is below
/// 256, and two big-endian bytes otherwise, as the format says. A signed
/// sample is shifted up by `2^(depth - 1)` to fit an unsigned format, and a
/// value outside the declared range (a palette channel can hold one) is
/// clamped to it.
///
/// Anything a PNM file can't hold is an error: a channel count other than 1
/// or 3, channels that differ in size or depth, or a depth over 16.
pub fn encode(image: &Image) -> Result<Vec<u8>, String> {
    let channels = image.components.len();
    let magic = match channels {
        1 => "P5",
        3 => "P6",
        n => {
            return Err(format!(
                "PGM and PPM hold 1 or 3 channels, and this image has {n}"
            ));
        }
    };
    let first = &image.components[0];
    for (i, c) in image.components.iter().enumerate() {
        if (c.width, c.height) != (first.width, first.height) {
            return Err(format!(
                "channel {i} is {}x{} and channel 0 is {}x{}: \
                 a PNM file needs one size for every channel",
                c.width, c.height, first.width, first.height
            ));
        }
        if c.bit_depth != first.bit_depth || c.signed != first.signed {
            return Err(format!(
                "channel {i} differs from channel 0 in depth or sign"
            ));
        }
    }
    let depth = u32::from(first.bit_depth);
    if depth > 16 {
        return Err(format!(
            "{depth}-bit samples don't fit a PNM file (16 at most)"
        ));
    }
    let maxval = (1i64 << depth) - 1;
    let offset = if first.signed { 1i64 << (depth - 1) } else { 0 };
    let wide = maxval > 255;

    let mut out = format!("{magic}\n{} {}\n{maxval}\n", first.width, first.height).into_bytes();
    let pixels = first.samples.len();
    out.reserve(pixels * channels * if wide { 2 } else { 1 });
    for i in 0..pixels {
        for c in &image.components {
            let value = (i64::from(c.samples[i]) + offset).clamp(0, maxval);
            if wide {
                out.extend_from_slice(&(value as u16).to_be_bytes());
            } else {
                out.push(value as u8);
            }
        }
    }
    Ok(out)
}
