//! One decoder per command line, for `docs/performance.md`.
//!
//! ```text
//! perf-decode ENGINE INPUT [--out PREFIX | --out8 FILE | --iters N]
//!             [--window X0,Y0,X1,Y1] [--tile N] [--incremental BYTES] [--srgb]
//! ```
//!
//! `ENGINE` is `rust-j2k` or `hayro`.
//!
//! - With none of `--out`, `--out8` and `--iters`, decode once and write
//!   nothing.
//! - `--out PREFIX` decodes once and writes one PGX file per component,
//!   `PREFIX_K.pgx`, at the component's own depth, as `opj_decompress` does.
//!   For `hayro` these are the components before its 8-bit step, so the
//!   output check sees the decoder's real samples.
//! - `--out8 FILE` (`hayro` only) writes the interleaved 8-bit bytes its API
//!   delivers.
//! - `--iters N` decodes once to warm up, then N timed times in process, and
//!   prints the times in milliseconds on one line.
//! - `--incremental BYTES` (`rust-j2k` only) feeds the file to an
//!   `IncrementalDecoder` in pieces of BYTES, calls `refine` after each, and
//!   ends with `finish`. Time it as `--iters` does.
//!
//! `--srgb` (`rust-j2k` only) asks for sRGB output, as `opj_decompress` gives
//! for a JP2 file with an ICC profile or sYCC colors.
//!
//! In every mode other than `--out`, `hayro` decodes as its API gives the
//! image: the decode, then the interleaved 8-bit bytes (`data_u8`).

use std::fs::File;
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::time::Instant;

use rust_j2k::{ColorOutput, DecodeOptions, IncrementalDecoder, Window, decode_with};

