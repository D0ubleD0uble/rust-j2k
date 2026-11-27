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
//! cargo run --example decode -- \
//!     tests/fixtures/multicomponent_lossless.j2k out.ppm
//! ```
//!
//! The input is a raw codestream (`.j2k`, `.j2c`) or a JP2 file. The values
//! are written as stored: no color conversion and no scaling to 8 bits. The
//! example uses the default limits. A file that needs more is refused, and
//! the error says which limit; raise it with `decode_with` in your own code.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

#[macro_use]
#[path = "common/say.rs"]
mod say;

#[path = "common/pnm.rs"]
mod pnm;

fn run(args: &[OsString]) -> Result<(), String> {
    let [input, output] = args else {
        return Err("usage: decode <in.jp2|in.j2k> <out.pgm|out.ppm>".into());
    };
    let (input, output) = (PathBuf::from(input), PathBuf::from(output));
    let name = input.display();
    let bytes = std::fs::read(&input).map_err(|e| format!("{name}: {e}"))?;
    let image = rust_j2k::decode(&bytes).map_err(|e| format!("{name}: {e}"))?;
    let pnm = pnm::encode(&image).map_err(|e| format!("{name}: {e}"))?;
    let out_name = output.display();
    std::fs::write(&output, pnm).map_err(|e| format!("{out_name}: {e}"))?;
    let n = image.components.len();
    say!(
        "wrote {}: {}x{}, {n} {}",
        output.display(),
        image.width,
        image.height,
        if n == 1 { "channel" } else { "channels" }
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
