//! Decode a JPEG 2000 file and write it as a PGM or PPM image.
//!
//! It shows `decode`, and how an `Image` holds its channels: one `Component`
//! each, with `samples` in raster order. A one-channel image becomes a binary
//! PGM and a three-channel one a binary PPM. Samples of 8 bits or fewer are
//! written as one byte, deeper ones as two big-endian bytes, as the formats
//! say. Any other channel count is an error, and so are channels that differ
//! in size or depth, such as a subsampled image.
//!
//! ```text
//! cargo run --example decode -- tests/fixtures/multicomponent_lossless.j2k out.ppm
//! ```
//!
//! The input is a raw codestream (`.j2k`, `.j2c`) or a JP2 file. The values
//! are written as stored: no color conversion and no scaling to 8 bits.

use std::process::ExitCode;

#[path = "common/pnm.rs"]
mod pnm;

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [input, output] = args else {
        return Err("usage: decode <in.jp2|in.j2k> <out.pgm|out.ppm>".into());
    };
    let bytes = std::fs::read(input)?;
    let image = rust_j2k::decode(&bytes)?;
    std::fs::write(output, pnm::encode(&image)?)?;
    println!(
        "wrote {output}: {}x{}, {} channels",
        image.width,
        image.height,
        image.components.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("decode: {e}");
            ExitCode::FAILURE
        }
    }
}