struct Args {
    engine: String,
    input: String,
    out: Option<String>,
    out8: Option<String>,
    iters: Option<usize>,
    window: Option<Window>,
    tile: Option<u32>,
    incremental: Option<usize>,
    srgb: bool,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let engine = it.next().ok_or("missing ENGINE")?;
    let input = it.next().ok_or("missing INPUT")?;
    let mut a = Args {
        engine,
        input,
        out: None,
        out8: None,
        iters: None,
        window: None,
        tile: None,
        incremental: None,
        srgb: false,
    };
    while let Some(flag) = it.next() {
        let mut val = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--srgb" => a.srgb = true,
            "--out" => a.out = Some(val()?),
            "--out8" => a.out8 = Some(val()?),
            "--iters" => a.iters = Some(val()?.parse().map_err(|e| format!("--iters: {e}"))?),
            "--tile" => a.tile = Some(val()?.parse().map_err(|e| format!("--tile: {e}"))?),
            "--incremental" => {
                a.incremental = Some(val()?.parse().map_err(|e| format!("--incremental: {e}"))?)
            }
            "--window" => {
                let v: Vec<u32> = val()?
                    .split(',')
                    .map(|s| s.parse().map_err(|e| format!("--window: {e}")))
                    .collect::<Result<_, _>>()?;
                let [x0, y0, x1, y1] = v[..] else {
                    return Err("--window needs X0,Y0,X1,Y1".into());
                };
                a.window = Some(Window::new(x0, y0, x1, y1));
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(a)
}

/// One component to write: its depth, sign, size and samples.
struct Plane {
    depth: u8,
    signed: bool,
    width: u32,
    height: u32,
    samples: Vec<i32>,
}

/// What a decode hands back, when the caller asked to keep it.
enum Kept {
    Nothing,
    Planes(Vec<Plane>),
    Bytes(Vec<u8>),
}

fn write_pgx(prefix: &str, planes: &[Plane]) -> std::io::Result<()> {
    for (k, p) in planes.iter().enumerate() {
        let mut w = BufWriter::new(File::create(format!("{prefix}_{k}.pgx"))?);
        let sign = if p.signed { '-' } else { '+' };
        writeln!(w, "PG ML {sign} {} {} {}", p.depth, p.width, p.height)?;
        for &s in &p.samples {
            if p.depth <= 8 {
                w.write_all(&[s as u8])?;
            } else {
                w.write_all(&(s as u16).to_be_bytes())?;
            }
        }
        w.flush()?;
    }
    Ok(())
}

fn options(a: &Args) -> DecodeOptions {
    let color = if a.srgb {
        ColorOutput::Srgb
    } else {
        ColorOutput::AsCoded
    };
    DecodeOptions::default()
        .with_window(a.window)
        .with_tile(a.tile)
        .with_color(color)
}

fn run_rust(a: &Args, bytes: &[u8], keep: bool) -> Result<Kept, String> {
    let opts = options(a);
    let image = match a.incremental {
        None => decode_with(black_box(bytes), black_box(&opts)).map_err(|e| e.to_string())?,
        Some(piece) => {
            let mut d = IncrementalDecoder::new(opts).map_err(|e| e.to_string())?;
            for chunk in bytes.chunks(piece) {
                d.feed(chunk);
                // The image a refine returns is dropped, as a viewer that
                // shows each one would drop the last.
                black_box(d.refine().map_err(|e| e.to_string())?);
            }
            d.finish().map_err(|e| e.to_string())?
        }
    };
    if !keep {
        black_box(image);
        return Ok(Kept::Nothing);
    }
    Ok(Kept::Planes(
        image
            .components
            .into_iter()
            .map(|c| Plane {
                depth: c.bit_depth,
                signed: c.signed,
                width: c.width,
                height: c.height,
                samples: c.samples,
            })
            .collect(),
    ))
}

fn run_hayro(a: &Args, bytes: &[u8], planes: bool, bytes8: bool) -> Result<Kept, String> {
    use hayro_jpeg2000::{DecodeSettings, DecoderContext, Image};
    if a.window.is_some() || a.tile.is_some() || a.incremental.is_some() {
        return Err("hayro-jpeg2000 has no window, tile or incremental decode".into());
    }
    let image =
        Image::new(black_box(bytes), &DecodeSettings::default()).map_err(|e| format!("{e:?}"))?;
    let mut ctx = DecoderContext::default();
    let decoded = image.decode(&mut ctx).map_err(|e| format!("{e:?}"))?;
    if planes {
        let (w, h) = (image.width(), image.height());
        return Ok(Kept::Planes(
            decoded
                .components()
                .iter()
                .map(|c| Plane {
                    depth: c.bit_depth(),
                    signed: false,
                    width: w,
                    height: h,
                    // The decoder's own 8-bit step clamps to the depth's range,
                    // and so does this.
                    samples: c
                        .samples()
                        .iter()
                        .map(|&v| (v.round() as i32).clamp(0, (1 << c.bit_depth()) - 1))
                        .collect(),
                })
                .collect(),
        ));
    }
    let data = decoded.data_u8();
    if bytes8 {
        Ok(Kept::Bytes(data))
    } else {
        black_box(data);
        Ok(Kept::Nothing)
    }
}

fn run(a: &Args, bytes: &[u8], planes: bool, bytes8: bool) -> Result<Kept, String> {
    match a.engine.as_str() {
        "rust-j2k" if bytes8 => Err("--out8 is for hayro".into()),
        "rust-j2k" => run_rust(a, bytes, planes),
        "hayro" => run_hayro(a, bytes, planes, bytes8),
        e => Err(format!("unknown engine {e}")),
    }
}

fn work(a: &Args) -> Result<(), String> {
    let bytes = std::fs::read(&a.input).map_err(|e| format!("{}: {e}", a.input))?;
    if let Some(n) = a.iters {
        run(a, &bytes, false, false)?;
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let kept = run(a, &bytes, false, false)?;
            let ms = t.elapsed().as_secs_f64() * 1e3;
            black_box(kept);
            times.push(format!("{ms:.3}"));
        }
        println!("{}", times.join(" "));
    } else if let Some(prefix) = &a.out {
        if let Kept::Planes(p) = run(a, &bytes, true, false)? {
            write_pgx(prefix, &p).map_err(|e| e.to_string())?;
        }
    } else if let Some(file) = &a.out8 {
        if let Kept::Bytes(b) = run(a, &bytes, false, true)? {
            std::fs::write(file, b).map_err(|e| e.to_string())?;
        }
    } else {
        run(a, &bytes, false, false)?;
    }
    Ok(())
}

fn main() {
    let a = parse().unwrap_or_else(|e| {
        eprintln!("perf-decode: {e}");
        std::process::exit(2);
    });
    if let Err(e) = work(&a) {
        eprintln!("perf-decode: {e}");
        std::process::exit(1);
    }
}
