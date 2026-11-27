//! Decode a JPEG 2000 file as it arrives, a chunk at a time.
//!
//! It shows `IncrementalDecoder`, for input that comes in over time, such as
//! a download. The example reads the file and hands it over in chunks of the
//! size you give, as a network would:
//!
//! 1. `feed` adds a chunk.
//! 2. `header` returns `None` until the main header is in. Then it returns
//!    the image's size and layout, which can be shown before any pixels.
//! 3. Once the header is in, `refine` after each chunk returns either
//!    `Refined::Image`, the image the bytes so far give (with a report of what
//!    is still missing), or `Refined::Waiting`, with the reason there is no
//!    new image. A refine decodes again only when the bytes fed have doubled
//!    since the last try, so most chunks wait.
//! 4. `finish` decodes all of the input, as `decode` does. On a truncated
//!    download it refuses by default. In your own code, `with_strict(false)`
//!    makes `finish` return the partial image once the header is in.
//!
//! ```text
//! cargo run --example stream -- tests/fixtures/layers/lrcp.j2k 100
//! ```
//!
//! The output is for reading, not parsing: its layout can change. The example
//! uses the default limits. A file that needs more is refused, and the error
//! says which limit; raise it with `DecodeOptions` in your own code.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use rust_j2k::{DecodeOptions, Error, IncrementalDecoder};
use rust_j2k::{Refined, Waiting, decode};

#[macro_use]
#[path = "common/say.rs"]
mod say;

/// Say why a refine gave no image.
fn describe(why: &Waiting) -> String {
    match why {
        Waiting::MoreBytes { until, .. } => {
            format!("waiting, the next try is at {until} bytes")
        }
        Waiting::HeaderIncomplete { .. } => "waiting, header not whole".into(),
        Waiting::SampleLimit { limit, .. } => format!("waiting, {limit}"),
        Waiting::WorkLimit { limit, .. } => format!("waiting, {limit}"),
        Waiting::BoundSpent { limit, .. } => format!("waiting, {limit}"),
        Waiting::MissingTiles { limit, .. } => format!("waiting, {limit}"),
        Waiting::RepeatLimit { .. } => {
            "waiting, repeat-walk bound spent; decode the bytes fed now".into()
        }
        Waiting::Held { error, .. } => format!("waiting, held: {error}"),
        _ => "waiting, for a reason this example does not show".into(),
    }
}

fn run(args: &[OsString]) -> Result<(), String> {
    let [input, chunk] = args else {
        return Err("usage: stream <in.jp2|in.j2k> <chunk-bytes>".into());
    };
    let path = PathBuf::from(input);
    let name = path.display();
    let chunk: usize = chunk
        .to_str()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .ok_or("chunk-bytes must be a whole number above 0")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
    let at = |e: Error| format!("{name}: {e}");
    let options = DecodeOptions::default();
    let mut decoder = IncrementalDecoder::new(options).map_err(at)?;
    let mut header_shown = false;
    for piece in bytes.chunks(chunk) {
        decoder.feed(piece);
        let fed = decoder.bytes_fed();
        if !header_shown {
            let header = decoder.header().map_err(at)?;
            let Some(header) = header else {
                say!("{fed} bytes: no header yet");
                continue;
            };
            say!(
                "{fed} bytes: header is in, {} x {}, {} quality layers",
                header.width,
                header.height,
                header.quality_layers
            );
            header_shown = true;
        }
        match decoder.refine().map_err(at)? {
            Refined::Image(image) => {
                let state = if image.report.is_complete() {
                    "complete"
                } else {
                    "partial"
                };
                say!(
                    "{fed} bytes: refined, {} x {}, {state}",
                    image.width,
                    image.height
                );
            }
            Refined::Waiting(why) => {
                say!("{fed} bytes: {}", describe(&why));
            }
            _ => say!("{fed} bytes: a result this example does not show"),
        }
    }
    let image = decoder.finish().map_err(at)?;
    say!("finished: {} x {}", image.width, image.height);
    let whole = decode(&bytes).map_err(at)?;
    let same = image.components == whole.components;
    say!("same samples as decode on the whole file: {same}");
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
