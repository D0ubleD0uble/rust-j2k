//! Decode a file from a source you don't trust: the default limits, an
//! opt-in retry past a limit, and a partial decode of a cut-short file.
//!
//! ```text
//! cargo run --example untrusted -- \
//!     tests/fixtures/psot0/single_psot0_no_eoc_trunc100.j2k
//! cargo run --example untrusted -- --allow-raise \
//!     tests/fixtures/tier1_work_bound/t1bomb_1024_64_88.j2k
//! ```
//!
//! The steps:
//!
//! 1. Decode with `DecodeOptions::default()`. The default limits bound the
//!    memory and work a decode may take, whatever the input.
//! 2. On `Error::Limit`, print the `LimitDetail`: which limit refused the
//!    file, what to raise it to, and, for some limits, the tile refused.
//!    Without `--allow-raise` that is the end. With it, retry once with
//!    `DecodeOptions::raised_for`, if the factor it gives is at most 4; a
//!    limit is never lifted to `Unlimited`. A flipped bit can make a
//!    damaged file ask for an absurd size, so a refusal alone is no reason
//!    to raise a limit: pass `--allow-raise` only for a file you trust, or
//!    when you can afford the decode it asks for. A service that decodes
//!    files from outside should not retry at all; see "Running a decode
//!    service" in `SECURITY.md`.
//! 3. On `Error::Codestream`, which is how a cut-short file shows, decode
//!    again with `with_strict(false)`. That decodes what the input holds and
//!    lists what is missing on `Image::report`. It lets through only missing
//!    data and bytes left after a tile's last packet; any other damage is
//!    still an error, so a file that is damaged rather than short fails
//!    again.
//!
//! Every other error is printed and ends the run. The output is for reading,
//! not parsing: its layout can change, and so can the text of an error or a
//! report entry.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use rust_j2k::decode_with;
use rust_j2k::{DecodeOptions, DecodeReport, Error, LimitDetail, Raise};

/// How many report entries to print. A hostile file can carry many.
const SHOWN: usize = 16;

/// The largest factor `--allow-raise` raises a limit by. Each step of it
/// lets the decode take that many times the default's memory or work.
const MAX_FACTOR: u32 = 4;

fn print_limit(limit: &LimitDetail) {
    println!("refused by a limit: {limit}");
    println!("  kind: {:?}", limit.kind());
    match limit.retry_with() {
        Some(raise) => println!("  retry with: {raise:?}"),
        None => println!("  retry with: no option raises it"),
    }
    if let Some(at) = limit.refused_at() {
        println!(
            "  refused at: tile {}, which needs {} ({:?})",
            at.tile(),
            at.amount(),
            at.unit()
        );
    }
}

fn print_report(report: &DecodeReport) {
    println!("complete: {}", report.is_complete());
    match report.complete_reduction {
        Some(r) => println!("complete at reduction: {r}"),
        None => println!("complete at reduction: none"),
    }
    for entry in report.entries.iter().take(SHOWN) {
        println!("  {entry}");
    }
    let more = report.entries.iter().skip(SHOWN).count();
    if more > 0 {
        println!("  and {more} more");
    }
}

/// Split the arguments into the `--allow-raise` flag and the input path.
fn parse(args: &[OsString]) -> Result<(bool, PathBuf), String> {
    const USAGE: &str = "usage: untrusted [--allow-raise] <file.jp2|file.j2k>";
    match args {
        [path] if path != "--allow-raise" => Ok((false, path.into())),
        [flag, path] if flag == "--allow-raise" => Ok((true, path.into())),
        _ => Err(USAGE.into()),
    }
}

fn run(args: &[OsString]) -> Result<(), String> {
    let (allow_raise, path) = parse(args)?;
    let name = path.display();
    let bytes = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;

    let mut options = DecodeOptions::default();
    let mut raised = false;
    // At most three decodes: the first, one with a limit raised, and one
    // with `strict` off.
    let image = loop {
        let error = match decode_with(&bytes, &options) {
            Ok(image) => break image,
            Err(error) => error,
        };
        match error {
            Error::Limit(limit) => {
                print_limit(&limit);
                // A factor to raise by. `Unlimited` would lift the limit,
                // which this example never does.
                let factor = match limit.retry_with() {
                    Some(Raise::Raised(factor)) => factor,
                    _ => return Err(format!("{name}: refused by a limit")),
                };
                if !allow_raise {
                    return Err(format!(
                        "{name}: refused; pass --allow-raise to retry with \
                         the limit raised, for a file you trust"
                    ));
                }
                if raised {
                    return Err(format!("{name}: refused after one raise"));
                }
                if factor > MAX_FACTOR {
                    return Err(format!(
                        "{name}: needs a factor of {factor}, past this \
                         example's cap of {MAX_FACTOR}"
                    ));
                }
                let Some(next) = options.raised_for(&limit) else {
                    return Err(format!("{name}: no option raises this limit"));
                };
                println!("retrying once with the limit raised");
                options = next;
                raised = true;
            }
            Error::Codestream(detail) if options.strict() => {
                println!("refused as damaged or cut short: {detail}");
                println!("decoding again with strict off");
                options = options.with_strict(false);
            }
            other => return Err(format!("{name}: {other}")),
        }
    };

    println!(
        "decoded {}: {}x{}, {} {}",
        name,
        image.width,
        image.height,
        image.components.len(),
        if image.components.len() == 1 {
            "channel"
        } else {
            "channels"
        }
    );
    print_report(&image.report);
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
