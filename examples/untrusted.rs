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
//!    memory a decode takes, in proportion to the input's size, and the
//!    work it does. They don't bound its time.
//! 2. On `Error::Limit`, print the `LimitDetail`: which limit refused the
//!    file, what to raise it to, and, for some limits, the tile refused.
//!    Without `--allow-raise` that is the end. With it, retry once with
//!    `DecodeOptions::raised_for`, if the factor it gives is at most 4. A
//!    limit is never lifted to `Unlimited`. A flipped bit can make a
//!    damaged file ask for an absurd size, so a refusal alone is no reason
//!    to raise a limit: pass `--allow-raise` only for a file you trust, or
//!    when you can afford the decode it asks for.
//! 3. On `Error::Codestream`, which is how a file cut after its main header
//!    shows, decode again with `with_strict(false)`. That decodes what the
//!    input holds and lists what is missing on `Image::report`. It lets
//!    through only missing data and bytes left after a tile's last packet;
//!    any other damage is still an error, so a file that is damaged rather
//!    than short fails again. This step needs no flag: every limit still
//!    holds, counted on the data that is there. A file cut before its main
//!    header ends is `Error::Incomplete`, which strict off does not help.
//!
//! Every other error is printed and ends the run. The exit status is 0 for
//! a complete image, 2 for an image with data missing, and 1 for an error.
//! An image with data missing is not the file's image: check
//! `DecodeReport::is_complete` before you trust it. A copy that adds an
//! argument parser that exits with 2 on a usage error, as clap does, should
//! give a partial image another code.
//!
//! The raise is capped at 4 because each factor multiplies what the decode
//! may take. At 4, a small file may take 8 GiB of output, up to about 26
//! GiB of sample buffers, and 4 times the default work. The example raises
//! once and stops if the retry is refused again. `raised_for`'s doc
//! suggests one more try at the cap when a refusal names a factor past it;
//! that is a further full-cost decode, so this example refuses instead.
//!
//! A service that decodes files from outside should not retry at all. It
//! should also cap the size of the input it reads (this example reads the
//! whole file), set `sample_limit` to `SampleLimit::AtMost(n)` for a fixed
//! ceiling, and run each decode under a timeout; see "Running a decode
//! service" in `SECURITY.md`.
//!
//! The output is for reading, not parsing: its layout can change, and so
//! can the text of an error or a report entry.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use rust_j2k::decode_with;
use rust_j2k::{DecodeOptions, DecodeReport, Error, LimitDetail, Raise};

#[macro_use]
#[path = "common/say.rs"]
mod say;

#[path = "common/capped.rs"]
mod capped;

/// The largest factor `--allow-raise` raises a limit by.
const MAX_FACTOR: u32 = 4;

/// The exit status for an image with data missing.
const PARTIAL: u8 = 2;

fn print_limit(limit: &LimitDetail) {
    say!("refused by a limit: {limit}");
    say!("  kind: {:?}", limit.kind());
    match limit.retry_with() {
        Some(raise) => say!("  retry with: {raise:?}"),
        None => say!("  retry with: no option raises it"),
    }
    if let Some(at) = limit.refused_at() {
        say!(
            "  refused at: tile {}, which needs {} ({:?})",
            at.tile(),
            at.amount(),
            at.unit()
        );
    }
}

fn print_report(report: &DecodeReport) {
    say!("complete: {}", report.is_complete());
    match report.complete_reduction {
        Some(r) => say!("complete at reduction: {r}"),
        None => say!("complete at reduction: none"),
    }
    for line in capped::capped(&report.entries) {
        say!("  {line}");
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

/// Decode the file; `Ok(true)` when the image is complete.
fn run(args: &[OsString]) -> Result<bool, String> {
    let (allow_raise, path) = parse(args)?;
    // `{:?}` quotes the name and escapes control characters, so a file name
    // from outside can't write to the terminal.
    let name = format!("{path:?}");
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
                let factor = match limit.retry_with() {
                    Some(Raise::Raised(factor)) => factor,
                    // Only lifting the limit would admit the file.
                    Some(_) => {
                        return Err(format!(
                            "{name}: refused; only an unlimited limit would \
                             admit it, and this example never sets one"
                        ));
                    }
                    // Such as `AddressSpace`, `PocVolumes` or `BitPlanes`.
                    None => {
                        let why = "refused by a limit no option raises";
                        return Err(format!("{name}: {why}"));
                    }
                };
                if raised {
                    return Err(format!("{name}: refused after one raise"));
                }
                if factor > MAX_FACTOR {
                    return Err(format!(
                        "{name}: needs a factor of {factor}, past this \
                         example's cap of {MAX_FACTOR}"
                    ));
                }
                if !allow_raise {
                    return Err(format!(
                        "{name}: refused; pass --allow-raise to retry with \
                         the limit raised, for a file you trust"
                    ));
                }
                let Some(next) = options.raised_for(&limit) else {
                    return Err(format!("{name}: no option raises this limit"));
                };
                say!("retrying once with the limit raised");
                options = next;
                raised = true;
            }
            Error::Codestream(detail) if options.strict() => {
                say!("refused as damaged or cut short: {detail}");
                say!("decoding again with strict off");
                options = options.with_strict(false);
            }
            other => return Err(format!("{name}: {other}")),
        }
    };

    let n = image.components.len();
    say!(
        "decoded {name}: {}x{}, {n} {}",
        image.width,
        image.height,
        if n == 1 { "channel" } else { "channels" }
    );
    print_report(&image.report);
    Ok(image.report.is_complete())
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(PARTIAL),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
