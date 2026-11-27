//! Decode a JPEG 2000 file at a smaller size, or just a part of it, and write
//! it as a PGM or PPM image.
//!
//! It shows two options of `DecodeOptions`. `with_resolution_reduction(n)`
//! drops the `n` highest resolution levels, so each side is divided by 2^n
//! (rounded up; in general a span `x0..x1` becomes
//! `ceil(x1 / 2^n) - ceil(x0 / 2^n)`) and the decoder skips the data for
//! those levels.
//! `with_region(Region::Window(..))` decodes only a rectangle of the
//! reference grid. Both can be used together, and then the rectangle is
//! reduced as well.
//!
//! ```text
//! cargo run --example thumbnail -- \
//!     tests/fixtures/precincts_varied_sizes_lossless.j2k 2 thumb.ppm
//! cargo run --example thumbnail -- \
//!     tests/fixtures/precincts_varied_sizes_lossless.j2k 1 \
//!     16 8 112 88 part.ppm
//! ```
//!
//! The arguments are the file, the number of levels to drop, an optional
//! window as `x0 y0 x1 y1` (the left and top edges, and one past the right and
//! bottom edges, on the full-size grid), and the output file. The levels
//! can't exceed what the file has: `info` shows the largest reduction.
//!
//! The output is for reading, not parsing: its layout can change. The example
//! uses the default limits. A file that needs more is refused, and the error
//! says which limit; raise it with `decode_with` in your own code.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use rust_j2k::{DecodeOptions, Region, Window, decode_with};

#[path = "common/pnm.rs"]
mod pnm;

const WHOLE: &str = "a whole number, 0 or more";
const USAGE: &str = "usage: thumbnail <in.jp2|in.j2k> <levels> \
                     [x0 y0 x1 y1] <out.pgm|out.ppm>";

/// Read one number argument. `want` finishes "<name> must be ".
fn number<T: FromStr>(a: &OsStr, name: &str, want: &str) -> Result<T, String> {
    let parsed = a.to_str().and_then(|s| s.parse().ok());
    parsed.ok_or_else(|| format!("{name} must be {want}\n{USAGE}"))
}

fn run(args: &[OsString]) -> Result<(), String> {
    let (input, levels, window, output) = match args {
        [input, levels, output] => (input, levels, None, output),
        [input, levels, x0, y0, x1, y1, output] => {
            let edge = |arg, name| number::<u32>(arg, name, WHOLE);
            let window = Window::new(
                edge(x0, "x0")?,
                edge(y0, "y0")?,
                edge(x1, "x1")?,
                edge(y1, "y1")?,
            );
            (input, levels, Some(window), output)
        }
        _ => return Err(USAGE.into()),
    };
    let levels: u8 = number(levels, "levels", "a whole number from 0 to 255")?;
    let (input, output) = (PathBuf::from(input), PathBuf::from(output));
    let name = input.display();
    let mut options = DecodeOptions::default();
    options = options.with_resolution_reduction(levels);
    if let Some(window) = window {
        options = options.with_region(Region::Window(window));
    }
    let bytes = std::fs::read(&input).map_err(|e| format!("{name}: {e}"))?;
    let decoded = decode_with(&bytes, &options);
    let image = decoded.map_err(|e| format!("{name}: {e}"))?;
    let pnm = pnm::encode(&image).map_err(|e| format!("{name}: {e}"))?;
    let out_name = output.display();
    std::fs::write(&output, pnm).map_err(|e| format!("{out_name}: {e}"))?;
    println!(
        "wrote {}: {}x{} after dropping {levels} resolution {}",
        output.display(),
        image.width,
        image.height,
        if levels == 1 { "level" } else { "levels" }
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
